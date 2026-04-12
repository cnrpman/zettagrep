use std::path::Path;

use crate::index::RebuildStats;

pub const INDEX_SCHEMA_VERSION_MISMATCH: &str = "index schema/version mismatch";

pub fn is_schema_version_mismatch(message: &str) -> bool {
    message == INDEX_SCHEMA_VERSION_MISMATCH
}

pub fn initialized_index(root: &Path, stats: &RebuildStats) -> String {
    format!(
        "initialized {} (.zg/, SQLite, lazy-first index) [{} indexed / {} scanned / {} chunks]",
        root.display(),
        stats.indexed_files,
        stats.scanned_files,
        stats.chunks_indexed,
    )
}

pub fn rebuilt_index(root: &Path, stats: &RebuildStats) -> String {
    format!(
        "rebuilt {} [{} indexed / {} scanned]",
        root.display(),
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

pub fn implicit_init_note(root: &Path, stats: &RebuildStats) -> String {
    format!(
        "note: no ancestor .zg index found; initialized local cache at {} for this search ({} files / {} chunks)",
        root.display(),
        stats.indexed_files,
        stats.chunks_indexed,
    )
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

pub fn protected_root_refusal(root: &Path, reason: &str) -> String {
    format!(
        "zg: refusing to auto-create index at {} ({})\n    run `zg index init \"{}\"` to confirm, or narrow the scope",
        root.display(),
        reason,
        root.display(),
    )
}

pub fn threshold_refusal(root: &Path, files: usize, total_size_bytes: u64) -> String {
    let total_mb = total_size_bytes.div_ceil(1024 * 1024);
    format!(
        "zg: refusing to auto-create index at {} ({} files, ~{} MB)\n    run `zg index init \"{}\"` to confirm, or narrow the scope",
        root.display(),
        files,
        total_mb,
        root.display(),
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
