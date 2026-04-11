use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, params};

use crate::{Query, ZgResult};

use super::db::{ensure_index_root, open_existing_db, validate_schema};
use super::types::{
    FTS_CANDIDATE_LIMIT, RRF_K, ScopeKind, SearchHit, StoredChunk, VECTOR_CANDIDATE_LIMIT,
    VECTOR_DIMENSIONS,
};
use super::util::{fnv1a64, scope_kind};

pub fn search_hybrid(
    root: &Path,
    scope: &Path,
    query: &str,
    limit: usize,
) -> ZgResult<Vec<SearchHit>> {
    let root = crate::paths::resolve_existing_dir(root)?;
    ensure_index_root(&root)?;
    let scope = crate::paths::resolve_existing_path(scope)?;
    let conn = open_existing_db(&root)?;
    validate_schema(&conn)?;

    let normalized = Query::new(query);
    if normalized.is_empty() {
        return Err(crate::other("query is empty"));
    }

    let query_vector = vectorize(normalized.normalized());
    let lexical_rows = lexical_candidates(&conn, &root, &scope, &normalized)?;
    let vector_rows = vector_candidates(&conn, &root, &scope, &query_vector)?;

    let mut by_chunk = HashMap::<i64, StoredChunk>::new();
    let mut lexical_rank = HashMap::<i64, usize>::new();
    let mut vector_rank = HashMap::<i64, usize>::new();

    for (rank, row) in lexical_rows.into_iter().enumerate() {
        lexical_rank.insert(row.chunk_id, rank);
        by_chunk.insert(row.chunk_id, row);
    }
    for (rank, row) in vector_rows.into_iter().enumerate() {
        vector_rank.insert(row.chunk_id, rank);
        by_chunk
            .entry(row.chunk_id)
            .and_modify(|existing| {
                existing.vector_score = row.vector_score;
                if existing.raw_text.is_empty() {
                    existing.raw_text = row.raw_text.clone();
                }
                if existing.rel_path.is_empty() {
                    existing.rel_path = row.rel_path.clone();
                }
            })
            .or_insert(row);
    }

    let mut hits = by_chunk
        .into_values()
        .filter(|row| row.lexical_score > 0.0 || row.vector_score > 0.0)
        .map(|row| {
            let lexical_rrf = lexical_rank
                .get(&row.chunk_id)
                .map(|rank| 1.0 / (RRF_K + *rank as f64))
                .unwrap_or(0.0);
            let vector_rrf = vector_rank
                .get(&row.chunk_id)
                .map(|rank| 1.0 / (RRF_K + *rank as f64))
                .unwrap_or(0.0);
            SearchHit {
                rel_path: row.rel_path,
                snippet: render_snippet(&row.raw_text),
                score: lexical_rrf + vector_rrf,
                lexical_score: row.lexical_score,
                vector_score: row.vector_score,
            }
        })
        .collect::<Vec<_>>();

    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                right
                    .lexical_score
                    .partial_cmp(&left.lexical_score)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.rel_path.cmp(&right.rel_path))
    });
    hits.truncate(limit);
    Ok(hits)
}

pub(crate) fn vectorize(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0; VECTOR_DIMENSIONS];
    for token in text.split_whitespace() {
        let hash = fnv1a64(token.as_bytes());
        let index = (hash as usize) % VECTOR_DIMENSIONS;
        let sign = if ((hash >> 8) & 1) == 0 { 1.0 } else { -1.0 };
        vector[index] += sign;
    }
    normalize_vector(&mut vector);
    vector
}

pub(crate) fn encode_vector(vector: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(std::mem::size_of_val(vector));
    for value in vector {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn lexical_candidates(
    conn: &Connection,
    root: &Path,
    scope: &Path,
    query: &Query,
) -> ZgResult<Vec<StoredChunk>> {
    let fts_query = query
        .terms()
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");

    match scope_kind(root, scope)? {
        ScopeKind::Root => {
            let mut stmt = conn.prepare(
                "SELECT c.id, f.rel_path, c.raw_text, c.normalized_text, bm25(fts_chunks)
                 FROM fts_chunks
                 JOIN chunks c ON c.id = fts_chunks.rowid
                 JOIN files f ON f.id = c.file_id
                 WHERE fts_chunks MATCH ?1
                 ORDER BY bm25(fts_chunks)
                 LIMIT ?2",
            )?;
            collect_lexical_rows(stmt.query_map(
                params![fts_query, FTS_CANDIDATE_LIMIT as i64],
                |row| {
                    let normalized_text: String = row.get(3)?;
                    let bm25: f64 = row.get(4)?;
                    Ok(StoredChunk {
                        chunk_id: row.get(0)?,
                        rel_path: row.get(1)?,
                        raw_text: row.get(2)?,
                        lexical_score: lexical_score(&normalized_text, query, bm25),
                        vector_score: 0.0,
                    })
                },
            )?)
        }
        ScopeKind::File(rel_path) => {
            let mut stmt = conn.prepare(
                "SELECT c.id, f.rel_path, c.raw_text, c.normalized_text, bm25(fts_chunks)
                 FROM fts_chunks
                 JOIN chunks c ON c.id = fts_chunks.rowid
                 JOIN files f ON f.id = c.file_id
                 WHERE fts_chunks MATCH ?1
                   AND f.rel_path = ?2
                 ORDER BY bm25(fts_chunks)
                 LIMIT ?3",
            )?;
            collect_lexical_rows(stmt.query_map(
                params![fts_query, rel_path, FTS_CANDIDATE_LIMIT as i64],
                |row| {
                    let normalized_text: String = row.get(3)?;
                    let bm25: f64 = row.get(4)?;
                    Ok(StoredChunk {
                        chunk_id: row.get(0)?,
                        rel_path: row.get(1)?,
                        raw_text: row.get(2)?,
                        lexical_score: lexical_score(&normalized_text, query, bm25),
                        vector_score: 0.0,
                    })
                },
            )?)
        }
        ScopeKind::Directory(rel_path, prefix) => {
            let mut stmt = conn.prepare(
                "SELECT c.id, f.rel_path, c.raw_text, c.normalized_text, bm25(fts_chunks)
                 FROM fts_chunks
                 JOIN chunks c ON c.id = fts_chunks.rowid
                 JOIN files f ON f.id = c.file_id
                 WHERE fts_chunks MATCH ?1
                   AND (f.rel_path = ?2 OR f.rel_path LIKE ?3)
                 ORDER BY bm25(fts_chunks)
                 LIMIT ?4",
            )?;
            collect_lexical_rows(stmt.query_map(
                params![fts_query, rel_path, prefix, FTS_CANDIDATE_LIMIT as i64],
                |row| {
                    let normalized_text: String = row.get(3)?;
                    let bm25: f64 = row.get(4)?;
                    Ok(StoredChunk {
                        chunk_id: row.get(0)?,
                        rel_path: row.get(1)?,
                        raw_text: row.get(2)?,
                        lexical_score: lexical_score(&normalized_text, query, bm25),
                        vector_score: 0.0,
                    })
                },
            )?)
        }
    }
}

fn vector_candidates(
    conn: &Connection,
    root: &Path,
    scope: &Path,
    query_vector: &[f32],
) -> ZgResult<Vec<StoredChunk>> {
    let mut out = match scope_kind(root, scope)? {
        ScopeKind::Root => {
            let mut stmt = conn.prepare(
                "SELECT c.id, f.rel_path, c.raw_text, v.vector
                 FROM chunks c
                 JOIN files f ON f.id = c.file_id
                 JOIN vec_chunks v ON v.chunk_id = c.id",
            )?;
            stmt.query_map([], |row| vector_row(row, query_vector))?
                .collect::<Result<Vec<_>, _>>()?
        }
        ScopeKind::File(rel_path) => {
            let mut stmt = conn.prepare(
                "SELECT c.id, f.rel_path, c.raw_text, v.vector
                 FROM chunks c
                 JOIN files f ON f.id = c.file_id
                 JOIN vec_chunks v ON v.chunk_id = c.id
                 WHERE f.rel_path = ?1",
            )?;
            stmt.query_map([rel_path], |row| vector_row(row, query_vector))?
                .collect::<Result<Vec<_>, _>>()?
        }
        ScopeKind::Directory(rel_path, prefix) => {
            let mut stmt = conn.prepare(
                "SELECT c.id, f.rel_path, c.raw_text, v.vector
                 FROM chunks c
                 JOIN files f ON f.id = c.file_id
                 JOIN vec_chunks v ON v.chunk_id = c.id
                 WHERE f.rel_path = ?1 OR f.rel_path LIKE ?2",
            )?;
            stmt.query_map(params![rel_path, prefix], |row| {
                vector_row(row, query_vector)
            })?
            .collect::<Result<Vec<_>, _>>()?
        }
    };

    out.sort_by(|left, right| {
        right
            .vector_score
            .partial_cmp(&left.vector_score)
            .unwrap_or(Ordering::Equal)
    });
    out.retain(|row| row.vector_score > 0.0);
    out.truncate(VECTOR_CANDIDATE_LIMIT);
    Ok(out)
}

fn lexical_score(text: &str, query: &Query, bm25: f64) -> f64 {
    let coverage = query
        .terms()
        .iter()
        .filter(|term| text.contains(term.as_str()))
        .count() as f64
        / query.terms().len().max(1) as f64;
    coverage + (-bm25).max(0.0)
}

fn normalize_vector(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector {
            *value /= norm;
        }
    }
}

fn decode_vector(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap_or([0; 4])))
        .collect()
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    left.iter()
        .zip(right.iter())
        .map(|(l, r)| f64::from(*l) * f64::from(*r))
        .sum::<f64>()
}

fn collect_lexical_rows(
    rows: impl Iterator<Item = rusqlite::Result<StoredChunk>>,
) -> ZgResult<Vec<StoredChunk>> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn vector_row(row: &rusqlite::Row<'_>, query_vector: &[f32]) -> rusqlite::Result<StoredChunk> {
    let blob: Vec<u8> = row.get(3)?;
    let vector = decode_vector(&blob);
    Ok(StoredChunk {
        chunk_id: row.get(0)?,
        rel_path: row.get(1)?,
        raw_text: row.get(2)?,
        lexical_score: 0.0,
        vector_score: cosine_similarity(&vector, query_vector),
    })
}

fn render_snippet(raw_text: &str) -> String {
    let snippet = raw_text.trim();
    if snippet.len() <= 200 {
        return snippet.to_string();
    }

    let mut cutoff = 200;
    while !snippet.is_char_boundary(cutoff) {
        cutoff -= 1;
    }
    format!("{}...", &snippet[..cutoff])
}
