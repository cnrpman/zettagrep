use std::path::Path;

use crate::index::{FTS_PROMPT_MAX_CHUNKS, IndexLevel, RebuildStats, VECTOR_PROMPT_MAX_CHUNKS};

pub const INDEX_SCHEMA_VERSION_MISMATCH: &str = "index schema/version mismatch";

pub fn is_schema_version_mismatch(message: &str) -> bool {
    message == INDEX_SCHEMA_VERSION_MISMATCH
}

pub fn initialized_index(root: &Path, index_level: IndexLevel, stats: &RebuildStats) -> String {
    format!(
        "initialized {} (.zg/, SQLite, level={}) [{} indexed / {} scanned / {} chunks]",
        root.display(),
        index_level,
        stats.indexed_files,
        stats.scanned_files,
        stats.chunks_indexed,
    )
}

pub fn rebuilt_index(root: &Path, index_level: IndexLevel, stats: &RebuildStats) -> String {
    format!(
        "rebuilt {} (level={}) [{} indexed / {} scanned]",
        root.display(),
        index_level,
        stats.indexed_files,
        stats.scanned_files
    )
}

pub fn deleted_local_cache(root: &Path) -> String {
    format!("deleted local cache at {}", root.join(".zg").display())
}

pub fn no_local_cache(root: &Path) -> String {
    format!("no local cache at {}", root.join(".zg").display())
}

pub fn explicit_index_required_error(
    root: &Path,
    estimated_chunks: usize,
    suggest_fts: bool,
    suggest_vector: bool,
) -> String {
    let mut lines = vec![format!(
        "zg: no ancestor .zg index found for {}",
        root.display()
    )];
    lines.push(format!("    estimated chunks: {}", estimated_chunks));

    if suggest_fts {
        lines.push(format!(
            "    quick path: `zg index init --level fts \"{}\"`  (fast lexical index; tuned for keyword and identifier search)",
            root.display(),
        ));
    }
    if suggest_vector {
        lines.push(format!(
            "    semantic path: `zg index init --level fts+vector \"{}\"`  (stronger natural-language recall; slower to build and query)",
            root.display(),
        ));
    }
    if !suggest_fts && !suggest_vector {
        lines.push(format!(
            "    run `zg index init \"{}\"` to build the default fts index first",
            root.display(),
        ));
    }

    lines.join("\n")
}

pub fn index_level_follow_up(
    root: &Path,
    index_level: IndexLevel,
    chunk_count: usize,
) -> Option<String> {
    match index_level {
        IndexLevel::Fts if chunk_count <= VECTOR_PROMPT_MAX_CHUNKS => Some(format!(
            "next: `zg index rebuild --level fts+vector \"{}\"` enables semantic recall for natural-language queries",
            root.display()
        )),
        IndexLevel::Fts => None,
        IndexLevel::FtsVector => None,
    }
}

pub fn vector_index_start_notice(root: &Path, operation: &str) -> String {
    format!(
        "note: starting `fts+vector` index {operation} for {}; this may take a while, especially on the first run while embeddings are prepared",
        root.display(),
    )
}

pub fn status_level_hint(root: &Path, index_level: IndexLevel, chunk_count: u64) -> Option<String> {
    match index_level {
        IndexLevel::Fts if (chunk_count as usize) <= VECTOR_PROMPT_MAX_CHUNKS => Some(format!(
            "upgrade hint: `zg index rebuild --level fts+vector \"{}\"`",
            root.display()
        )),
        IndexLevel::FtsVector if (chunk_count as usize) <= FTS_PROMPT_MAX_CHUNKS => Some(format!(
            "downgrade hint: `zg index rebuild --level fts \"{}\"`",
            root.display()
        )),
        _ => None,
    }
}

pub fn cache_delete_note(root: &Path) -> String {
    format!(
        "note: this cache is optional; delete it later with `zg index delete \"{}\"`",
        root.display()
    )
}

pub fn overlap_parent_note(ancestor: &Path) -> String {
    format!(
        "note: this directory is also covered by parent index {} ; adding a local index costs extra disk and duplicate updates, and under the current brute-force search path it is not a recall fix",
        ancestor.display()
    )
}

pub fn overlap_child_note(descendant: &Path) -> String {
    format!(
        "note: this directory already contains a nested index at {} ; overlapping indexes cost extra disk and duplicate updates, and under the current brute-force search path they are not needed for recall",
        descendant.display()
    )
}

pub fn schema_rebuild_required_error(root: &Path) -> String {
    format!(
        "zg: index schema/version mismatch at {}\n    run `zg index rebuild \"{}\"` to rebuild this index",
        root.display(),
        root.display(),
    )
}

pub fn schema_rebuild_dirty_reason(root: &Path) -> String {
    format!(
        "index schema/version mismatch; run `zg index rebuild \"{}\"`",
        root.display(),
    )
}
