use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rusqlite::auto_extension::{RawAutoExtension, register_auto_extension};
use rusqlite::ffi::ErrorCode;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sqlite_vec::sqlite3_vec_init;

use crate::messages::INDEX_SCHEMA_VERSION_MISMATCH;
use crate::paths;
use crate::walk::DEFAULT_WALK_POLICY;
use crate::{ZgResult, other};

use super::embed::embed_passages;
use super::types::{
    DEFAULT_CHUNK_MARKER, DEFAULT_CHUNK_MODE, DEFAULT_INDEX_LEVEL, DEFAULT_SCOPE_POLICY,
    DEFAULT_VECTOR_PROVIDER, FileRecord, IndexLevel, IndexStatus, IndexedDocument, SCHEMA_VERSION,
    ScopeKind, StateMirror, StateRow, VECTOR_DIMENSIONS,
};
use super::util::{now_unix_ms, scope_kind};

static SQLITE_VEC_REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
const WRITE_TX_MAX_WAIT: Duration = Duration::from_secs(900);
const WRITE_TX_RETRY_DELAY_CAP: Duration = Duration::from_secs(5);

pub(crate) fn open_or_create_db(root: &Path) -> ZgResult<Connection> {
    ensure_sqlite_vec_registered()?;
    let db_path = paths::db_path(root);
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(Duration::from_secs(3))?;
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

pub(crate) fn with_write_transaction_retry<T, F>(
    conn: &Connection,
    root: &Path,
    operation: &str,
    mut work: F,
) -> ZgResult<T>
where
    F: FnMut(&rusqlite::Transaction<'_>) -> ZgResult<T>,
{
    let started_at = Instant::now();
    let mut attempt = 0u32;
    loop {
        let tx = match Transaction::new_unchecked(conn, TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(error) if is_sqlite_busy_or_locked_rusqlite(&error) => {
                if let Some(delay) = retry_delay(started_at, attempt) {
                    std::thread::sleep(delay);
                    attempt += 1;
                    continue;
                }
                return Err(write_lock_retry_exhausted(root, operation, error.into()));
            }
            Err(error) => return Err(error.into()),
        };

        match work(&tx) {
            Ok(result) => match tx.commit() {
                Ok(()) => return Ok(result),
                Err(error) if is_sqlite_busy_or_locked_rusqlite(&error) => {
                    if let Some(delay) = retry_delay(started_at, attempt) {
                        std::thread::sleep(delay);
                        attempt += 1;
                        continue;
                    }
                    return Err(write_lock_retry_exhausted(root, operation, error.into()));
                }
                Err(error) => return Err(error.into()),
            },
            Err(error) if is_sqlite_busy_or_locked(&error) => {
                drop(tx);
                if let Some(delay) = retry_delay(started_at, attempt) {
                    std::thread::sleep(delay);
                    attempt += 1;
                    continue;
                }
                return Err(write_lock_retry_exhausted(root, operation, error));
            }
            Err(error) => return Err(error),
        }
    }
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

fn retry_delay(started_at: Instant, attempt: u32) -> Option<Duration> {
    let elapsed = started_at.elapsed();
    let remaining = WRITE_TX_MAX_WAIT.checked_sub(elapsed)?;
    let backoff_ms = 200u64.saturating_mul(1u64 << attempt.min(5));
    Some(
        Duration::from_millis(backoff_ms)
            .min(WRITE_TX_RETRY_DELAY_CAP)
            .min(remaining),
    )
}

fn is_sqlite_busy_or_locked(error: &crate::DynError) -> bool {
    error
        .downcast_ref::<rusqlite::Error>()
        .is_some_and(is_sqlite_busy_or_locked_rusqlite)
}

fn is_sqlite_busy_or_locked_rusqlite(error: &rusqlite::Error) -> bool {
    match error {
        rusqlite::Error::SqliteFailure(inner, _) => {
            matches!(
                inner.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            )
        }
        _ => false,
    }
}

fn write_lock_retry_exhausted(
    root: &Path,
    operation: &str,
    error: crate::DynError,
) -> crate::DynError {
    other(format!(
        "sqlite write lock persisted while {operation} {} for up to {} seconds: {error}",
        root.display(),
        WRITE_TX_MAX_WAIT.as_secs(),
    ))
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
        CREATE TABLE IF NOT EXISTS shared_chunks (
            id INTEGER PRIMARY KEY,
            normalized_text_hash TEXT NOT NULL,
            normalized_text TEXT NOT NULL,
            ref_count INTEGER NOT NULL,
            created_at_unix_ms INTEGER NOT NULL,
            last_used_unix_ms INTEGER NOT NULL,
            UNIQUE(normalized_text_hash, normalized_text)
        );
        CREATE TABLE IF NOT EXISTS chunk_refs (
            id INTEGER PRIMARY KEY,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            shared_chunk_id INTEGER NOT NULL REFERENCES shared_chunks(id),
            chunk_kind TEXT NOT NULL,
            language TEXT,
            symbol_kind TEXT,
            container TEXT,
            chunk_index INTEGER NOT NULL,
            line_start INTEGER NOT NULL,
            line_end INTEGER NOT NULL,
            normalized_text TEXT NOT NULL,
            shared_normalized_text_hash TEXT NOT NULL,
            UNIQUE(file_id, chunk_index)
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS fts_chunks USING fts5(
            rel_path UNINDEXED,
            normalized_text,
            tokenize = 'unicode61 remove_diacritics 2'
        );
        CREATE TABLE IF NOT EXISTS vec_chunks (
            shared_chunk_id INTEGER PRIMARY KEY REFERENCES shared_chunks(id) ON DELETE CASCADE,
            dims INTEGER NOT NULL,
            vector BLOB NOT NULL
                CHECK(typeof(vector) = 'blob' AND vec_length(vector) = dims)
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS vec_index USING vec0(
            shared_chunk_id INTEGER PRIMARY KEY,
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

pub(crate) fn reset_schema(conn: &Connection) -> ZgResult<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS index_runs;
         DROP TABLE IF EXISTS vec_chunks;
         DROP TABLE IF EXISTS fts_chunks;
         DROP TABLE IF EXISTS chunk_refs;
         DROP TABLE IF EXISTS shared_chunks;
         DROP TABLE IF EXISTS files;
         DROP TABLE IF EXISTS state;
         DROP TABLE IF EXISTS settings;
         DROP TABLE IF EXISTS vec_index;",
    )?;
    Ok(())
}

pub(crate) fn validate_schema(conn: &Connection) -> ZgResult<()> {
    let required = [
        "settings",
        "state",
        "files",
        "shared_chunks",
        "chunk_refs",
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
        return Err(other(INDEX_SCHEMA_VERSION_MISMATCH));
    }

    Ok(())
}

pub(crate) fn seed_defaults(conn: &Connection) -> ZgResult<()> {
    let settings = [
        ("schema_version", SCHEMA_VERSION.to_string()),
        ("index_level", DEFAULT_INDEX_LEVEL.to_string()),
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

pub(crate) fn load_index_level(conn: &Connection) -> ZgResult<IndexLevel> {
    let value = load_setting(conn, "index_level")?;
    match value {
        Some(value) => value
            .parse::<IndexLevel>()
            .map_err(|error| other(format!("invalid index level setting `{value}`: {error}"))),
        None => Ok(DEFAULT_INDEX_LEVEL),
    }
}

pub(crate) fn set_index_level(conn: &Connection, index_level: IndexLevel) -> ZgResult<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('index_level', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [index_level.as_str()],
    )?;
    Ok(())
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
        index_level: DEFAULT_INDEX_LEVEL.as_str(),
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
        last_index_run_status: None,
        last_index_run_duration_ms: None,
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
        index_level: status.index_level.as_str(),
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
        last_index_run_status: status.last_index_run_status.clone(),
        last_index_run_duration_ms: status.last_index_run_duration_ms,
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
    prepared_vectors: Option<&HashMap<SharedChunkKey, Vec<f32>>>,
    vectors_enabled: bool,
) -> ZgResult<()> {
    let old_counts = load_shared_ref_counts_for_rel_path(conn, rel_path)?;
    delete_file_rows_by_rel_path(conn, rel_path)?;

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
    let shared_chunk_ids =
        resolve_shared_chunk_ids(conn, &document.chunks, prepared_vectors, vectors_enabled)?;
    let mut new_counts = HashMap::<i64, i64>::new();

    for (chunk, shared_chunk_id) in document.chunks.iter().zip(shared_chunk_ids.into_iter()) {
        conn.execute(
            "INSERT INTO chunk_refs (
                file_id,
                shared_chunk_id,
                chunk_kind,
                language,
                symbol_kind,
                container,
                chunk_index,
                line_start,
                line_end,
                normalized_text,
                shared_normalized_text_hash
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                file_id,
                shared_chunk_id,
                chunk.chunk_kind,
                chunk.language,
                chunk.symbol_kind,
                chunk.container,
                chunk.chunk_index as i64,
                chunk.line_start as i64,
                chunk.line_end as i64,
                chunk.normalized_text,
                chunk.shared_normalized_text_hash,
            ],
        )?;
        let chunk_ref_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO fts_chunks (rowid, rel_path, normalized_text) VALUES (?1, ?2, ?3)",
            params![chunk_ref_id, rel_path, chunk.normalized_text],
        )?;
        *new_counts.entry(shared_chunk_id).or_insert(0) += 1;
    }

    apply_shared_ref_count_deltas(conn, &old_counts, &new_counts)?;
    Ok(())
}

pub(crate) type SharedChunkKey = (String, String);

pub(crate) fn prepare_missing_shared_chunk_vectors(
    conn: &Connection,
    documents: &[&IndexedDocument],
    vectors_enabled: bool,
) -> ZgResult<HashMap<SharedChunkKey, Vec<f32>>> {
    if !vectors_enabled || documents.is_empty() {
        return Ok(HashMap::new());
    }

    let mut unique_keys = Vec::new();
    let mut seen = HashSet::<SharedChunkKey>::new();
    for document in documents {
        for chunk in &document.chunks {
            let key = (
                chunk.shared_normalized_text_hash.clone(),
                chunk.shared_normalized_text.clone(),
            );
            if seen.insert(key.clone()) {
                unique_keys.push(key);
            }
        }
    }

    if unique_keys.is_empty() {
        return Ok(HashMap::new());
    }

    let mut select_stmt = conn.prepare(
        "SELECT 1
         FROM shared_chunks
         WHERE normalized_text_hash = ?1
           AND normalized_text = ?2",
    )?;
    let mut missing_keys = Vec::new();
    for key in unique_keys {
        let exists = select_stmt
            .query_row(params![&key.0, &key.1], |row| row.get::<_, i64>(0))
            .optional()?
            .is_some();
        if !exists {
            missing_keys.push(key);
        }
    }

    if missing_keys.is_empty() {
        return Ok(HashMap::new());
    }

    let texts = missing_keys
        .iter()
        .map(|(_, text)| text.clone())
        .collect::<Vec<_>>();
    let vectors = embed_passages(&texts)?;
    if vectors.len() != missing_keys.len() {
        return Err(other("fastembed returned unexpected embedding count"));
    }

    Ok(missing_keys.into_iter().zip(vectors).collect())
}

pub(crate) fn delete_by_rel_path(conn: &Connection, rel_path: &str) -> ZgResult<()> {
    let old_counts = load_shared_ref_counts_for_rel_path(conn, rel_path)?;
    if old_counts.is_empty() {
        delete_file_rows_by_rel_path(conn, rel_path)?;
        return Ok(());
    }

    delete_file_rows_by_rel_path(conn, rel_path)?;
    apply_shared_ref_count_deltas(conn, &old_counts, &HashMap::new())?;
    Ok(())
}

pub(crate) fn gc_unreferenced_shared_chunks(conn: &Connection) -> ZgResult<()> {
    let mut stmt = conn.prepare(
        "SELECT id
         FROM shared_chunks sc
         WHERE NOT EXISTS (
             SELECT 1
             FROM chunk_refs cr
             WHERE cr.shared_chunk_id = sc.id
         )",
    )?;
    let shared_chunk_ids = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    for shared_chunk_id in shared_chunk_ids {
        conn.execute(
            "DELETE FROM vec_index WHERE shared_chunk_id = ?1",
            [shared_chunk_id],
        )?;
        conn.execute(
            "DELETE FROM vec_chunks WHERE shared_chunk_id = ?1",
            [shared_chunk_id],
        )?;
        conn.execute("DELETE FROM shared_chunks WHERE id = ?1", [shared_chunk_id])?;
    }

    Ok(())
}

fn resolve_shared_chunk_ids(
    conn: &Connection,
    chunks: &[super::types::IndexedChunk],
    prepared_vectors: Option<&HashMap<SharedChunkKey, Vec<f32>>>,
    vectors_enabled: bool,
) -> ZgResult<Vec<i64>> {
    let mut keys = Vec::with_capacity(chunks.len());
    let mut unique_keys = Vec::new();
    let mut seen = HashSet::<(String, String)>::new();
    for chunk in chunks {
        let key = (
            chunk.shared_normalized_text_hash.clone(),
            chunk.shared_normalized_text.clone(),
        );
        if seen.insert(key.clone()) {
            unique_keys.push(key.clone());
        }
        keys.push(key);
    }

    let mut shared_ids = HashMap::<(String, String), i64>::new();
    let mut select_stmt = conn.prepare(
        "SELECT id
         FROM shared_chunks
         WHERE normalized_text_hash = ?1
           AND normalized_text = ?2",
    )?;
    let mut missing = Vec::new();
    for key in unique_keys {
        let existing = select_stmt
            .query_row(params![&key.0, &key.1], |row| row.get::<_, i64>(0))
            .optional()?;
        if let Some(shared_chunk_id) = existing {
            shared_ids.insert(key, shared_chunk_id);
        } else {
            missing.push(key);
        }
    }

    if !missing.is_empty() {
        let mut embedded_missing = HashMap::<SharedChunkKey, Vec<f32>>::new();
        if vectors_enabled {
            let fallback_missing = missing
                .iter()
                .filter(|key| prepared_vectors.is_none_or(|vectors| !vectors.contains_key(*key)))
                .cloned()
                .collect::<Vec<_>>();
            if !fallback_missing.is_empty() {
                let normalized = fallback_missing
                    .iter()
                    .map(|(_, text)| text.clone())
                    .collect::<Vec<_>>();
                let vectors = embed_passages(&normalized)?;
                if vectors.len() != fallback_missing.len() {
                    return Err(other("fastembed returned unexpected embedding count"));
                }
                embedded_missing.extend(fallback_missing.into_iter().zip(vectors));
            }
        }

        let now = now_unix_ms() as i64;
        for (hash, text) in missing {
            conn.execute(
                "INSERT OR IGNORE INTO shared_chunks (
                    normalized_text_hash,
                    normalized_text,
                    ref_count,
                    created_at_unix_ms,
                    last_used_unix_ms
                ) VALUES (?1, ?2, 0, ?3, ?3)",
                params![hash, text, now],
            )?;
            let inserted = conn.changes() > 0;
            let shared_chunk_id =
                select_stmt.query_row(params![&hash, &text], |row| row.get::<_, i64>(0))?;
            if inserted && vectors_enabled {
                let vector = prepared_vectors
                    .and_then(|vectors| vectors.get(&(hash.clone(), text.clone())))
                    .or_else(|| embedded_missing.get(&(hash.clone(), text.clone())))
                    .ok_or_else(|| {
                        other("failed to resolve prepared embedding for shared chunk")
                    })?;
                let encoded = super::hybrid::encode_vector(vector);
                conn.execute(
                    "INSERT INTO vec_chunks (shared_chunk_id, dims, vector) VALUES (?1, ?2, ?3)",
                    params![shared_chunk_id, vector.len() as i64, &encoded],
                )?;
                conn.execute(
                    "INSERT INTO vec_index (shared_chunk_id, embedding) VALUES (?1, ?2)",
                    params![shared_chunk_id, &encoded],
                )?;
            }
            shared_ids.insert((hash, text), shared_chunk_id);
        }
    }

    let now = now_unix_ms() as i64;
    let used_ids = shared_ids.values().copied().collect::<HashSet<_>>();
    for shared_chunk_id in used_ids {
        conn.execute(
            "UPDATE shared_chunks SET last_used_unix_ms = ?2 WHERE id = ?1",
            params![shared_chunk_id, now],
        )?;
    }

    keys.into_iter()
        .map(|key| {
            shared_ids
                .get(&key)
                .copied()
                .ok_or_else(|| other("failed to resolve shared chunk id"))
        })
        .collect()
}

fn load_shared_ref_counts_for_rel_path(
    conn: &Connection,
    rel_path: &str,
) -> ZgResult<HashMap<i64, i64>> {
    let file_id = conn
        .query_row(
            "SELECT id FROM files WHERE rel_path = ?1",
            [rel_path],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let Some(file_id) = file_id else {
        return Ok(HashMap::new());
    };
    load_shared_ref_counts_for_file_id(conn, file_id)
}

fn load_shared_ref_counts_for_file_id(
    conn: &Connection,
    file_id: i64,
) -> ZgResult<HashMap<i64, i64>> {
    let mut stmt = conn.prepare(
        "SELECT shared_chunk_id, COUNT(*)
         FROM chunk_refs
         WHERE file_id = ?1
         GROUP BY shared_chunk_id",
    )?;
    let rows = stmt
        .query_map([file_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().collect())
}

fn delete_file_rows_by_rel_path(conn: &Connection, rel_path: &str) -> ZgResult<()> {
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

    let mut stmt = conn.prepare("SELECT id FROM chunk_refs WHERE file_id = ?1")?;
    let chunk_ref_ids = stmt
        .query_map([file_id], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    for chunk_ref_id in chunk_ref_ids {
        conn.execute("DELETE FROM fts_chunks WHERE rowid = ?1", [chunk_ref_id])?;
    }
    conn.execute("DELETE FROM chunk_refs WHERE file_id = ?1", [file_id])?;
    conn.execute("DELETE FROM files WHERE id = ?1", [file_id])?;
    Ok(())
}

fn apply_shared_ref_count_deltas(
    conn: &Connection,
    old_counts: &HashMap<i64, i64>,
    new_counts: &HashMap<i64, i64>,
) -> ZgResult<()> {
    let shared_chunk_ids = old_counts
        .keys()
        .chain(new_counts.keys())
        .copied()
        .collect::<HashSet<_>>();
    let now = now_unix_ms() as i64;
    for shared_chunk_id in shared_chunk_ids {
        let old_count = old_counts.get(&shared_chunk_id).copied().unwrap_or(0);
        let new_count = new_counts.get(&shared_chunk_id).copied().unwrap_or(0);
        let delta = new_count - old_count;
        if delta == 0 {
            continue;
        }

        conn.execute(
            "UPDATE shared_chunks
             SET ref_count = ref_count + ?2,
                 last_used_unix_ms = CASE WHEN ?2 > 0 THEN ?3 ELSE last_used_unix_ms END
             WHERE id = ?1",
            params![shared_chunk_id, delta, now],
        )?;
    }
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
    let index_level = load_index_level(&conn)?;

    let file_count =
        conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))? as u64;
    let chunk_count = conn.query_row("SELECT COUNT(*) FROM chunk_refs", [], |row| {
        row.get::<_, i64>(0)
    })? as u64;
    let shared_chunk_count = conn.query_row("SELECT COUNT(*) FROM shared_chunks", [], |row| {
        row.get::<_, i64>(0)
    })? as u64;
    let fts_ready = conn.query_row("SELECT COUNT(*) FROM fts_chunks", [], |row| {
        row.get::<_, i64>(0)
    })? as u64
        == chunk_count;
    let vector_ready = conn.query_row("SELECT COUNT(*) FROM vec_chunks", [], |row| {
        row.get::<_, i64>(0)
    })? as u64
        == shared_chunk_count
        && conn.query_row("SELECT COUNT(*) FROM vec_index", [], |row| {
            row.get::<_, i64>(0)
        })? as u64
            == shared_chunk_count
        && index_level.vectors_enabled();
    let state = load_state(&conn)?.unwrap_or(StateRow {
        dirty: false,
        dirty_reason: None,
        last_sync_unix_ms: None,
    });
    let last_index_run = conn
        .query_row(
            "SELECT status, started_at_unix_ms, finished_at_unix_ms
             FROM index_runs
             ORDER BY id DESC
             LIMIT 1",
            [],
            |row| {
                let started = row.get::<_, i64>(1)? as u64;
                let finished = row.get::<_, i64>(2)? as u64;
                Ok((row.get::<_, String>(0)?, finished.saturating_sub(started)))
            },
        )
        .optional()?;

    Ok(IndexStatus {
        requested_path: root.to_path_buf(),
        index_root: Some(root.to_path_buf()),
        indexed: true,
        index_level,
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
        last_index_run_status: last_index_run.as_ref().map(|(status, _)| status.clone()),
        last_index_run_duration_ms: last_index_run.map(|(_, duration)| duration),
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
        index_level: DEFAULT_INDEX_LEVEL,
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
        last_index_run_status: None,
        last_index_run_duration_ms: None,
    }
}

fn file_record_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileRecord> {
    Ok(FileRecord {
        rel_path: row.get(0)?,
        size_bytes: row.get::<_, i64>(1)? as u64,
        modified_unix_ms: row.get::<_, i64>(2)? as u64,
    })
}
