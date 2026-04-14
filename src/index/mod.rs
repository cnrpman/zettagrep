mod code_symbols;
mod db;
mod dev;
mod embed;
mod files;
mod hybrid;
mod sync;
mod types;
mod util;

pub use dev::{
    ChunkProbeReport, DbCacheProbeReport, SearchQualityFixtureSuite, SearchQualityGoldenSuite,
    SearchQualityReport, load_search_quality_fixture, load_search_quality_golden, probe_chunks,
    probe_db_cache, run_search_quality_suite, write_search_quality_golden,
};
pub use files::collect_candidate_files;
pub use hybrid::{
    search_fts, search_fts_with_context, search_hybrid, search_hybrid_with_context, search_indexed,
    search_indexed_with_context,
};
pub use sync::{
    best_effort_overlap_note, delete_index, init_index, init_index_with_level, load_status,
    preflight_init, rebuild_index, rebuild_index_with_level, reconcile_covering_roots,
    require_index_root_for_search,
};
pub use types::{
    FTS_PROMPT_MAX_CHUNKS, IndexLevel, IndexStatus, InitPreflight, RebuildStats, SearchHit,
    VECTOR_PROMPT_MAX_CHUNKS,
};

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zg-index-{name}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn init_hybrid(root: &std::path::Path) {
        init_index_with_level(root, IndexLevel::FtsVector).unwrap();
    }

    fn long_chunk_body(line_count: usize) -> String {
        (0..line_count)
            .map(|index| {
                format!("line {index:05} xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    #[test]
    fn preflight_init_requires_force_for_oversized_vector_index() {
        let root = temp_dir("preflight-init-force");
        fs::write(
            root.join("alpha.md"),
            long_chunk_body(VECTOR_PROMPT_MAX_CHUNKS * 10 + 1),
        )
        .unwrap();

        let preflight = preflight_init(&root, IndexLevel::FtsVector).unwrap();

        assert_eq!(
            preflight.estimated_chunks,
            VECTOR_PROMPT_MAX_CHUNKS * 10 + 1
        );
        assert_eq!(preflight.recommended_chunk_limit, VECTOR_PROMPT_MAX_CHUNKS);
        assert_eq!(preflight.force_threshold, VECTOR_PROMPT_MAX_CHUNKS * 10);
        assert!(preflight.requires_force);
    }

    #[test]
    fn rebuild_creates_searchable_index() {
        let root = temp_dir("rebuild");
        fs::write(root.join("alpha.md"), "sqlite :: vector adapter").unwrap();
        fs::write(root.join("beta.md"), "rust search tooling").unwrap();

        let stats = init_index_with_level(&root, IndexLevel::FtsVector).unwrap();
        assert_eq!(stats.indexed_files, 2);

        let hits = search_hybrid(&root, &root, "sqlite adapter", 10).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].rel_path, "alpha.md");
        assert!(hits.iter().any(|hit| hit.rel_path == "alpha.md"));
        assert!(hits.iter().any(|hit| hit.vector_score > 0.0));
    }

    #[test]
    fn rebuild_batches_embedding_work_across_all_documents() {
        let root = temp_dir("rebuild-batch");
        fs::write(
            root.join("alpha.md"),
            "alpha unique long line one abcdefghijklmnop",
        )
        .unwrap();
        fs::write(
            root.join("beta.md"),
            "beta unique long line two abcdefghijklmnop",
        )
        .unwrap();

        super::embed::test_begin_embed_capture_for_current_thread();
        init_hybrid(&root);
        let (calls, texts) = super::embed::test_embed_counters();

        assert_eq!(calls, 1);
        assert_eq!(texts, 2);
    }

    #[test]
    fn repeated_normalized_text_across_files_shares_embedding_owner() {
        let root = temp_dir("shared-owner");
        fs::write(root.join("alpha.md"), "- Shared Note").unwrap();
        fs::write(root.join("beta.md"), "# shared note").unwrap();

        init_hybrid(&root);

        let conn = super::db::open_existing_db(&root).unwrap();
        let shared_count = conn
            .query_row("SELECT COUNT(*) FROM shared_chunks", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        let ref_count = conn
            .query_row("SELECT COUNT(*) FROM chunk_refs", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        let vec_count = conn
            .query_row("SELECT COUNT(*) FROM vec_index", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();

        assert_eq!(shared_count, 1);
        assert_eq!(ref_count, 2);
        assert_eq!(vec_count, 1);
    }

    #[test]
    fn code_symbol_search_indexes_supported_languages() {
        let root = temp_dir("code-symbol-search");
        fs::write(
            root.join("parser.rs"),
            "pub fn parse_query(input: &str) -> String {\n    let retry_backoff_ms = input.len();\n    retry_backoff_ms.to_string()\n}\n",
        )
        .unwrap();

        init_hybrid(&root);
        let hits = search_hybrid(&root, &root, "backoff", 10).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].rel_path, "parser.rs");
        assert!(hits[0].snippet.contains("retry_backoff_ms"));
        assert_eq!(hits[0].line_start, 2);
        assert_eq!(hits[0].line_end, 2);
    }

    #[test]
    fn deleting_last_reference_gcs_shared_embedding() {
        let root = temp_dir("gc-shared");
        let file = root.join("alpha.md");
        fs::write(&file, "shared note").unwrap();
        init_hybrid(&root);

        fs::remove_file(&file).unwrap();
        reconcile_covering_roots(&root).unwrap();

        let conn = super::db::open_existing_db(&root).unwrap();
        let shared_count = conn
            .query_row("SELECT COUNT(*) FROM shared_chunks", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        let vec_count = conn
            .query_row("SELECT COUNT(*) FROM vec_index", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        let ref_count = conn
            .query_row("SELECT COUNT(*) FROM chunk_refs", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();

        assert_eq!(shared_count, 0);
        assert_eq!(vec_count, 0);
        assert_eq!(ref_count, 0);
    }

    #[test]
    fn delete_index_removes_local_cache_directory() {
        let root = temp_dir("delete-index");
        fs::write(root.join("alpha.md"), "sqlite vector adapter").unwrap();
        init_hybrid(&root);
        assert!(root.join(".zg/index.db").exists());

        let removed = delete_index(&root).unwrap();
        assert!(removed);
        assert!(!root.join(".zg").exists());
    }

    #[test]
    fn delete_index_is_a_noop_when_cache_is_missing() {
        let root = temp_dir("delete-index-missing");

        let removed = delete_index(&root).unwrap();
        assert!(!removed);
    }

    #[test]
    fn file_scope_semantic_search_stays_on_the_requested_file() {
        let root = temp_dir("file-scope");
        fs::write(
            root.join("alpha.md"),
            "sqlite vector adapter and semantic ranking",
        )
        .unwrap();
        fs::write(
            root.join("beta.md"),
            "sqlite vector adapter and semantic ranking with extra noise",
        )
        .unwrap();
        init_hybrid(&root);

        let hits = search_hybrid(&root, &root.join("alpha.md"), "semantic ranking", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel_path, "alpha.md");
        assert!(hits[0].vector_score > 0.0);
    }

    #[test]
    fn directory_scope_semantic_search_filters_outside_paths() {
        let root = temp_dir("directory-scope");
        let notes = root.join("notes");
        let other = root.join("other");
        fs::create_dir_all(&notes).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(
            notes.join("alpha.md"),
            "sqlite vector adapter for scoped semantic search",
        )
        .unwrap();
        fs::write(
            other.join("beta.md"),
            "sqlite vector adapter for scoped semantic search",
        )
        .unwrap();
        init_hybrid(&root);

        let hits = search_hybrid(&root, &notes, "scoped semantic search", 10).unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|hit| hit.rel_path.starts_with("notes/")));
        assert!(hits.iter().any(|hit| hit.vector_score > 0.0));
    }

    #[test]
    fn nested_roots_use_nearest_ancestor_for_search() {
        let root = temp_dir("nested");
        let nested = root.join("journal");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("root.md"), "root entry").unwrap();
        fs::write(nested.join("today.md"), "today sqlite entry").unwrap();
        init_index(&root).unwrap();
        init_hybrid(&nested);

        let active_root = reconcile_covering_roots(&nested).unwrap().unwrap();
        assert_eq!(active_root, nested);
    }

    #[test]
    fn require_index_root_for_search_reuses_nearest_ancestor() {
        let root = temp_dir("ancestor-root");
        let child = root.join("notes/daily");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(root.join(".zg")).unwrap();
        fs::write(root.join(".zg/index.db"), "").unwrap();

        let index_root = require_index_root_for_search(&child).unwrap();
        assert_eq!(index_root, root);
        assert!(!child.join(".zg/index.db").exists());
    }

    #[test]
    fn require_index_root_for_search_errors_without_ancestor_index() {
        let root = temp_dir("file-root");
        let file = root.join("note.md");
        fs::write(&file, "").unwrap();

        let error = require_index_root_for_search(&file).unwrap_err();
        assert!(error.to_string().contains("no ancestor .zg index found"));
        assert!(!root.join(".zg/index.db").exists());
    }

    #[test]
    fn collect_candidate_files_uses_suffix_whitelist() {
        let root = temp_dir("whitelist");
        fs::write(root.join("keep.md"), "hello").unwrap();
        fs::write(root.join("skip.bin"), "hello").unwrap();

        let files = collect_candidate_files(&root).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("keep.md"));
    }

    #[test]
    fn status_marks_vector_backend_unready_when_vec_index_drifts() {
        let root = temp_dir("vector-ready");
        fs::write(root.join("alpha.md"), "sqlite :: vector adapter").unwrap();
        init_hybrid(&root);

        let conn = super::db::open_existing_db(&root).unwrap();
        conn.execute("DELETE FROM vec_index", []).unwrap();

        let status = load_status(&root).unwrap();
        assert!(!status.vector_ready);
        assert!(status.fts_ready);
    }

    #[test]
    fn rebuild_refreshes_schema_version_after_hash_upgrade() {
        let root = temp_dir("schema-upgrade");
        fs::write(root.join("alpha.md"), "sqlite :: vector adapter").unwrap();
        init_hybrid(&root);

        let conn = super::db::open_existing_db(&root).unwrap();
        conn.execute(
            "UPDATE settings SET value = '4' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();

        rebuild_index(&root).unwrap();
        let status = load_status(&root).unwrap();
        assert!(status.indexed);
        assert!(status.vector_ready);
    }
}
