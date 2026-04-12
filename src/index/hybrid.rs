use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, params};

use crate::{Query, ZgResult};

use super::code_symbols::build_symbol_chunks;
use super::db::{ensure_index_root, open_existing_db, validate_schema};
use super::embed::embed_query;
use super::files::build_chunks;
use super::types::{
    FTS_CANDIDATE_LIMIT, RRF_K, ScopeKind, SearchHit, StoredChunk, VECTOR_CANDIDATE_LIMIT,
};
use super::util::scope_kind;

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

    let query_vector = embed_query(normalized.normalized())?;
    let lexical_rows = lexical_candidates(&conn, &root, &scope, &normalized)?;
    let vector_rows = vector_candidates(&conn, &root, &scope, &query_vector)?;
    let hits = merge_ranked_hits(lexical_rows, vector_rows);
    let mut hits = materialize_live_snippets(&root, hits);
    hits.truncate(limit);
    Ok(hits)
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
                "SELECT cr.id, f.rel_path, cr.chunk_index, cr.line_start, cr.line_end, cr.chunk_kind, cr.language, cr.normalized_text, bm25(fts_chunks)
                 FROM fts_chunks
                 JOIN chunk_refs cr ON cr.id = fts_chunks.rowid
                 JOIN files f ON f.id = cr.file_id
                 WHERE fts_chunks MATCH ?1
                 ORDER BY bm25(fts_chunks)
                 LIMIT ?2",
            )?;
            collect_lexical_rows(stmt.query_map(
                params![fts_query, FTS_CANDIDATE_LIMIT as i64],
                |row| {
                    let normalized_text: String = row.get(7)?;
                    let bm25: f64 = row.get(8)?;
                    Ok(StoredChunk {
                        chunk_id: row.get(0)?,
                        rel_path: row.get(1)?,
                        chunk_index: row.get::<_, i64>(2)? as usize,
                        line_start: row.get::<_, i64>(3)? as usize,
                        line_end: row.get::<_, i64>(4)? as usize,
                        chunk_kind: row.get(5)?,
                        language: row.get(6)?,
                        lexical_score: lexical_score(&normalized_text, query, bm25),
                        vector_score: 0.0,
                    })
                },
            )?)
        }
        ScopeKind::File(rel_path) => {
            let mut stmt = conn.prepare(
                "SELECT cr.id, f.rel_path, cr.chunk_index, cr.line_start, cr.line_end, cr.chunk_kind, cr.language, cr.normalized_text, bm25(fts_chunks)
                 FROM fts_chunks
                 JOIN chunk_refs cr ON cr.id = fts_chunks.rowid
                 JOIN files f ON f.id = cr.file_id
                 WHERE fts_chunks MATCH ?1
                   AND f.rel_path = ?2
                 ORDER BY bm25(fts_chunks)
                 LIMIT ?3",
            )?;
            collect_lexical_rows(stmt.query_map(
                params![fts_query, rel_path, FTS_CANDIDATE_LIMIT as i64],
                |row| {
                    let normalized_text: String = row.get(7)?;
                    let bm25: f64 = row.get(8)?;
                    Ok(StoredChunk {
                        chunk_id: row.get(0)?,
                        rel_path: row.get(1)?,
                        chunk_index: row.get::<_, i64>(2)? as usize,
                        line_start: row.get::<_, i64>(3)? as usize,
                        line_end: row.get::<_, i64>(4)? as usize,
                        chunk_kind: row.get(5)?,
                        language: row.get(6)?,
                        lexical_score: lexical_score(&normalized_text, query, bm25),
                        vector_score: 0.0,
                    })
                },
            )?)
        }
        ScopeKind::Directory(rel_path, prefix) => {
            let mut stmt = conn.prepare(
                "SELECT cr.id, f.rel_path, cr.chunk_index, cr.line_start, cr.line_end, cr.chunk_kind, cr.language, cr.normalized_text, bm25(fts_chunks)
                 FROM fts_chunks
                 JOIN chunk_refs cr ON cr.id = fts_chunks.rowid
                 JOIN files f ON f.id = cr.file_id
                 WHERE fts_chunks MATCH ?1
                   AND (f.rel_path = ?2 OR f.rel_path LIKE ?3)
                 ORDER BY bm25(fts_chunks)
                 LIMIT ?4",
            )?;
            collect_lexical_rows(stmt.query_map(
                params![fts_query, rel_path, prefix, FTS_CANDIDATE_LIMIT as i64],
                |row| {
                    let normalized_text: String = row.get(7)?;
                    let bm25: f64 = row.get(8)?;
                    Ok(StoredChunk {
                        chunk_id: row.get(0)?,
                        rel_path: row.get(1)?,
                        chunk_index: row.get::<_, i64>(2)? as usize,
                        line_start: row.get::<_, i64>(3)? as usize,
                        line_end: row.get::<_, i64>(4)? as usize,
                        chunk_kind: row.get(5)?,
                        language: row.get(6)?,
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
    let query_blob = encode_vector(query_vector);
    let rows = match scope_kind(root, scope)? {
        ScopeKind::Root => {
            let mut stmt = conn.prepare(
                "WITH knn_matches AS (
                    SELECT shared_chunk_id, distance
                    FROM vec_index
                    WHERE embedding MATCH ?1
                      AND k = ?2
                )
                SELECT
                    cr.id,
                    f.rel_path,
                    cr.chunk_index,
                    cr.line_start,
                    cr.line_end,
                    cr.chunk_kind,
                    cr.language,
                    km.distance
                FROM knn_matches km
                JOIN chunk_refs cr ON cr.shared_chunk_id = km.shared_chunk_id
                JOIN files f ON f.id = cr.file_id
                ORDER BY km.distance ASC, cr.id ASC",
            )?;
            collect_vector_rows(stmt.query_map(
                params![query_blob, VECTOR_CANDIDATE_LIMIT as i64],
                vector_row_from_distance,
            )?)
        }
        ScopeKind::File(rel_path) => {
            let mut stmt = conn.prepare(
                "WITH knn_matches AS (
                    SELECT shared_chunk_id, distance
                    FROM vec_index
                    WHERE embedding MATCH ?1
                      AND k = ?2
                      AND shared_chunk_id IN (
                          SELECT cr.shared_chunk_id
                          FROM chunk_refs cr
                          JOIN files f ON f.id = cr.file_id
                          WHERE f.rel_path = ?3
                      )
                )
                SELECT
                    cr.id,
                    f.rel_path,
                    cr.chunk_index,
                    cr.line_start,
                    cr.line_end,
                    cr.chunk_kind,
                    cr.language,
                    km.distance
                FROM knn_matches km
                JOIN chunk_refs cr ON cr.shared_chunk_id = km.shared_chunk_id
                JOIN files f ON f.id = cr.file_id
                WHERE f.rel_path = ?3
                ORDER BY km.distance ASC, cr.id ASC",
            )?;
            collect_vector_rows(stmt.query_map(
                params![query_blob, VECTOR_CANDIDATE_LIMIT as i64, rel_path],
                vector_row_from_distance,
            )?)
        }
        ScopeKind::Directory(rel_path, prefix) => {
            let mut stmt = conn.prepare(
                "WITH knn_matches AS (
                    SELECT shared_chunk_id, distance
                    FROM vec_index
                    WHERE embedding MATCH ?1
                      AND k = ?2
                      AND shared_chunk_id IN (
                          SELECT cr.shared_chunk_id
                          FROM chunk_refs cr
                          JOIN files f ON f.id = cr.file_id
                          WHERE f.rel_path = ?3 OR f.rel_path LIKE ?4
                      )
                )
                SELECT
                    cr.id,
                    f.rel_path,
                    cr.chunk_index,
                    cr.line_start,
                    cr.line_end,
                    cr.chunk_kind,
                    cr.language,
                    km.distance
                FROM knn_matches km
                JOIN chunk_refs cr ON cr.shared_chunk_id = km.shared_chunk_id
                JOIN files f ON f.id = cr.file_id
                WHERE f.rel_path = ?3 OR f.rel_path LIKE ?4
                ORDER BY km.distance ASC, cr.id ASC",
            )?;
            collect_vector_rows(stmt.query_map(
                params![query_blob, VECTOR_CANDIDATE_LIMIT as i64, rel_path, prefix],
                vector_row_from_distance,
            )?)
        }
    }?;

    Ok(rows
        .into_iter()
        .filter(|row| row.vector_score > 0.0)
        .collect())
}

fn merge_ranked_hits(
    lexical_rows: Vec<StoredChunk>,
    vector_rows: Vec<StoredChunk>,
) -> Vec<SearchHit> {
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
                snippet: String::new(),
                line_start: row.line_start,
                line_end: row.line_end,
                score: lexical_rrf + vector_rrf,
                lexical_score: row.lexical_score,
                vector_score: row.vector_score,
                chunk_index: row.chunk_index,
                chunk_kind: row.chunk_kind,
                language: row.language,
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
    hits
}

fn materialize_live_snippets(root: &Path, mut hits: Vec<SearchHit>) -> Vec<SearchHit> {
    let mut by_path = HashMap::<String, Vec<usize>>::new();
    for (idx, hit) in hits.iter().enumerate() {
        by_path.entry(hit.rel_path.clone()).or_default().push(idx);
    }
    let mut keep = vec![false; hits.len()];

    for (rel_path, hit_indexes) in by_path {
        let file_path = root.join(&rel_path);
        let Ok(bytes) = fs::read(&file_path) else {
            continue;
        };
        let Ok(body) = String::from_utf8(bytes) else {
            continue;
        };
        for hit_idx in hit_indexes {
            let chunks = if hits[hit_idx].chunk_kind == "symbol" {
                build_symbol_chunks(&file_path, &body)
            } else {
                let Ok(chunks) = build_chunks(&body) else {
                    continue;
                };
                chunks
            };
            let Some(chunk) = chunks.get(hits[hit_idx].chunk_index) else {
                continue;
            };
            hits[hit_idx].snippet = render_snippet(&chunk.raw_text);
            hits[hit_idx].line_start = chunk.line_start;
            hits[hit_idx].line_end = chunk.line_end;
            keep[hit_idx] = true;
        }
    }

    hits.into_iter()
        .enumerate()
        .filter_map(|(idx, hit)| keep[idx].then_some(hit))
        .collect()
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

fn collect_lexical_rows(
    rows: impl Iterator<Item = rusqlite::Result<StoredChunk>>,
) -> ZgResult<Vec<StoredChunk>> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn collect_vector_rows(
    rows: impl Iterator<Item = rusqlite::Result<StoredChunk>>,
) -> ZgResult<Vec<StoredChunk>> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn vector_row_from_distance(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredChunk> {
    let distance: f64 = row.get(7)?;
    Ok(StoredChunk {
        chunk_id: row.get(0)?,
        rel_path: row.get(1)?,
        chunk_index: row.get::<_, i64>(2)? as usize,
        line_start: row.get::<_, i64>(3)? as usize,
        line_end: row.get::<_, i64>(4)? as usize,
        chunk_kind: row.get(5)?,
        language: row.get(6)?,
        lexical_score: 0.0,
        vector_score: 1.0 - distance,
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{SearchHit, StoredChunk, materialize_live_snippets, merge_ranked_hits};
    use crate::index::{init_index, search_hybrid};

    struct Row<'a> {
        chunk_id: i64,
        rel_path: &'a str,
        chunk_index: usize,
        line_start: usize,
        line_end: usize,
        lexical: f64,
        vector: f64,
    }

    fn row(input: Row<'_>) -> StoredChunk {
        StoredChunk {
            chunk_id: input.chunk_id,
            rel_path: input.rel_path.to_string(),
            chunk_index: input.chunk_index,
            line_start: input.line_start,
            line_end: input.line_end,
            chunk_kind: "text".to_string(),
            language: None,
            lexical_score: input.lexical,
            vector_score: input.vector,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zg-hybrid-{name}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hybrid_merge_keeps_lexical_only_and_vector_only_candidates_visible() {
        let hits = merge_ranked_hits(
            vec![row(Row {
                chunk_id: 1,
                rel_path: "alpha.md",
                chunk_index: 0,
                line_start: 1,
                line_end: 1,
                lexical: 2.0,
                vector: 0.0,
            })],
            vec![row(Row {
                chunk_id: 2,
                rel_path: "beta.md",
                chunk_index: 0,
                line_start: 1,
                line_end: 1,
                lexical: 0.0,
                vector: 0.9,
            })],
        );

        assert_eq!(hits.len(), 2);
        assert!(
            hits.iter()
                .any(|hit| hit.rel_path == "alpha.md" && hit.lexical_score > 0.0)
        );
        assert!(
            hits.iter()
                .any(|hit| hit.rel_path == "beta.md" && hit.vector_score > 0.0)
        );
    }

    #[test]
    fn hybrid_merge_rewards_dual_channel_hits() {
        let hits = merge_ranked_hits(
            vec![
                row(Row {
                    chunk_id: 1,
                    rel_path: "alpha.md",
                    chunk_index: 0,
                    line_start: 1,
                    line_end: 1,
                    lexical: 1.5,
                    vector: 0.0,
                }),
                row(Row {
                    chunk_id: 2,
                    rel_path: "beta.md",
                    chunk_index: 0,
                    line_start: 1,
                    line_end: 1,
                    lexical: 1.4,
                    vector: 0.0,
                }),
            ],
            vec![
                row(Row {
                    chunk_id: 1,
                    rel_path: "alpha.md",
                    chunk_index: 0,
                    line_start: 1,
                    line_end: 1,
                    lexical: 0.0,
                    vector: 0.8,
                }),
                row(Row {
                    chunk_id: 3,
                    rel_path: "gamma.md",
                    chunk_index: 0,
                    line_start: 1,
                    line_end: 1,
                    lexical: 0.0,
                    vector: 0.7,
                }),
            ],
        );

        assert_eq!(hits[0].rel_path, "alpha.md");
        assert!(
            hits.iter()
                .any(|hit| hit.rel_path == "beta.md" && hit.lexical_score > 0.0)
        );
        assert!(
            hits.iter()
                .any(|hit| hit.rel_path == "gamma.md" && hit.vector_score > 0.0)
        );
    }

    #[test]
    fn live_materialization_prefers_current_file_contents_over_stale_index_metadata() {
        let root = temp_dir("materialize-live");
        let file = root.join("alpha.md");
        fs::write(&file, "- real visible line").unwrap();
        init_index(&root).unwrap();

        let conn = crate::index::db::open_existing_db(&root).unwrap();
        conn.execute(
            "UPDATE chunk_refs SET line_start = 999, line_end = 999 WHERE file_id IN (SELECT id FROM files WHERE rel_path = 'alpha.md')",
            [],
        )
        .unwrap();

        let hits = search_hybrid(&root, &root, "real visible", 10).unwrap();
        assert_eq!(hits[0].snippet, "real visible line");
        assert_eq!(hits[0].line_start, 1);
    }

    #[test]
    fn materialize_live_snippets_drops_unreadable_results() {
        let root = temp_dir("materialize-fallback");
        let hits = vec![SearchHit {
            rel_path: "missing.md".to_string(),
            snippet: String::new(),
            line_start: 1,
            line_end: 1,
            score: 1.0,
            lexical_score: 1.0,
            vector_score: 0.0,
            chunk_index: 0,
            chunk_kind: "text".to_string(),
            language: None,
        }];

        let hits = materialize_live_snippets(&root, hits);
        assert!(hits.is_empty());
    }
}
