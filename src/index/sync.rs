use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::ZgResult;
use crate::messages;
use crate::paths;

use super::db::{
    create_schema, delete_by_rel_path, ensure_index_root, gc_unreferenced_shared_chunks,
    load_file_rows_for_scope, load_index_level, load_state, load_state_snapshot_status, mark_dirty,
    open_existing_db, open_or_create_db, prepare_missing_shared_chunk_vectors, reset_schema,
    seed_defaults, set_dirty_state, set_index_level, status_for_index_root, upsert_document,
    validate_schema, with_write_transaction_retry, write_state_snapshot,
};
use super::files::{
    collect_candidate_files, collect_scope_candidates, estimate_indexable_chunks,
    load_indexable_documents,
};
use super::types::{
    DEFAULT_INDEX_LEVEL, FTS_PROMPT_MAX_CHUNKS, IndexLevel, IndexStatus, IndexedDocument,
    InitPreflight, RebuildStats, StateRow, SyncStats, VECTOR_PROMPT_MAX_CHUNKS,
};
use super::util::{
    ancestor_index_root, descendant_index_root, modified_unix_ms, now_unix_ms, relative_path_string,
};

pub fn init_index(root: &Path) -> ZgResult<RebuildStats> {
    init_index_with_level(root, DEFAULT_INDEX_LEVEL)
}

pub fn preflight_init(root: &Path, index_level: IndexLevel) -> ZgResult<InitPreflight> {
    let root = paths::resolve_existing_dir(root)?;
    let estimate = estimate_indexable_chunks(&root)?;
    let recommended_chunk_limit = index_level.recommended_chunk_limit();
    let force_threshold = index_level.init_force_chunk_limit();

    Ok(InitPreflight {
        estimated_chunks: estimate.chunk_count,
        recommended_chunk_limit,
        force_threshold,
        requires_force: estimate.chunk_count > force_threshold,
    })
}

pub fn init_index_with_level(root: &Path, index_level: IndexLevel) -> ZgResult<RebuildStats> {
    let root = paths::resolve_existing_dir(root)?;
    paths::ensure_hidden_dir(&root)?;

    let conn = open_or_create_db(&root)?;
    create_schema(&conn)?;
    seed_defaults(&conn)?;
    set_index_level(&conn, index_level)?;
    write_state_snapshot(&root, &status_for_index_root(&root)?)?;

    rebuild_index_with_level(&root, Some(index_level))
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

pub fn require_index_root_for_search(scope: &Path) -> ZgResult<PathBuf> {
    let scope = paths::resolve_existing_path(scope)?;
    if let Some(root) = paths::find_index_root(&scope) {
        return Ok(root);
    }

    let root = if scope.is_dir() {
        scope
    } else {
        scope
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| crate::other("cannot determine directory index root for search"))?
    };
    let estimate = estimate_indexable_chunks(&root)?;
    Err(crate::other(messages::explicit_index_required_error(
        &root,
        estimate.chunk_count,
        estimate.chunk_count <= FTS_PROMPT_MAX_CHUNKS,
        estimate.chunk_count <= VECTOR_PROMPT_MAX_CHUNKS,
    )))
}

pub fn rebuild_index(root: &Path) -> ZgResult<RebuildStats> {
    rebuild_index_with_level(root, None)
}

pub fn rebuild_index_with_level(
    root: &Path,
    index_level_override: Option<IndexLevel>,
) -> ZgResult<RebuildStats> {
    let root = paths::resolve_existing_dir(root)?;
    ensure_index_root(&root)?;

    let conn = open_or_create_db(&root)?;
    let retained_index_level = if index_level_override.is_none() {
        load_index_level(&conn).ok()
    } else {
        None
    };
    if validate_schema(&conn).is_err() {
        reset_schema(&conn)?;
    }
    create_schema(&conn)?;
    seed_defaults(&conn)?;
    if let Some(index_level) = index_level_override.or(retained_index_level) {
        set_index_level(&conn, index_level)?;
    }
    let index_level = load_index_level(&conn)?;

    let started_at = now_unix_ms();
    let candidate_files = collect_candidate_files(&root)?;
    let loaded_documents = load_indexable_documents(&candidate_files)?;
    let mut pending_upserts = Vec::new();
    let mut indexed_files = 0usize;
    let mut chunks_indexed = 0usize;
    let mut warnings = Vec::new();

    for (path, document) in candidate_files.iter().zip(loaded_documents.into_iter()) {
        match document {
            Some(document) => {
                let rel_path = relative_path_string(&root, path)?;
                chunks_indexed += document.chunks.len();
                pending_upserts.push(PendingDocument { rel_path, document });
                indexed_files += 1;
            }
            None => warnings.push(format!("skipped {}", path.display())),
        }
    }

    // Hold the writer lock before any embedding work so concurrent zg processes do
    // not race to compute the same missing vectors for the same root.
    with_write_transaction_retry(&conn, &root, "rebuilding index at", |tx| {
        tx.execute("DELETE FROM fts_chunks", [])?;
        tx.execute("DELETE FROM chunk_refs", [])?;
        tx.execute("DELETE FROM files", [])?;
        tx.execute("DELETE FROM vec_index", [])?;
        tx.execute("DELETE FROM vec_chunks", [])?;
        tx.execute("DELETE FROM shared_chunks", [])?;
        let prepared_vectors = prepare_missing_shared_chunk_vectors(
            tx,
            &pending_upserts
                .iter()
                .map(|pending| &pending.document)
                .collect::<Vec<_>>(),
            index_level.vectors_enabled(),
        )?;

        for pending in &pending_upserts {
            upsert_document(
                tx,
                &pending.rel_path,
                &pending.document,
                Some(&prepared_vectors),
                index_level.vectors_enabled(),
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
            tx,
            !warnings.is_empty(),
            warnings.first().map(String::as_str),
            Some(now_unix_ms()),
        )?;
        Ok(())
    })?;

    write_state_snapshot(&root, &status_for_index_root(&root)?)?;

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
                let mut status = load_state_snapshot_status(&requested_path, Some(root.clone()));
                status.dirty = true;
                status.dirty_reason = Some(dirty_reason);
                Ok(status)
            }
        },
        None => Ok(load_state_snapshot_status(&requested_path, None)),
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
    let index_level = load_index_level(&conn)?;
    let current_state = load_state(&conn)?.unwrap_or(StateRow {
        dirty: false,
        dirty_reason: None,
        last_sync_unix_ms: None,
    });
    let candidate_files = collect_scope_candidates(&root, &scope)?;
    let mut dirty_paths = Vec::new();
    let mut dirty_rel_paths = Vec::new();
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

        dirty_rel_paths.push(rel_path);
        dirty_paths.push(path);
    }

    let loaded_documents = load_indexable_documents(&dirty_paths)?;
    for ((rel_path, path), document) in dirty_rel_paths
        .into_iter()
        .zip(dirty_paths.into_iter())
        .zip(loaded_documents.into_iter())
    {
        match document {
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
        // Take the writer lock before embedding so a second zg process waits and
        // then reuses shared vectors written by the first writer instead of
        // recomputing them eagerly.
        with_write_transaction_retry(&conn, &root, "reconciling index scope at", |tx| {
            let prepared_vectors = prepare_missing_shared_chunk_vectors(
                tx,
                &pending_upserts
                    .iter()
                    .map(|pending| &pending.document)
                    .collect::<Vec<_>>(),
                index_level.vectors_enabled(),
            )?;
            for pending in &pending_upserts {
                upsert_document(
                    tx,
                    &pending.rel_path,
                    &pending.document,
                    Some(&prepared_vectors),
                    index_level.vectors_enabled(),
                )?;
            }
            for rel_path in &pending_deletes {
                delete_by_rel_path(tx, rel_path)?;
            }
            if !pending_upserts.is_empty() || !pending_deletes.is_empty() {
                gc_unreferenced_shared_chunks(tx)?;
            }

            set_dirty_state(
                tx,
                desired_dirty,
                desired_reason.as_deref(),
                Some(now_unix_ms()),
            )?;
            Ok(())
        })?;
    }

    write_state_snapshot(&root, &status_for_index_root(&root)?)?;
    Ok(stats)
}

struct PendingDocument {
    rel_path: String,
    document: IndexedDocument,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::require_index_root_for_search;
    use crate::messages;

    #[test]
    fn missing_index_error_is_actionable() {
        let root = PathBuf::from("/tmp/zg-project");
        let rendered = messages::explicit_index_required_error(&root, 64, true, true);
        assert!(rendered.contains("zg: no ancestor .zg index found"));
        assert!(rendered.contains("estimated chunks: 64"));
        assert!(rendered.contains("quick path: `zg index init --level fts"));
        assert!(rendered.contains("semantic path: `zg index init --level fts+vector"));
    }

    #[test]
    fn schema_mismatch_messages_include_rebuild_instruction() {
        let root = PathBuf::from("/tmp/zg-root");
        assert!(messages::schema_rebuild_required_error(&root).contains("zg index rebuild"));
        assert!(messages::schema_rebuild_dirty_reason(&root).contains("zg index rebuild"));
    }

    #[test]
    fn require_index_root_for_search_reports_missing_root() {
        let root = std::env::temp_dir().join("zg-missing-index-root");
        std::fs::create_dir_all(&root).unwrap();

        let error = require_index_root_for_search(&root).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("zg: no ancestor .zg index found")
        );
    }
}
