mod db;
mod files;
mod hybrid;
mod sync;
mod types;
mod util;

pub use files::{collect_candidate_files, collect_scan_files};
pub use hybrid::search_hybrid;
pub use sync::{
    best_effort_overlap_note, init_index, load_status, rebuild_index, reconcile_covering_roots,
};
pub use types::{IndexStatus, RebuildStats, SearchHit};

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

    #[test]
    fn rebuild_creates_searchable_index() {
        let root = temp_dir("rebuild");
        fs::write(root.join("alpha.md"), "sqlite :: vector adapter").unwrap();
        fs::write(root.join("beta.md"), "rust search tooling").unwrap();

        let stats = init_index(&root).unwrap();
        assert_eq!(stats.indexed_files, 2);

        let hits = search_hybrid(&root, &root, "sqlite adapter", 10).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].rel_path, "alpha.md");
        assert!(hits.iter().any(|hit| hit.rel_path == "alpha.md"));
    }

    #[test]
    fn reconcile_refreshes_modified_scope_on_search_path() {
        let root = temp_dir("reconcile");
        let nested = root.join("notes");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("alpha.md");
        fs::write(&file, "first line").unwrap();
        init_index(&root).unwrap();

        fs::write(&file, "updated sqlite recall").unwrap();
        let active_root = reconcile_covering_roots(&nested).unwrap().unwrap();
        assert_eq!(active_root, root);

        let hits = search_hybrid(&root, &nested, "sqlite recall", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel_path, "notes/alpha.md");
    }

    #[test]
    fn nested_roots_use_nearest_ancestor_for_search() {
        let root = temp_dir("nested");
        let nested = root.join("journal");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("root.md"), "root entry").unwrap();
        fs::write(nested.join("today.md"), "today sqlite entry").unwrap();
        init_index(&root).unwrap();
        init_index(&nested).unwrap();

        let active_root = reconcile_covering_roots(&nested).unwrap().unwrap();
        assert_eq!(active_root, nested);
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
}
