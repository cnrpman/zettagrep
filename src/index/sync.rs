use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::ZgResult;
use crate::messages;
use crate::paths;

use super::db::{
    create_schema, delete_by_rel_path, ensure_index_root, gc_unreferenced_shared_chunks,
    load_file_rows_for_scope, load_state, load_state_mirror_status, mark_dirty, open_existing_db,
    open_or_create_db, prepare_shared_chunk_vectors, reset_schema, seed_defaults, set_dirty_state,
    status_for_index_root, upsert_document, validate_schema, write_state_mirror,
};
use super::files::{
    collect_candidate_files, collect_scope_candidates, load_indexable_document,
    scan_candidate_files_until,
};
use super::types::{IndexStatus, IndexedDocument, RebuildStats, StateRow, SyncStats};
use super::util::{
    ancestor_index_root, descendant_index_root, modified_unix_ms, now_unix_ms, relative_path_string,
};

const IMPLICIT_INIT_MAX_FILES: usize = 2000;
const IMPLICIT_INIT_MAX_TOTAL_BYTES: u64 = 200 * 1024 * 1024;
const PROTECTED_HOME_DIR_NAMES: &[&str] = &[
    "Desktop",
    "Documents",
    "Downloads",
    "Music",
    "Pictures",
    "Public",
    "Templates",
    "Videos",
    "Movies",
];

pub fn init_index(root: &Path) -> ZgResult<RebuildStats> {
    let root = paths::resolve_existing_dir(root)?;
    paths::ensure_hidden_dir(&root)?;

    let conn = open_or_create_db(&root)?;
    create_schema(&conn)?;
    seed_defaults(&conn)?;
    write_state_mirror(&root, &status_for_index_root(&root)?)?;

    rebuild_index(&root)
}

pub fn delete_index(root: &Path) -> ZgResult<bool> {
    let root = paths::resolve_existing_dir(root)?;
    let hidden = paths::hidden_dir(&root);
    if !hidden.exists() {
        return Ok(false);
    }
    if !fs::metadata(&hidden)?.is_dir() {
        return Err(crate::other(format!(
            "expected cache directory at {}",
            hidden.display()
        )));
    }

    fs::remove_dir_all(hidden)?;
    Ok(true)
}

pub fn ensure_index_root_for_search(scope: &Path) -> ZgResult<(PathBuf, Option<RebuildStats>)> {
    let scope = paths::resolve_existing_path(scope)?;
    if let Some(root) = paths::find_index_root(&scope) {
        return Ok((root, None));
    }

    let root = if scope.is_dir() {
        scope
    } else {
        scope.parent().map(Path::to_path_buf).ok_or_else(|| {
            crate::other("cannot determine directory index root for search-triggered creation")
        })?
    };

    ensure_safe_implicit_init_root(&root)?;
    let stats = init_index(&root)?;
    Ok((root, Some(stats)))
}

pub fn rebuild_index(root: &Path) -> ZgResult<RebuildStats> {
    let root = paths::resolve_existing_dir(root)?;
    ensure_index_root(&root)?;

    let conn = open_or_create_db(&root)?;
    if validate_schema(&conn).is_err() {
        reset_schema(&conn)?;
    }
    create_schema(&conn)?;
    seed_defaults(&conn)?;

    let started_at = now_unix_ms();
    let candidate_files = collect_candidate_files(&root)?;
    let mut pending_upserts = Vec::new();
    let mut indexed_files = 0usize;
    let mut chunks_indexed = 0usize;
    let mut warnings = Vec::new();

    for path in &candidate_files {
        match load_indexable_document(path)? {
            Some(document) => {
                let rel_path = relative_path_string(&root, path)?;
                chunks_indexed += document.chunks.len();
                pending_upserts.push(PendingDocument { rel_path, document });
                indexed_files += 1;
            }
            None => warnings.push(format!("skipped {}", path.display())),
        }
    }

    let prepared_vectors = prepare_shared_chunk_vectors(
        &pending_upserts
            .iter()
            .map(|pending| &pending.document)
            .collect::<Vec<_>>(),
    )?;
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM fts_chunks", [])?;
    tx.execute("DELETE FROM chunk_refs", [])?;
    tx.execute("DELETE FROM files", [])?;
    tx.execute("DELETE FROM vec_index", [])?;
    tx.execute("DELETE FROM vec_chunks", [])?;
    tx.execute("DELETE FROM shared_chunks", [])?;

    for pending in &pending_upserts {
        upsert_document(
            &tx,
            &pending.rel_path,
            &pending.document,
            Some(&prepared_vectors),
        )?;
    }

    tx.execute(
        "INSERT INTO index_runs (
            started_at_unix_ms,
            finished_at_unix_ms,
            status,
            scanned_files,
            indexed_files,
            chunks_indexed,
            error
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            started_at as i64,
            now_unix_ms() as i64,
            if warnings.is_empty() {
                "completed"
            } else {
                "completed_with_warnings"
            },
            candidate_files.len() as i64,
            indexed_files as i64,
            chunks_indexed as i64,
            warnings.first().cloned(),
        ],
    )?;
    set_dirty_state(
        &tx,
        !warnings.is_empty(),
        warnings.first().map(String::as_str),
        Some(now_unix_ms()),
    )?;
    tx.commit()?;

    write_state_mirror(&root, &status_for_index_root(&root)?)?;

    Ok(RebuildStats {
        scanned_files: candidate_files.len(),
        indexed_files,
        chunks_indexed,
    })
}

pub fn reconcile_covering_roots(scope: &Path) -> ZgResult<Option<PathBuf>> {
    let scope = paths::resolve_existing_path(scope)?;
    let roots = paths::covering_index_roots(&scope);
    let active_root = roots.first().cloned();

    for (index, root) in roots.iter().enumerate() {
        if let Err(error) = reconcile_scope_for_root(root, &scope) {
            let rendered = if messages::is_schema_version_mismatch(&error.to_string()) {
                messages::schema_rebuild_dirty_reason(root)
            } else {
                error.to_string()
            };
            mark_dirty(root, &rendered)?;
            if index == 0 {
                if messages::is_schema_version_mismatch(&error.to_string()) {
                    return Err(crate::other(messages::schema_rebuild_required_error(root)));
                }
                return Err(error);
            }
        }
    }

    Ok(active_root)
}

pub fn load_status(path: &Path) -> ZgResult<IndexStatus> {
    let requested_path = paths::resolve_existing_path(path)?;
    let index_root = paths::find_index_root(&requested_path);
    match index_root {
        Some(root) => match status_for_index_root(&root) {
            Ok(mut status) => {
                status.requested_path = requested_path;
                Ok(status)
            }
            Err(error) => {
                let dirty_reason = if messages::is_schema_version_mismatch(&error.to_string()) {
                    messages::schema_rebuild_dirty_reason(&root)
                } else {
                    error.to_string()
                };
                mark_dirty(&root, &dirty_reason)?;
                let mut status = load_state_mirror_status(&requested_path, Some(root.clone()));
                status.dirty = true;
                status.dirty_reason = Some(dirty_reason);
                Ok(status)
            }
        },
        None => Ok(load_state_mirror_status(&requested_path, None)),
    }
}

pub fn best_effort_overlap_note(root: &Path) -> ZgResult<Option<String>> {
    let root = paths::resolve_existing_dir(root)?;
    if let Some(ancestor) = ancestor_index_root(&root) {
        return Ok(Some(messages::overlap_parent_note(&ancestor)));
    }

    if let Some(descendant) = descendant_index_root(&root)? {
        return Ok(Some(messages::overlap_child_note(&descendant)));
    }

    Ok(None)
}

fn reconcile_scope_for_root(root: &Path, scope: &Path) -> ZgResult<SyncStats> {
    let root = paths::resolve_existing_dir(root)?;
    let scope = paths::resolve_existing_path(scope)?;
    ensure_index_root(&root)?;
    let conn = open_existing_db(&root)?;
    validate_schema(&conn)?;

    let existing = load_file_rows_for_scope(&conn, &root, &scope)?;
    let current_state = load_state(&conn)?.unwrap_or(StateRow {
        dirty: false,
        dirty_reason: None,
        last_sync_unix_ms: None,
    });
    let candidate_files = collect_scope_candidates(&root, &scope)?;
    let mut seen = HashSet::new();
    let mut pending_upserts = Vec::new();
    let mut pending_deletes = Vec::new();
    let mut stats = SyncStats {
        indexed_files: 0,
        chunks_indexed: 0,
        warnings: Vec::new(),
    };

    for path in candidate_files {
        let rel_path = relative_path_string(&root, &path)?;
        seen.insert(rel_path.clone());

        let metadata = fs::metadata(&path)?;
        let modified_unix_ms = modified_unix_ms(&metadata)?;
        let size_bytes = metadata.len();
        let row = existing.get(&rel_path);
        if row.is_some_and(|value| {
            value.size_bytes == size_bytes && value.modified_unix_ms == modified_unix_ms
        }) {
            continue;
        }

        match load_indexable_document(&path)? {
            Some(document) => {
                stats.indexed_files += 1;
                stats.chunks_indexed += document.chunks.len();
                pending_upserts.push(PendingDocument { rel_path, document });
            }
            None => {
                pending_deletes.push(rel_path.clone());
                stats.warnings.push(format!(
                    "skipped unreadable or disallowed file {}",
                    path.display()
                ));
            }
        }
    }

    for rel_path in existing.keys() {
        if !seen.contains(rel_path) {
            pending_deletes.push(rel_path.clone());
        }
    }

    let desired_dirty = !stats.warnings.is_empty();
    let desired_reason = stats.warnings.first().cloned();
    let needs_state_write =
        current_state.dirty != desired_dirty || current_state.dirty_reason != desired_reason;

    if !pending_upserts.is_empty() || !pending_deletes.is_empty() || needs_state_write {
        let prepared_vectors = prepare_shared_chunk_vectors(
            &pending_upserts
                .iter()
                .map(|pending| &pending.document)
                .collect::<Vec<_>>(),
        )?;
        let tx = conn.unchecked_transaction()?;
        for pending in &pending_upserts {
            upsert_document(
                &tx,
                &pending.rel_path,
                &pending.document,
                Some(&prepared_vectors),
            )?;
        }
        for rel_path in &pending_deletes {
            delete_by_rel_path(&tx, rel_path)?;
        }
        if !pending_upserts.is_empty() || !pending_deletes.is_empty() {
            gc_unreferenced_shared_chunks(&tx)?;
        }

        set_dirty_state(
            &tx,
            desired_dirty,
            desired_reason.as_deref(),
            Some(now_unix_ms()),
        )?;
        tx.commit()?;
    }

    write_state_mirror(&root, &status_for_index_root(&root)?)?;
    Ok(stats)
}

struct PendingDocument {
    rel_path: String,
    document: IndexedDocument,
}

fn ensure_safe_implicit_init_root(root: &Path) -> ZgResult<()> {
    if let Some(reason) = protected_implicit_root_reason(root, home_dir()) {
        return Err(crate::other(format_protected_root_refusal(root, reason)));
    }

    let summary =
        scan_candidate_files_until(root, IMPLICIT_INIT_MAX_FILES, IMPLICIT_INIT_MAX_TOTAL_BYTES)?;
    if summary.limit_tripped {
        return Err(crate::other(format_threshold_refusal(
            root,
            summary.candidate_files,
            summary.total_size_bytes,
        )));
    }

    Ok(())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn protected_implicit_root_reason(root: &Path, home: Option<PathBuf>) -> Option<&'static str> {
    if root == Path::new("/") {
        return Some("filesystem root");
    }

    let home = home?;
    if root == home {
        return Some("home directory");
    }
    if PROTECTED_HOME_DIR_NAMES
        .iter()
        .map(|name| home.join(name))
        .any(|dir| dir == root)
    {
        return Some("user content directory");
    }

    None
}

fn format_threshold_refusal(root: &Path, files: usize, total_size_bytes: u64) -> String {
    messages::threshold_refusal(root, files, total_size_bytes)
}

fn format_protected_root_refusal(root: &Path, reason: &str) -> String {
    messages::protected_root_refusal(root, reason)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{format_protected_root_refusal, protected_implicit_root_reason};
    use crate::messages;

    #[test]
    fn protected_root_reason_matches_home_and_documents() {
        let home = PathBuf::from("/tmp/zg-home");
        assert_eq!(
            protected_implicit_root_reason(&home, Some(home.clone())),
            Some("home directory")
        );
        assert_eq!(
            protected_implicit_root_reason(&home.join("Documents"), Some(home.clone())),
            Some("user content directory")
        );
        assert_eq!(
            protected_implicit_root_reason(&home.join("project"), Some(home)),
            None
        );
    }

    #[test]
    fn protected_root_refusal_is_actionable() {
        let root = PathBuf::from("/tmp/zg-home/Documents");
        let rendered = format_protected_root_refusal(&root, "user content directory");
        assert!(rendered.contains("zg: refusing to auto-create index"));
        assert!(rendered.contains("zg index init"));
        assert!(rendered.contains("user content directory"));
    }

    #[test]
    fn schema_mismatch_messages_include_rebuild_instruction() {
        let root = PathBuf::from("/tmp/zg-root");
        assert!(messages::schema_rebuild_required_error(&root).contains("zg index rebuild"));
        assert!(messages::schema_rebuild_dirty_reason(&root).contains("zg index rebuild"));
    }
}
