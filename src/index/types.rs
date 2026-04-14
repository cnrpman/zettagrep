use std::path::PathBuf;
use std::str::FromStr;

use serde::Serialize;

pub(crate) const SCHEMA_VERSION: u32 = 10;
pub(crate) const DEFAULT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const DEFAULT_MAX_FILE_LINES: usize = 100_000;
pub(crate) const DEFAULT_CHUNK_MODE: &str = "line+short-merge";
pub(crate) const DEFAULT_CHUNK_MARKER: &str = " :: ";
pub(crate) const DEFAULT_SHORT_CHUNK_MERGE_MAX_CHARS: usize = 48;
pub(crate) const DEFAULT_SCOPE_POLICY: &str =
    "document suffix + supported code language whitelist + encoding/character whitelist";
pub(crate) const DEFAULT_VECTOR_PROVIDER: &str = "fastembed-ParaphraseMLMiniLML12V2Q";
pub(crate) const DEFAULT_INDEX_LEVEL: IndexLevel = IndexLevel::Fts;
pub(crate) const INIT_SANITY_CHECK_MULTIPLIER: usize = 10;
pub const FTS_PROMPT_MAX_CHUNKS: usize = 3_000;
// Vector prompt gating is based on steady-state embedding throughput after model
// initialization, not on cold-start wall time for a fresh CLI process.
pub const VECTOR_PROMPT_MAX_CHUNKS: usize = 1_024;

pub(crate) const VECTOR_DIMENSIONS: usize = 384;
pub(crate) const FTS_CANDIDATE_LIMIT: usize = 96;
pub(crate) const VECTOR_CANDIDATE_LIMIT: usize = 96;
// Weak near-zero semantic similarity adds noisy recall once rank fusion starts
// rewarding candidate position. Keep the floor conservative so lexical matches
// still dominate while clearly relevant semantic-only hits remain visible.
pub(crate) const MIN_VECTOR_SCORE_FOR_MERGE: f64 = 0.20;
pub(crate) const MAX_SEMANTIC_ONLY_HITS_WITH_LEXICAL: usize = 5;
pub(crate) const RRF_K: f64 = 20.0;

pub(crate) const ALLOWED_EXTENSIONS: &[&str] = &[
    "adoc", "asciidoc", "markdown", "md", "org", "rst", "text", "txt",
];
pub(crate) const ALLOWED_BASENAMES: &[&str] = &["LICENSE", "README", "CHANGELOG", "CONTRIBUTING"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebuildStats {
    pub scanned_files: usize,
    pub indexed_files: usize,
    pub chunks_indexed: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitPreflight {
    pub estimated_chunks: usize,
    pub recommended_chunk_limit: usize,
    pub force_threshold: usize,
    pub requires_force: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum IndexLevel {
    #[serde(rename = "fts")]
    Fts,
    #[serde(rename = "fts+vector")]
    FtsVector,
}

impl IndexLevel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Fts => "fts",
            Self::FtsVector => "fts+vector",
        }
    }

    pub(crate) fn vectors_enabled(self) -> bool {
        matches!(self, Self::FtsVector)
    }

    pub(crate) fn recommended_chunk_limit(self) -> usize {
        match self {
            Self::Fts => FTS_PROMPT_MAX_CHUNKS,
            Self::FtsVector => VECTOR_PROMPT_MAX_CHUNKS,
        }
    }

    pub(crate) fn init_force_chunk_limit(self) -> usize {
        self.recommended_chunk_limit()
            .saturating_mul(INIT_SANITY_CHECK_MULTIPLIER)
    }
}

impl std::fmt::Display for IndexLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for IndexLevel {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "fts" => Ok(Self::Fts),
            "fts+vector" => Ok(Self::FtsVector),
            _ => Err("expected `fts` or `fts+vector`"),
        }
    }
}

impl IndexStatus {
    pub fn index_level_known(&self) -> bool {
        self.index_level_known
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub rel_path: String,
    pub snippet: String,
    pub line_start: usize,
    pub line_end: usize,
    pub score: f64,
    pub lexical_score: f64,
    pub vector_score: f64,
    pub indexed_text_match: bool,
    pub partial_text_match: bool,
    pub literal_line_number: Option<usize>,
    pub literal_preview: Option<String>,
    pub(crate) chunk_index: usize,
    pub(crate) chunk_kind: String,
    pub(crate) language: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IndexStatus {
    pub requested_path: PathBuf,
    pub index_root: Option<PathBuf>,
    pub indexed: bool,
    pub index_level: IndexLevel,
    #[serde(skip)]
    pub(crate) index_level_known: bool,
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
    pub last_index_run_status: Option<String>,
    pub last_index_run_duration_ms: Option<u64>,
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
    pub(crate) shared_normalized_text: String,
    pub(crate) shared_normalized_text_hash: String,
    pub(crate) chunk_kind: String,
    pub(crate) language: Option<String>,
    pub(crate) symbol_kind: Option<String>,
    pub(crate) container: Option<String>,
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
    pub(crate) chunk_index: usize,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) chunk_kind: String,
    pub(crate) language: Option<String>,
    pub(crate) lexical_score: f64,
    pub(crate) vector_score: f64,
    pub(crate) indexed_text_match: bool,
    pub(crate) partial_text_match: bool,
    pub(crate) literal_line_number: Option<usize>,
    pub(crate) literal_preview: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct StateSnapshot {
    pub(crate) schema_version: u32,
    pub(crate) index_root: String,
    pub(crate) indexed: bool,
    pub(crate) index_level: String,
    pub(crate) chunk_mode: String,
    pub(crate) chunk_marker: String,
    pub(crate) scope_policy: String,
    pub(crate) walk_policy: String,
    pub(crate) dirty: bool,
    pub(crate) dirty_reason: Option<String>,
    pub(crate) last_sync_unix_ms: Option<u64>,
    pub(crate) file_count: u64,
    pub(crate) chunk_count: u64,
    pub(crate) fts_ready: bool,
    pub(crate) vector_ready: bool,
    pub(crate) last_index_run_status: Option<String>,
    pub(crate) last_index_run_duration_ms: Option<u64>,
}

pub(crate) enum ScopeKind {
    Root,
    File(String),
    Directory(String, String),
}
