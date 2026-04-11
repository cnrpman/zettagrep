use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::ZgResult;
use crate::paths;

use super::db::{
    create_schema, delete_by_rel_path, ensure_index_root, load_file_rows_for_scope,
    load_state_mirror_status, mark_dirty, open_existing_db, open_or_create_db, seed_defaults,
    set_dirty_state, status_for_index_root, upsert_document, validate_schema, write_state_mirror,
};
use super::files::{collect_candidate_files, collect_scope_candidates, load_indexable_document};
use super::types::{IndexStatus, RebuildStats, SyncStats};
use super::util::{
    ancestor_index_root, descendant_index_root, modified_unix_ms, now_unix_ms, relative_path_string,
};

pub fn init_index(root: &Path) -> ZgResult<RebuildStats> {
    let root = paths::resolve_existing_dir(root)?;
    paths::ensure_hidden_dir(&root)?;

    let conn = open_or_create_db(&root)?;
    create_schema(&conn)?;
    seed_defaults(&conn)?;
    write_state_mirror(&root, &status_for_index_root(&root)?)?;

    rebuild_index(&root)
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

    let stats = init_index(&root)?;
    Ok((root, Some(stats)))
}

pub fn rebuild_index(root: &Path) -> ZgResult<RebuildStats> {
    let root = paths::resolve_existing_dir(root)?;
    ensure_index_root(&root)?;

    let conn = open_or_create_db(&root)?;
    create_schema(&conn)?;
    seed_defaults(&conn)?;

    let started_at = now_unix_ms();
    let candidate_files = collect_candidate_files(&root)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM vec_chunks", [])?;
    tx.execute("DELETE FROM fts_chunks", [])?;
    tx.execute("DELETE FROM chunks", [])?;
    tx.execute("DELETE FROM files", [])?;

    let mut indexed_files = 0usize;
    let mut chunks_indexed = 0usize;
    let mut warnings = Vec::new();

    for path in &candidate_files {
        match load_indexable_document(path)? {
            Some(document) => {
                let rel_path = relative_path_string(&root, path)?;
                chunks_indexed += document.chunks.len();
                upsert_document(&tx, &rel_path, &document)?;
                indexed_files += 1;
            }
            None => warnings.push(format!("skipped {}", path.display())),
        }
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
            mark_dirty(root, &error.to_string())?;
            if index == 0 {
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
                mark_dirty(&root, &error.to_string())?;
                let mut status = load_state_mirror_status(&requested_path, Some(root.clone()));
                status.dirty = true;
                status.dirty_reason = Some(error.to_string());
                Ok(status)
            }
        },
        None => Ok(load_state_mirror_status(&requested_path, None)),
    }
}

pub fn best_effort_overlap_note(root: &Path) -> ZgResult<Option<String>> {
    let root = paths::resolve_existing_dir(root)?;
    if let Some(ancestor) = ancestor_index_root(&root) {
        return Ok(Some(format!(
            "note: this directory is also covered by parent index {} ; adding a local index trades disk and slower updates for tighter local recall",
            ancestor.display()
        )));
    }

    if let Some(descendant) = descendant_index_root(&root)? {
        return Ok(Some(format!(
            "note: this directory already contains a nested index at {} ; overlapping indexes cost extra disk and slower updates",
            descendant.display()
        )));
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
    let candidate_files = collect_scope_candidates(&root, &scope)?;
    let mut seen = HashSet::new();
    let mut stats = SyncStats {
        indexed_files: 0,
        chunks_indexed: 0,
        warnings: Vec::new(),
    };

    let tx = conn.unchecked_transaction()?;
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
                upsert_document(&tx, &rel_path, &document)?;
            }
            None => {
                delete_by_rel_path(&tx, &rel_path)?;
                stats.warnings.push(format!(
                    "skipped unreadable or disallowed file {}",
                    path.display()
                ));
            }
        }
    }

    for rel_path in existing.keys() {
        if !seen.contains(rel_path) {
            delete_by_rel_path(&tx, rel_path)?;
        }
    }

    set_dirty_state(
        &tx,
        !stats.warnings.is_empty(),
        stats.warnings.first().map(String::as_str),
        Some(now_unix_ms()),
    )?;
    tx.commit()?;

    write_state_mirror(&root, &status_for_index_root(&root)?)?;
    Ok(stats)
}
