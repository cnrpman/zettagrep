use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rusqlite::auto_extension::{RawAutoExtension, register_auto_extension};
use rusqlite::{Connection, OptionalExtension, params};
use sqlite_vec::sqlite3_vec_init;

use crate::paths;
use crate::walk::DEFAULT_WALK_POLICY;
use crate::{ZgResult, other};

use super::types::{
    DEFAULT_CHUNK_MARKER, DEFAULT_CHUNK_MODE, DEFAULT_SCOPE_POLICY, DEFAULT_VECTOR_PROVIDER,
    FileRecord, IndexStatus, IndexedDocument, SCHEMA_VERSION, ScopeKind, StateMirror, StateRow,
    VECTOR_DIMENSIONS,
};
use super::util::{now_unix_ms, scope_kind};

static SQLITE_VEC_REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();

pub(crate) fn open_or_create_db(root: &Path) -> ZgResult<Connection> {
    ensure_sqlite_vec_registered()?;
    let db_path = paths::db_path(root);
    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(conn)
}

fn ensure_sqlite_vec_registered() -> ZgResult<()> {
    SQLITE_VEC_REGISTERED
        .get_or_init(|| unsafe {
            let sqlite_vec_init: RawAutoExtension =
                std::mem::transmute::<*const (), RawAutoExtension>(sqlite3_vec_init as *const ());
            register_auto_extension(sqlite_vec_init)
                .map_err(|error| format!("failed to register sqlite-vec auto extension: {error}"))
        })
        .clone()
        .map_err(crate::other)
}

pub(crate) fn open_existing_db(root: &Path) -> ZgResult<Connection> {
    if !paths::db_path(root).exists() {
        return Err(other(format!(
            "{} is not an indexed zg root",
            root.display()
        )));
    }
    open_or_create_db(root)
}

pub(crate) fn ensure_index_root(root: &Path) -> ZgResult<()> {
    if paths::is_indexed_root(root) {
        return Ok(());
    }
    Err(other(format!(
        "{} is not an indexed zg root",
        root.display()
    )))
}

pub(crate) fn create_schema(conn: &Connection) -> ZgResult<()> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            dirty INTEGER NOT NULL DEFAULT 0,
            dirty_reason TEXT,
            last_sync_unix_ms INTEGER
        );
        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY,
            rel_path TEXT NOT NULL UNIQUE,
            size_bytes INTEGER NOT NULL,
            modified_unix_ms INTEGER NOT NULL,
            content_hash TEXT NOT NULL,
            indexed_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS chunks (
            id INTEGER PRIMARY KEY,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            chunk_index INTEGER NOT NULL,
            line_start INTEGER NOT NULL,
            line_end INTEGER NOT NULL,
            raw_text TEXT NOT NULL,
            normalized_text TEXT NOT NULL,
            text_hash TEXT NOT NULL,
            UNIQUE(file_id, chunk_index)
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS fts_chunks USING fts5(
            rel_path UNINDEXED,
            normalized_text,
            tokenize = 'unicode61 remove_diacritics 2'
        );
        CREATE TABLE IF NOT EXISTS vec_chunks (
            chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
            provider TEXT NOT NULL,
            dims INTEGER NOT NULL,
            vector BLOB NOT NULL
                CHECK(typeof(vector) = 'blob' AND vec_length(vector) = dims)
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS vec_index USING vec0(
            chunk_id INTEGER PRIMARY KEY,
            embedding FLOAT[{VECTOR_DIMENSIONS}] distance_metric=cosine
        );
        CREATE TABLE IF NOT EXISTS index_runs (
            id INTEGER PRIMARY KEY,
            started_at_unix_ms INTEGER NOT NULL,
            finished_at_unix_ms INTEGER NOT NULL,
            status TEXT NOT NULL,
            scanned_files INTEGER NOT NULL,
            indexed_files INTEGER NOT NULL,
            chunks_indexed INTEGER NOT NULL,
            error TEXT
        );",
    ))?;
    Ok(())
}

pub(crate) fn validate_schema(conn: &Connection) -> ZgResult<()> {
    let required = [
        "settings",
        "state",
        "files",
        "chunks",
        "fts_chunks",
        "vec_chunks",
        "vec_index",
        "index_runs",
    ];
    for table in required {
        let exists = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)",
            [table],
            |row| row.get::<_, i64>(0),
        )?;
        if exists == 0 {
            return Err(other(format!(
                "index schema is missing required table `{table}`"
            )));
        }
    }

    let schema_version = load_setting(conn, "schema_version")?;
    if schema_version.as_deref() != Some(&SCHEMA_VERSION.to_string()) {
        return Err(other("index schema/version mismatch"));
    }

    Ok(())
}

pub(crate) fn seed_defaults(conn: &Connection) -> ZgResult<()> {
    let settings = [
        ("schema_version", SCHEMA_VERSION.to_string()),
        ("chunk_mode", DEFAULT_CHUNK_MODE.to_string()),
        ("chunk_marker", DEFAULT_CHUNK_MARKER.to_string()),
        ("scope_policy", DEFAULT_SCOPE_POLICY.to_string()),
        ("vector_provider", DEFAULT_VECTOR_PROVIDER.to_string()),
    ];

    for (key, value) in settings {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
    }
    conn.execute("INSERT OR IGNORE INTO state (id, dirty) VALUES (1, 0)", [])?;
    Ok(())
}

pub(crate) fn load_setting(conn: &Connection, key: &str) -> ZgResult<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()?)
}

pub(crate) fn load_state(conn: &Connection) -> ZgResult<Option<StateRow>> {
    Ok(conn
        .query_row(
            "SELECT dirty, dirty_reason, last_sync_unix_ms FROM state WHERE id = 1",
            [],
            |row| {
                Ok(StateRow {
                    dirty: row.get::<_, i64>(0)? != 0,
                    dirty_reason: row.get(1)?,
                    last_sync_unix_ms: row.get::<_, Option<i64>>(2)?.map(|value| value as u64),
                })
            },
        )
        .optional()?)
}

pub(crate) fn set_dirty_state(
    conn: &Connection,
    dirty: bool,
    dirty_reason: Option<&str>,
    last_sync_unix_ms: Option<u64>,
) -> ZgResult<()> {
    conn.execute(
        "INSERT INTO state (id, dirty, dirty_reason, last_sync_unix_ms)
         VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET
            dirty = excluded.dirty,
            dirty_reason = excluded.dirty_reason,
            last_sync_unix_ms = COALESCE(excluded.last_sync_unix_ms, state.last_sync_unix_ms)",
        params![
            dirty as i64,
            dirty_reason,
            last_sync_unix_ms.map(|value| value as i64)
        ],
    )?;
    Ok(())
}

pub(crate) fn mark_dirty(root: &Path, reason: &str) -> ZgResult<()> {
    if let Ok(conn) = open_or_create_db(root) {
        let _ = create_schema(&conn);
        let _ = seed_defaults(&conn);
        let _ = set_dirty_state(&conn, true, Some(reason), None);
    }

    let mirror = StateMirror {
        schema_version: SCHEMA_VERSION,
        index_root: root.display().to_string(),
        indexed: paths::is_indexed_root(root),
        chunk_mode: DEFAULT_CHUNK_MODE,
        chunk_marker: DEFAULT_CHUNK_MARKER,
        scope_policy: DEFAULT_SCOPE_POLICY,
        walk_policy: DEFAULT_WALK_POLICY,
        dirty: true,
        dirty_reason: Some(reason.to_string()),
        last_sync_unix_ms: None,
        file_count: 0,
        chunk_count: 0,
        fts_ready: false,
        vector_ready: false,
    };
    fs::write(
        paths::state_path(root),
        serde_json::to_string_pretty(&mirror)? + "\n",
    )?;
    Ok(())
}

pub(crate) fn write_state_mirror(root: &Path, status: &IndexStatus) -> ZgResult<()> {
    let mirror = StateMirror {
        schema_version: SCHEMA_VERSION,
        index_root: status
            .index_root
            .as_deref()
            .unwrap_or(root)
            .display()
            .to_string(),
        indexed: status.indexed,
        chunk_mode: DEFAULT_CHUNK_MODE,
        chunk_marker: DEFAULT_CHUNK_MARKER,
        scope_policy: DEFAULT_SCOPE_POLICY,
        walk_policy: DEFAULT_WALK_POLICY,
        dirty: status.dirty,
        dirty_reason: status.dirty_reason.clone(),
        last_sync_unix_ms: status.last_sync_unix_ms,
        file_count: status.file_count,
        chunk_count: status.chunk_count,
        fts_ready: status.fts_ready,
        vector_ready: status.vector_ready,
    };
    fs::write(
        paths::state_path(root),
        serde_json::to_string_pretty(&mirror)? + "\n",
    )?;
    Ok(())
}

pub(crate) fn upsert_document(
    conn: &Connection,
    rel_path: &str,
    document: &IndexedDocument,
) -> ZgResult<()> {
    delete_by_rel_path(conn, rel_path)?;

    conn.execute(
        "INSERT INTO files (
            rel_path,
            size_bytes,
            modified_unix_ms,
            content_hash,
            indexed_at_unix_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            rel_path,
            document.size_bytes as i64,
            document.modified_unix_ms as i64,
            document.content_hash,
            now_unix_ms() as i64
        ],
    )?;

    let file_id = conn.last_insert_rowid();
    for chunk in &document.chunks {
        conn.execute(
            "INSERT INTO chunks (
                file_id,
                chunk_index,
                line_start,
                line_end,
                raw_text,
                normalized_text,
                text_hash
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                file_id,
                chunk.chunk_index as i64,
                chunk.line_start as i64,
                chunk.line_end as i64,
                chunk.raw_text,
                chunk.normalized_text,
                chunk.text_hash,
            ],
        )?;
        let chunk_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO fts_chunks (rowid, rel_path, normalized_text) VALUES (?1, ?2, ?3)",
            params![chunk_id, rel_path, chunk.normalized_text],
        )?;
        conn.execute(
            "INSERT INTO vec_chunks (chunk_id, provider, dims, vector) VALUES (?1, ?2, ?3, ?4)",
            params![
                chunk_id,
                DEFAULT_VECTOR_PROVIDER,
                chunk.vector.len() as i64,
                super::hybrid::encode_vector(&chunk.vector)
            ],
        )?;
        conn.execute(
            "INSERT INTO vec_index (chunk_id, embedding) VALUES (?1, ?2)",
            params![chunk_id, super::hybrid::encode_vector(&chunk.vector)],
        )?;
    }

    Ok(())
}

pub(crate) fn delete_by_rel_path(conn: &Connection, rel_path: &str) -> ZgResult<()> {
    let file_id = conn
        .query_row(
            "SELECT id FROM files WHERE rel_path = ?1",
            [rel_path],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;

    let Some(file_id) = file_id else {
        return Ok(());
    };

    let mut stmt = conn.prepare("SELECT id FROM chunks WHERE file_id = ?1")?;
    let chunk_ids = stmt
        .query_map([file_id], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    for chunk_id in chunk_ids {
        conn.execute("DELETE FROM fts_chunks WHERE rowid = ?1", [chunk_id])?;
        conn.execute("DELETE FROM vec_index WHERE chunk_id = ?1", [chunk_id])?;
        conn.execute("DELETE FROM vec_chunks WHERE chunk_id = ?1", [chunk_id])?;
    }
    conn.execute("DELETE FROM chunks WHERE file_id = ?1", [file_id])?;
    conn.execute("DELETE FROM files WHERE id = ?1", [file_id])?;
    Ok(())
}

pub(crate) fn load_file_rows_for_scope(
    conn: &Connection,
    root: &Path,
    scope: &Path,
) -> ZgResult<HashMap<String, FileRecord>> {
    let rows = match scope_kind(root, scope)? {
        ScopeKind::Root => {
            let mut stmt =
                conn.prepare("SELECT rel_path, size_bytes, modified_unix_ms FROM files")?;
            stmt.query_map([], file_record_row)?
                .collect::<Result<Vec<_>, _>>()?
        }
        ScopeKind::File(rel_path) => {
            let mut stmt = conn.prepare(
                "SELECT rel_path, size_bytes, modified_unix_ms
                 FROM files
                 WHERE rel_path = ?1",
            )?;
            stmt.query_map([rel_path], file_record_row)?
                .collect::<Result<Vec<_>, _>>()?
        }
        ScopeKind::Directory(rel_path, prefix) => {
            let mut stmt = conn.prepare(
                "SELECT rel_path, size_bytes, modified_unix_ms
                 FROM files
                 WHERE rel_path = ?1 OR rel_path LIKE ?2",
            )?;
            stmt.query_map(params![rel_path, prefix], file_record_row)?
                .collect::<Result<Vec<_>, _>>()?
        }
    };

    Ok(rows
        .into_iter()
        .map(|row| (row.rel_path.clone(), row))
        .collect::<HashMap<_, _>>())
}

pub(crate) fn status_for_index_root(root: &Path) -> ZgResult<IndexStatus> {
    let conn = open_existing_db(root)?;
    validate_schema(&conn)?;

    let file_count =
        conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))? as u64;
    let chunk_count = conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| {
        row.get::<_, i64>(0)
    })? as u64;
    let fts_ready = conn.query_row("SELECT COUNT(*) FROM fts_chunks", [], |row| {
        row.get::<_, i64>(0)
    })? as u64
        == chunk_count;
    let vector_ready = conn.query_row("SELECT COUNT(*) FROM vec_chunks", [], |row| {
        row.get::<_, i64>(0)
    })? as u64
        == chunk_count
        && conn.query_row("SELECT COUNT(*) FROM vec_index", [], |row| {
            row.get::<_, i64>(0)
        })? as u64
            == chunk_count;
    let state = load_state(&conn)?.unwrap_or(StateRow {
        dirty: false,
        dirty_reason: None,
        last_sync_unix_ms: None,
    });

    Ok(IndexStatus {
        requested_path: root.to_path_buf(),
        index_root: Some(root.to_path_buf()),
        indexed: true,
        chunk_mode: DEFAULT_CHUNK_MODE.to_string(),
        chunk_marker: DEFAULT_CHUNK_MARKER.to_string(),
        scope_policy: DEFAULT_SCOPE_POLICY.to_string(),
        walk_policy: DEFAULT_WALK_POLICY.to_string(),
        dirty: state.dirty,
        dirty_reason: state.dirty_reason,
        last_sync_unix_ms: state.last_sync_unix_ms,
        file_count,
        chunk_count,
        fts_ready,
        vector_ready,
    })
}

pub(crate) fn load_state_mirror_status(
    requested_path: &Path,
    index_root: Option<PathBuf>,
) -> IndexStatus {
    IndexStatus {
        requested_path: requested_path.to_path_buf(),
        index_root,
        indexed: false,
        chunk_mode: DEFAULT_CHUNK_MODE.to_string(),
        chunk_marker: DEFAULT_CHUNK_MARKER.to_string(),
        scope_policy: DEFAULT_SCOPE_POLICY.to_string(),
        walk_policy: DEFAULT_WALK_POLICY.to_string(),
        dirty: false,
        dirty_reason: None,
        last_sync_unix_ms: None,
        file_count: 0,
        chunk_count: 0,
        fts_ready: false,
        vector_ready: false,
    }
}

fn file_record_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileRecord> {
    Ok(FileRecord {
        rel_path: row.get(0)?,
        size_bytes: row.get::<_, i64>(1)? as u64,
        modified_unix_ms: row.get::<_, i64>(2)? as u64,
    })
}
