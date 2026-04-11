use std::path::PathBuf;

use serde::Serialize;

pub(crate) const SCHEMA_VERSION: u32 = 2;
pub(crate) const DEFAULT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const DEFAULT_CHUNK_MODE: &str = "line";
pub(crate) const DEFAULT_CHUNK_MARKER: &str = " :: ";
pub(crate) const DEFAULT_SCOPE_POLICY: &str = "suffix + encoding/character whitelist";
pub(crate) const DEFAULT_VECTOR_PROVIDER: &str = "local-hash-v1";

pub(crate) const VECTOR_DIMENSIONS: usize = 64;
pub(crate) const FTS_CANDIDATE_LIMIT: usize = 96;
pub(crate) const VECTOR_CANDIDATE_LIMIT: usize = 96;
pub(crate) const RRF_K: f64 = 20.0;

pub(crate) const ALLOWED_EXTENSIONS: &[&str] = &[
    "c", "cc", "cfg", "conf", "cpp", "css", "csv", "go", "h", "hpp", "html", "ini", "java", "js",
    "json", "jsx", "kt", "kts", "log", "lua", "md", "markdown", "mjs", "py", "rb", "rs", "sass",
    "scala", "scss", "sh", "sql", "swift", "toml", "ts", "tsx", "txt", "xml", "yaml", "yml", "zsh",
];
pub(crate) const ALLOWED_BASENAMES: &[&str] = &[
    "AGENTS.md",
    "Dockerfile",
    "LICENSE",
    "Makefile",
    "README",
    "README.md",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebuildStats {
    pub scanned_files: usize,
    pub indexed_files: usize,
    pub chunks_indexed: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub rel_path: String,
    pub snippet: String,
    pub score: f64,
    pub lexical_score: f64,
    pub vector_score: f64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexStatus {
    pub requested_path: PathBuf,
    pub index_root: Option<PathBuf>,
    pub indexed: bool,
    pub chunk_mode: String,
    pub chunk_marker: String,
    pub scope_policy: String,
    pub walk_policy: String,
    pub dirty: bool,
    pub dirty_reason: Option<String>,
    pub last_sync_unix_ms: Option<u64>,
    pub file_count: u64,
    pub chunk_count: u64,
    pub fts_ready: bool,
    pub vector_ready: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct FileRecord {
    pub(crate) rel_path: String,
    pub(crate) size_bytes: u64,
    pub(crate) modified_unix_ms: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct IndexedDocument {
    pub(crate) size_bytes: u64,
    pub(crate) modified_unix_ms: u64,
    pub(crate) content_hash: String,
    pub(crate) chunks: Vec<IndexedChunk>,
}

#[derive(Clone, Debug)]
pub(crate) struct IndexedChunk {
    pub(crate) chunk_index: usize,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) raw_text: String,
    pub(crate) normalized_text: String,
    pub(crate) text_hash: String,
    pub(crate) vector: Vec<f32>,
}

#[derive(Clone, Debug)]
pub(crate) struct SyncStats {
    pub(crate) indexed_files: usize,
    pub(crate) chunks_indexed: usize,
    pub(crate) warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct StateRow {
    pub(crate) dirty: bool,
    pub(crate) dirty_reason: Option<String>,
    pub(crate) last_sync_unix_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct StoredChunk {
    pub(crate) chunk_id: i64,
    pub(crate) rel_path: String,
    pub(crate) raw_text: String,
    pub(crate) lexical_score: f64,
    pub(crate) vector_score: f64,
}

#[derive(Serialize)]
pub(crate) struct StateMirror {
    pub(crate) schema_version: u32,
    pub(crate) index_root: String,
    pub(crate) indexed: bool,
    pub(crate) chunk_mode: &'static str,
    pub(crate) chunk_marker: &'static str,
    pub(crate) scope_policy: &'static str,
    pub(crate) walk_policy: &'static str,
    pub(crate) dirty: bool,
    pub(crate) dirty_reason: Option<String>,
    pub(crate) last_sync_unix_ms: Option<u64>,
    pub(crate) file_count: u64,
    pub(crate) chunk_count: u64,
    pub(crate) fts_ready: bool,
    pub(crate) vector_ready: bool,
}

pub(crate) enum ScopeKind {
    Root,
    File(String),
    Directory(String, String),
}
