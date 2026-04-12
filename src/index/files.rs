use std::fs;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::paths;
use crate::walk;
use crate::{ZgResult, normalize_query};

use super::types::{
    ALLOWED_BASENAMES, ALLOWED_EXTENSIONS, DEFAULT_CHUNK_MARKER, DEFAULT_MAX_FILE_BYTES,
    DEFAULT_MAX_FILE_LINES, IndexedChunk, IndexedDocument,
};
use super::util::{has_zg_component, modified_unix_ms, stable_hash};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateScanSummary {
    pub(crate) candidate_files: usize,
    pub(crate) total_size_bytes: u64,
    pub(crate) limit_tripped: bool,
}

pub fn collect_candidate_files(scope: &Path) -> ZgResult<Vec<PathBuf>> {
    let scope = paths::resolve_existing_path(scope)?;
    if scope.is_file() {
        return Ok(if candidate_file_size(&scope)?.is_some() {
            vec![scope]
        } else {
            Vec::new()
        });
    }

    let mut builder = WalkBuilder::new(&scope);
    walk::apply_content_filters(&mut builder);

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
        if file_type.is_file() && candidate_file_size(path)?.is_some() {
            files.push(path.to_path_buf());
        }
    }

    Ok(files)
}

pub(crate) fn scan_candidate_files_until(
    scope: &Path,
    max_files: usize,
    max_total_bytes: u64,
) -> ZgResult<CandidateScanSummary> {
    let scope = paths::resolve_existing_path(scope)?;
    let mut summary = CandidateScanSummary {
        candidate_files: 0,
        total_size_bytes: 0,
        limit_tripped: false,
    };

    if scope.is_file() {
        if let Some(size_bytes) = candidate_file_size(&scope)? {
            summary.candidate_files = 1;
            summary.total_size_bytes = size_bytes;
            summary.limit_tripped =
                summary.candidate_files >= max_files || summary.total_size_bytes >= max_total_bytes;
        }
        return Ok(summary);
    }

    let mut builder = WalkBuilder::new(&scope);
    walk::apply_content_filters(&mut builder);

    for entry in builder.build() {
        let entry = entry?;
        let path = entry.path();
        let Some(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_symlink() || has_zg_component(path) || !file_type.is_file() {
            continue;
        }
        let Some(size_bytes) = candidate_file_size(path)? else {
            continue;
        };

        summary.candidate_files += 1;
        summary.total_size_bytes += size_bytes;
        if summary.candidate_files >= max_files || summary.total_size_bytes >= max_total_bytes {
            summary.limit_tripped = true;
            break;
        }
    }

    Ok(summary)
}

pub(crate) fn collect_scope_candidates(root: &Path, scope: &Path) -> ZgResult<Vec<PathBuf>> {
    let scope = paths::resolve_existing_path(scope)?;
    if scope.is_file() {
        return Ok(
            if scope.starts_with(root) && candidate_file_size(&scope)?.is_some() {
                vec![scope]
            } else {
                Vec::new()
            },
        );
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
    if exceeds_line_limit(&body) {
        return Ok(None);
    }

    let chunks = build_chunks(&body)?;
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

fn build_chunks(body: &str) -> ZgResult<Vec<IndexedChunk>> {
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
                normalized_text_hash: stable_hash(normalized_text.as_bytes()),
            });
        }
    }

    Ok(chunks)
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

fn candidate_file_size(path: &Path) -> ZgResult<Option<u64>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > DEFAULT_MAX_FILE_BYTES {
        return Ok(None);
    }
    if metadata.file_type().is_symlink() || has_zg_component(path) {
        return Ok(None);
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
        return Ok(None);
    }

    let bytes = fs::read(path)?;
    if !bytes_are_text_whitelisted(&bytes) {
        return Ok(None);
    }

    Ok(Some(metadata.len()))
}

fn exceeds_line_limit(body: &str) -> bool {
    body.lines().take(DEFAULT_MAX_FILE_LINES + 1).count() > DEFAULT_MAX_FILE_LINES
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{collect_candidate_files, load_indexable_document, scan_candidate_files_until};

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zg-files-{name}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rel_names(root: &Path, paths: Vec<PathBuf>) -> Vec<String> {
        let mut names = paths
            .into_iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn candidate_collection_skips_hidden_files_and_zg_state() {
        let root = temp_dir("hidden");
        fs::write(root.join("keep.md"), "visible note").unwrap();
        fs::write(root.join(".hidden.md"), "hidden note").unwrap();
        fs::create_dir_all(root.join(".zg")).unwrap();
        fs::write(root.join(".zg/internal.md"), "state note").unwrap();

        let files = rel_names(&root, collect_candidate_files(&root).unwrap());
        assert_eq!(files, vec!["keep.md"]);
    }

    #[test]
    fn candidate_collection_uses_parent_ignore_and_local_zgignore_override() {
        let root = temp_dir("override");
        let child = root.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::write(root.join(".ignore"), "*.md\n").unwrap();
        fs::write(child.join(".zgignore"), "!keep.md\n").unwrap();
        fs::write(child.join("keep.md"), "keep me").unwrap();
        fs::write(child.join("blocked.md"), "block me").unwrap();

        let files = rel_names(&child, collect_candidate_files(&child).unwrap());
        assert_eq!(files, vec!["keep.md"]);
    }

    #[test]
    fn candidate_collection_only_indexes_document_like_files() {
        let root = temp_dir("document-only");
        fs::write(root.join("notes.md"), "note").unwrap();
        fs::write(root.join("journal.rst"), "entry").unwrap();
        fs::write(root.join("code.rs"), "fn main() {}").unwrap();
        fs::write(root.join("config.toml"), "name = 'zg'").unwrap();

        let files = rel_names(&root, collect_candidate_files(&root).unwrap());
        assert_eq!(files, vec!["journal.rst", "notes.md"]);
    }

    #[test]
    fn bounded_candidate_scan_stops_after_threshold() {
        let root = temp_dir("bounded-scan");
        for idx in 0..4 {
            fs::write(root.join(format!("note-{idx}.md")), "note").unwrap();
        }

        let summary = scan_candidate_files_until(&root, 3, u64::MAX).unwrap();
        assert_eq!(summary.candidate_files, 3);
        assert!(summary.limit_tripped);
    }

    #[test]
    fn load_indexable_document_skips_files_over_line_limit() {
        let root = temp_dir("line-limit");
        let file = root.join("huge.md");
        let body = std::iter::repeat_n("a\n", 100_001).collect::<String>();
        fs::write(&file, body).unwrap();

        let document = load_indexable_document(&file).unwrap();
        assert!(document.is_none());
    }
}
