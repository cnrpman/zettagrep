use std::fs;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::paths;
use crate::{ZgResult, normalize_query};

use super::types::{
    ALLOWED_BASENAMES, ALLOWED_EXTENSIONS, DEFAULT_CHUNK_MARKER, DEFAULT_MAX_FILE_BYTES,
    IndexedChunk, IndexedDocument,
};
use super::util::{has_zg_component, modified_unix_ms, stable_hash};

pub fn collect_candidate_files(scope: &Path) -> ZgResult<Vec<PathBuf>> {
    let scope = paths::resolve_existing_path(scope)?;
    if scope.is_file() {
        return Ok(if is_candidate_file(&scope)? {
            vec![scope]
        } else {
            Vec::new()
        });
    }

    let mut builder = WalkBuilder::new(&scope);
    builder
        .hidden(false)
        .standard_filters(true)
        .add_custom_ignore_filename(".zgignore");

    let mut files = Vec::new();
    for entry in builder.build() {
        let entry = entry?;
        let path = entry.path();
        let Some(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_symlink() || has_zg_component(path) {
            continue;
        }
        if file_type.is_file() && is_candidate_file(path)? {
            files.push(path.to_path_buf());
        }
    }

    Ok(files)
}

pub fn collect_scan_files(scope: &Path) -> ZgResult<Vec<PathBuf>> {
    let scope = paths::resolve_existing_path(scope)?;
    if scope.is_file() {
        return Ok(if is_scan_file(&scope)? {
            vec![scope]
        } else {
            Vec::new()
        });
    }

    let mut builder = WalkBuilder::new(&scope);
    builder
        .hidden(false)
        .standard_filters(true)
        .add_custom_ignore_filename(".zgignore");

    let mut files = Vec::new();
    for entry in builder.build() {
        let entry = entry?;
        let path = entry.path();
        let Some(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_symlink() || has_zg_component(path) {
            continue;
        }
        if file_type.is_file() && is_scan_file(path)? {
            files.push(path.to_path_buf());
        }
    }

    Ok(files)
}

pub(crate) fn collect_scope_candidates(root: &Path, scope: &Path) -> ZgResult<Vec<PathBuf>> {
    let scope = paths::resolve_existing_path(scope)?;
    if scope.is_file() {
        return Ok(if scope.starts_with(root) && is_candidate_file(&scope)? {
            vec![scope]
        } else {
            Vec::new()
        });
    }

    let files = collect_candidate_files(&scope)?;
    Ok(files
        .into_iter()
        .filter(|path| path.starts_with(root))
        .collect::<Vec<_>>())
}

pub(crate) fn load_indexable_document(path: &Path) -> ZgResult<Option<IndexedDocument>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > DEFAULT_MAX_FILE_BYTES {
        return Ok(None);
    }
    if metadata.file_type().is_symlink() {
        return Ok(None);
    }

    let bytes = fs::read(path)?;
    if !bytes_are_text_whitelisted(&bytes) {
        return Ok(None);
    }
    let body = match String::from_utf8(bytes) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };

    let chunks = build_chunks(&body);
    if chunks.is_empty() {
        return Ok(None);
    }

    Ok(Some(IndexedDocument {
        size_bytes: metadata.len(),
        modified_unix_ms: modified_unix_ms(&metadata)?,
        content_hash: stable_hash(body.as_bytes()),
        chunks,
    }))
}

fn build_chunks(body: &str) -> Vec<IndexedChunk> {
    let mut chunks = Vec::new();
    for (line_index, line) in body.lines().enumerate() {
        for raw_segment in line.split(DEFAULT_CHUNK_MARKER) {
            let raw_text = raw_segment.trim();
            if raw_text.is_empty() {
                continue;
            }

            let cleaned = strip_line_decorator(raw_text);
            let normalized_text = normalize_query(cleaned);
            if normalized_text.is_empty() {
                continue;
            }

            chunks.push(IndexedChunk {
                chunk_index: chunks.len(),
                line_start: line_index + 1,
                line_end: line_index + 1,
                raw_text: cleaned.to_string(),
                normalized_text: normalized_text.clone(),
                text_hash: stable_hash(normalized_text.as_bytes()),
                vector: super::hybrid::vectorize(&normalized_text),
            });
        }
    }
    chunks
}

fn strip_line_decorator(line: &str) -> &str {
    let trimmed = line.trim_start();
    for prefix in ["- ", "* ", "+ ", "> "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.trim_start();
        }
    }

    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if hashes > 0 {
        let rest = &trimmed[hashes..];
        if let Some(rest) = rest.strip_prefix(' ') {
            return rest.trim_start();
        }
    }

    trimmed
}

fn is_candidate_file(path: &Path) -> ZgResult<bool> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > DEFAULT_MAX_FILE_BYTES {
        return Ok(false);
    }
    if metadata.file_type().is_symlink() || has_zg_component(path) {
        return Ok(false);
    }

    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let allowed = ALLOWED_EXTENSIONS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(ext))
        || ALLOWED_BASENAMES
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(file_name));
    if !allowed {
        return Ok(false);
    }

    let bytes = fs::read(path)?;
    Ok(bytes_are_text_whitelisted(&bytes))
}

fn is_scan_file(path: &Path) -> ZgResult<bool> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > DEFAULT_MAX_FILE_BYTES {
        return Ok(false);
    }
    if metadata.file_type().is_symlink() || has_zg_component(path) {
        return Ok(false);
    }

    let bytes = fs::read(path)?;
    Ok(bytes_are_text_whitelisted(&bytes))
}

fn bytes_are_text_whitelisted(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return false;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };

    text.chars()
        .filter(|ch| !ch.is_whitespace())
        .all(|ch| !ch.is_control())
}
