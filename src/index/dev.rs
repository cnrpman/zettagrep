use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::paths;
use crate::{ZgResult, other};

use super::db::{open_existing_db, validate_schema};
use super::files::load_indexable_document;
use super::sync::{init_index_with_level, reconcile_covering_roots, require_index_root_for_search};
use super::types::SearchHit;
use super::{IndexLevel, IndexStatus, load_status, search_indexed};

#[derive(Clone, Debug, Deserialize)]
pub struct SearchQualityFixtureSuite {
    pub suite_id: String,
    pub sample_vault_manifest: String,
    pub default_limit: usize,
    pub cases: Vec<SearchQualityCase>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SearchQualityCase {
    pub id: String,
    pub query: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub expectations: SearchQualityExpectations,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SearchQualityExpectations {
    #[serde(default)]
    pub must_include: Vec<ExpectedSearchHit>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ExpectedSearchHit {
    pub path: String,
    pub within_top: usize,
    #[serde(default)]
    pub snippet_contains: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchQualityGoldenSuite {
    pub suite_id: String,
    pub sample_vault_manifest: String,
    pub cases: Vec<SearchQualityGoldenCase>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchQualityGoldenCase {
    pub id: String,
    pub query: String,
    pub scope: Option<String>,
    pub limit: usize,
    pub hits: Vec<SearchQualityGoldenHit>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchQualityGoldenHit {
    pub rank: usize,
    pub rel_path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub snippet: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchQualityReport {
    pub fixture_path: PathBuf,
    pub golden_path: Option<PathBuf>,
    pub vault_root: PathBuf,
    pub suite_id: String,
    pub total_cases: usize,
    pub passed_cases: usize,
    pub expectation_failures: usize,
    pub golden_failures: usize,
    pub cases: Vec<SearchQualityCaseReport>,
}

impl SearchQualityReport {
    pub fn passed(&self) -> bool {
        self.expectation_failures == 0 && self.golden_failures == 0
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchQualityCaseReport {
    pub id: String,
    pub query: String,
    pub scope: Option<String>,
    pub notes: Option<String>,
    pub limit: usize,
    pub passed: bool,
    pub expectation_failures: Vec<String>,
    pub golden_failures: Vec<String>,
    pub hits: Vec<SearchQualityObservedHit>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchQualityObservedHit {
    pub rank: usize,
    pub rel_path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub snippet: String,
    pub score: f64,
    pub lexical_score: f64,
    pub vector_score: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChunkProbeReport {
    pub path: PathBuf,
    pub chunk_count: usize,
    pub chunks: Vec<ChunkProbeEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChunkProbeEntry {
    pub chunk_index: usize,
    pub chunk_kind: String,
    pub line_start: usize,
    pub line_end: usize,
    pub language: Option<String>,
    pub symbol_kind: Option<String>,
    pub container: Option<String>,
    pub raw_text: String,
    pub normalized_text: String,
    pub shared_normalized_text: String,
    pub shared_normalized_text_hash: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DbCacheProbeReport {
    pub requested_path: PathBuf,
    pub index_root: PathBuf,
    pub status: IndexStatus,
    pub totals: DbCacheTotals,
    pub chunk_kinds: Vec<KeyCount>,
    pub symbol_languages: Vec<KeyCount>,
    pub symbol_kinds: Vec<KeyCount>,
    pub top_files_by_chunks: Vec<FileChunkCount>,
    pub top_shared_chunks: Vec<SharedChunkCount>,
    pub last_index_run: Option<IndexRunProbe>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DbCacheTotals {
    pub files: u64,
    pub chunk_refs: u64,
    pub shared_chunks: u64,
    pub vec_chunks: u64,
    pub vec_index_rows: u64,
    pub fts_rows: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct KeyCount {
    pub key: String,
    pub count: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileChunkCount {
    pub rel_path: String,
    pub chunk_count: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SharedChunkCount {
    pub ref_count: u64,
    pub normalized_text_preview: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct IndexRunProbe {
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: u64,
    pub status: String,
    pub scanned_files: u64,
    pub indexed_files: u64,
    pub chunks_indexed: u64,
    pub error: Option<String>,
}

pub fn load_search_quality_fixture(path: &Path) -> ZgResult<SearchQualityFixtureSuite> {
    let path = paths::resolve_existing_path(path)?;
    let bytes = fs::read(&path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn load_search_quality_golden(path: &Path) -> ZgResult<SearchQualityGoldenSuite> {
    let path = paths::resolve_existing_path(path)?;
    let bytes = fs::read(&path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn write_search_quality_golden(
    fixture_path: &Path,
    golden_path: &Path,
    vault_root: &Path,
) -> ZgResult<SearchQualityGoldenSuite> {
    let fixture = load_search_quality_fixture(fixture_path)?;
    let suite = build_search_quality_golden(&fixture, vault_root)?;
    if let Some(parent) = golden_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(golden_path, serde_json::to_string_pretty(&suite)?)?;
    Ok(suite)
}

pub fn run_search_quality_suite(
    fixture_path: &Path,
    golden_path: Option<&Path>,
    vault_root: &Path,
) -> ZgResult<SearchQualityReport> {
    let fixture_path = paths::resolve_existing_path(fixture_path)?;
    let vault_root = paths::resolve_existing_dir(vault_root)?;
    let fixture = load_search_quality_fixture(&fixture_path)?;
    let golden = match golden_path {
        Some(path) => Some(load_search_quality_golden(path)?),
        None => None,
    };

    let index_root = ensure_eval_index(&vault_root)?;
    reconcile_covering_roots(&vault_root)?;

    let golden_cases = golden
        .as_ref()
        .map(|suite| {
            suite
                .cases
                .iter()
                .map(|case| (case.id.as_str(), case))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    let mut reports = Vec::new();
    let mut expectation_failures = 0usize;
    let mut golden_failures = 0usize;

    for case in &fixture.cases {
        let limit = case.limit.unwrap_or(fixture.default_limit);
        let scope = resolve_case_scope(&vault_root, case.scope.as_deref())?;
        let hits = search_indexed(&index_root, &scope, &case.query, limit)?;
        let observed_hits = hits
            .into_iter()
            .enumerate()
            .map(|(index, hit)| observed_hit(index + 1, hit, &vault_root))
            .collect::<Vec<_>>();

        let mut case_expectation_failures = Vec::new();
        for expected in &case.expectations.must_include {
            match observed_hits
                .iter()
                .find(|hit| hit.rel_path == expected.path)
            {
                Some(hit) if hit.rank <= expected.within_top => {
                    if let Some(snippet) = &expected.snippet_contains {
                        if !hit.snippet.contains(snippet) {
                            case_expectation_failures.push(format!(
                                "`{}` appeared at rank {} but snippet did not contain {:?}",
                                expected.path, hit.rank, snippet
                            ));
                        }
                    }
                }
                Some(hit) => case_expectation_failures.push(format!(
                    "`{}` appeared at rank {}, expected within top {}",
                    expected.path, hit.rank, expected.within_top
                )),
                None => case_expectation_failures.push(format!(
                    "missing required hit `{}` within top {}",
                    expected.path, expected.within_top
                )),
            }
        }

        let mut case_golden_failures = Vec::new();
        if let Some(golden_case) = golden_cases.get(case.id.as_str()) {
            let actual = observed_hits
                .iter()
                .map(|hit| (&hit.rel_path, hit.line_start, hit.line_end, &hit.snippet))
                .collect::<Vec<_>>();
            let expected = golden_case
                .hits
                .iter()
                .map(|hit| (&hit.rel_path, hit.line_start, hit.line_end, &hit.snippet))
                .collect::<Vec<_>>();
            if actual != expected {
                case_golden_failures.push(format!(
                    "golden mismatch for `{}` (expected {} hits, got {})",
                    case.id,
                    expected.len(),
                    actual.len()
                ));

                let max = expected.len().max(actual.len());
                for index in 0..max {
                    match (golden_case.hits.get(index), observed_hits.get(index)) {
                        (Some(expected_hit), Some(actual_hit))
                            if expected_hit.rel_path == actual_hit.rel_path
                                && expected_hit.line_start == actual_hit.line_start
                                && expected_hit.line_end == actual_hit.line_end
                                && expected_hit.snippet == actual_hit.snippet => {}
                        (Some(expected_hit), Some(actual_hit)) => {
                            case_golden_failures.push(format!(
                                "rank {} expected {}:{}-{} {:?}, got {}:{}-{} {:?}",
                                index + 1,
                                expected_hit.rel_path,
                                expected_hit.line_start,
                                expected_hit.line_end,
                                expected_hit.snippet,
                                actual_hit.rel_path,
                                actual_hit.line_start,
                                actual_hit.line_end,
                                actual_hit.snippet,
                            ));
                        }
                        (Some(expected_hit), None) => case_golden_failures.push(format!(
                            "rank {} expected {}:{}-{} {:?}, got <missing>",
                            index + 1,
                            expected_hit.rel_path,
                            expected_hit.line_start,
                            expected_hit.line_end,
                            expected_hit.snippet,
                        )),
                        (None, Some(actual_hit)) => case_golden_failures.push(format!(
                            "rank {} expected <missing>, got {}:{}-{} {:?}",
                            index + 1,
                            actual_hit.rel_path,
                            actual_hit.line_start,
                            actual_hit.line_end,
                            actual_hit.snippet,
                        )),
                        (None, None) => {}
                    }
                }
            }
        }

        if !case_expectation_failures.is_empty() {
            expectation_failures += 1;
        }
        if !case_golden_failures.is_empty() {
            golden_failures += 1;
        }

        reports.push(SearchQualityCaseReport {
            id: case.id.clone(),
            query: case.query.clone(),
            scope: case.scope.clone(),
            notes: case.notes.clone(),
            limit,
            passed: case_expectation_failures.is_empty() && case_golden_failures.is_empty(),
            expectation_failures: case_expectation_failures,
            golden_failures: case_golden_failures,
            hits: observed_hits,
        });
    }

    let passed_cases = reports.iter().filter(|case| case.passed).count();
    Ok(SearchQualityReport {
        fixture_path,
        golden_path: golden_path.map(Path::to_path_buf),
        vault_root,
        suite_id: fixture.suite_id,
        total_cases: reports.len(),
        passed_cases,
        expectation_failures,
        golden_failures,
        cases: reports,
    })
}

pub fn probe_chunks(path: &Path) -> ZgResult<ChunkProbeReport> {
    let path = paths::resolve_existing_path(path)?;
    let document = load_indexable_document(&path)?.ok_or_else(|| {
        other(format!(
            "{} is not indexable under the current chunking/content rules",
            path.display()
        ))
    })?;

    Ok(ChunkProbeReport {
        path,
        chunk_count: document.chunks.len(),
        chunks: document
            .chunks
            .into_iter()
            .map(|chunk| ChunkProbeEntry {
                chunk_index: chunk.chunk_index,
                chunk_kind: chunk.chunk_kind,
                line_start: chunk.line_start,
                line_end: chunk.line_end,
                language: chunk.language,
                symbol_kind: chunk.symbol_kind,
                container: chunk.container,
                raw_text: chunk.raw_text,
                normalized_text: chunk.normalized_text,
                shared_normalized_text: chunk.shared_normalized_text,
                shared_normalized_text_hash: chunk.shared_normalized_text_hash,
            })
            .collect(),
    })
}

pub fn probe_db_cache(path: &Path, limit: usize) -> ZgResult<DbCacheProbeReport> {
    let requested_path = paths::resolve_existing_path(path)?;
    let mut status = load_status(&requested_path)?;
    let index_root = status.index_root.clone().ok_or_else(|| {
        other(format!(
            "no ancestor .zg index found for {}",
            requested_path.display()
        ))
    })?;
    let conn = open_existing_db(&index_root)?;
    validate_schema(&conn)?;
    status.requested_path = requested_path.clone();

    let totals = DbCacheTotals {
        files: count_rows(&conn, "files")?,
        chunk_refs: count_rows(&conn, "chunk_refs")?,
        shared_chunks: count_rows(&conn, "shared_chunks")?,
        vec_chunks: count_rows(&conn, "vec_chunks")?,
        vec_index_rows: count_rows(&conn, "vec_index")?,
        fts_rows: count_rows(&conn, "fts_chunks")?,
    };

    Ok(DbCacheProbeReport {
        requested_path,
        index_root,
        status,
        totals,
        chunk_kinds: query_key_counts(
            &conn,
            "SELECT chunk_kind, COUNT(*) FROM chunk_refs GROUP BY chunk_kind ORDER BY COUNT(*) DESC, chunk_kind ASC",
        )?,
        symbol_languages: query_key_counts(
            &conn,
            "SELECT COALESCE(language, '<none>'), COUNT(*) FROM chunk_refs GROUP BY COALESCE(language, '<none>') ORDER BY COUNT(*) DESC, COALESCE(language, '<none>') ASC",
        )?,
        symbol_kinds: query_key_counts(
            &conn,
            "SELECT COALESCE(symbol_kind, '<none>'), COUNT(*) FROM chunk_refs GROUP BY COALESCE(symbol_kind, '<none>') ORDER BY COUNT(*) DESC, COALESCE(symbol_kind, '<none>') ASC",
        )?,
        top_files_by_chunks: query_top_files_by_chunks(&conn, limit)?,
        top_shared_chunks: query_top_shared_chunks(&conn, limit)?,
        last_index_run: query_last_index_run(&conn)?,
    })
}

fn build_search_quality_golden(
    fixture: &SearchQualityFixtureSuite,
    vault_root: &Path,
) -> ZgResult<SearchQualityGoldenSuite> {
    let vault_root = paths::resolve_existing_dir(vault_root)?;
    let index_root = ensure_eval_index(&vault_root)?;
    reconcile_covering_roots(&vault_root)?;

    let mut cases = Vec::new();
    for case in &fixture.cases {
        let limit = case.limit.unwrap_or(fixture.default_limit);
        let scope = resolve_case_scope(&vault_root, case.scope.as_deref())?;
        let hits = search_indexed(&index_root, &scope, &case.query, limit)?;
        cases.push(SearchQualityGoldenCase {
            id: case.id.clone(),
            query: case.query.clone(),
            scope: case.scope.clone(),
            limit,
            hits: hits
                .into_iter()
                .enumerate()
                .map(|(index, hit)| SearchQualityGoldenHit {
                    rank: index + 1,
                    rel_path: hit.rel_path,
                    line_start: hit.line_start,
                    line_end: hit.line_end,
                    snippet: normalize_snippet(&hit.snippet, &vault_root),
                })
                .collect(),
        });
    }

    Ok(SearchQualityGoldenSuite {
        suite_id: fixture.suite_id.clone(),
        sample_vault_manifest: fixture.sample_vault_manifest.clone(),
        cases,
    })
}

fn resolve_case_scope(vault_root: &Path, scope: Option<&str>) -> ZgResult<PathBuf> {
    match scope {
        Some(relative) => paths::resolve_existing_path(&vault_root.join(relative)),
        None => Ok(vault_root.to_path_buf()),
    }
}

fn ensure_eval_index(vault_root: &Path) -> ZgResult<PathBuf> {
    match require_index_root_for_search(vault_root) {
        Ok(root) => Ok(root),
        Err(_) => {
            init_index_with_level(vault_root, IndexLevel::FtsVector)?;
            require_index_root_for_search(vault_root)
        }
    }
}

fn observed_hit(rank: usize, hit: SearchHit, vault_root: &Path) -> SearchQualityObservedHit {
    SearchQualityObservedHit {
        rank,
        rel_path: hit.rel_path,
        line_start: hit.line_start,
        line_end: hit.line_end,
        snippet: normalize_snippet(&hit.snippet, vault_root),
        score: hit.score,
        lexical_score: hit.lexical_score,
        vector_score: hit.vector_score,
    }
}

fn count_rows(conn: &rusqlite::Connection, table: &str) -> ZgResult<u64> {
    Ok(
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })? as u64,
    )
}

fn query_key_counts(conn: &rusqlite::Connection, sql: &str) -> ZgResult<Vec<KeyCount>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(KeyCount {
            key: row.get(0)?,
            count: row.get::<_, i64>(1)? as u64,
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn query_top_files_by_chunks(
    conn: &rusqlite::Connection,
    limit: usize,
) -> ZgResult<Vec<FileChunkCount>> {
    let mut stmt = conn.prepare(
        "SELECT f.rel_path, COUNT(*) AS chunk_count
         FROM chunk_refs cr
         JOIN files f ON f.id = cr.file_id
         GROUP BY f.rel_path
         ORDER BY chunk_count DESC, f.rel_path ASC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |row| {
        Ok(FileChunkCount {
            rel_path: row.get(0)?,
            chunk_count: row.get::<_, i64>(1)? as u64,
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn query_top_shared_chunks(
    conn: &rusqlite::Connection,
    limit: usize,
) -> ZgResult<Vec<SharedChunkCount>> {
    let mut stmt = conn.prepare(
        "SELECT ref_count, normalized_text
         FROM shared_chunks
         ORDER BY ref_count DESC, normalized_text ASC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |row| {
        let normalized_text: String = row.get(1)?;
        Ok(SharedChunkCount {
            ref_count: row.get::<_, i64>(0)? as u64,
            normalized_text_preview: truncate_preview(&normalized_text, 120),
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn query_last_index_run(conn: &rusqlite::Connection) -> ZgResult<Option<IndexRunProbe>> {
    let mut stmt = conn.prepare(
        "SELECT started_at_unix_ms, finished_at_unix_ms, status, scanned_files, indexed_files, chunks_indexed, error
         FROM index_runs
         ORDER BY id DESC
         LIMIT 1",
    )?;
    let probe = stmt
        .query_row([], |row| {
            Ok(IndexRunProbe {
                started_at_unix_ms: row.get::<_, i64>(0)? as u64,
                finished_at_unix_ms: row.get::<_, i64>(1)? as u64,
                status: row.get(2)?,
                scanned_files: row.get::<_, i64>(3)? as u64,
                indexed_files: row.get::<_, i64>(4)? as u64,
                chunks_indexed: row.get::<_, i64>(5)? as u64,
                error: row.get(6)?,
            })
        })
        .optional()?;
    Ok(probe)
}

fn truncate_preview(input: &str, limit: usize) -> String {
    if input.chars().count() <= limit {
        return input.to_string();
    }

    let mut end = limit;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &input[..end])
}

fn normalize_snippet(snippet: &str, vault_root: &Path) -> String {
    let root = vault_root.to_string_lossy().replace('\\', "/");
    snippet.replace(&root, "<VAULT_ROOT>")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        load_search_quality_fixture, probe_chunks, probe_db_cache, run_search_quality_suite,
        write_search_quality_golden,
    };
    use crate::index::init_index;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zg-index-dev-{name}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn chunk_probe_uses_current_chunking_rules() {
        let root = temp_dir("chunk-probe");
        let file = root.join("note.md");
        fs::write(&file, "- alpha\nbeta :: gamma\n").unwrap();

        let report = probe_chunks(&file).unwrap();
        assert_eq!(report.chunk_count, 1);
        assert_eq!(report.chunks[0].raw_text, "alpha\nbeta\ngamma");
    }

    #[test]
    fn db_cache_probe_reports_shared_reuse() {
        let root = temp_dir("db-probe");
        fs::write(root.join("a.md"), "shared note").unwrap();
        fs::write(root.join("b.md"), "shared note").unwrap();
        init_index(&root).unwrap();

        let report = probe_db_cache(&root, 5).unwrap();
        assert_eq!(report.totals.files, 2);
        assert_eq!(report.totals.shared_chunks, 1);
        assert_eq!(report.top_shared_chunks[0].ref_count, 2);
    }

    #[test]
    fn search_quality_suite_checks_expectations_and_goldens() {
        let root = temp_dir("quality-suite");
        fs::write(root.join("alpha.md"), "sqlite vector adapter").unwrap();
        fs::write(root.join("beta.md"), "haystack builder").unwrap();

        let fixture = root.join("fixture.json");
        let golden = root.join("golden.json");
        fs::write(
            &fixture,
            serde_json::to_string_pretty(&serde_json::json!({
                "suite_id": "mini",
                "sample_vault_manifest": "sample.json",
                "default_limit": 2,
                "cases": [
                    {
                        "id": "sqlite-adapter",
                        "query": "sqlite adapter",
                        "expectations": {
                            "must_include": [
                                {
                                    "path": "alpha.md",
                                    "within_top": 1,
                                    "snippet_contains": "sqlite vector adapter"
                                }
                            ]
                        }
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let parsed = load_search_quality_fixture(&fixture).unwrap();
        assert_eq!(parsed.suite_id, "mini");

        write_search_quality_golden(&fixture, &golden, &root).unwrap();
        let report = run_search_quality_suite(&fixture, Some(&golden), &root).unwrap();
        assert!(report.passed());
        assert_eq!(report.total_cases, 1);
        assert_eq!(report.passed_cases, 1);
    }

    #[test]
    fn truncate_preview_preserves_char_boundaries() {
        let text = "alpha beta gamma delta epsilon";
        let preview = super::truncate_preview(text, 10);
        assert!(preview.starts_with("alpha beta"));
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn normalize_snippet_scrubs_absolute_vault_path() {
        let root = Path::new("/tmp/ripgrep");
        let snippet = "function demo in /tmp/ripgrep/crates/demo.rs";
        assert_eq!(
            super::normalize_snippet(snippet, root),
            "function demo in <VAULT_ROOT>/crates/demo.rs"
        );
    }
}
