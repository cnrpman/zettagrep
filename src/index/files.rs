use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::thread;

use ignore::WalkBuilder;

use super::code_symbols::{build_symbol_chunks, supports_code_symbol_indexing};
use crate::paths;
use crate::walk;
use crate::{ZgResult, normalize_query};

use super::types::{
    ALLOWED_BASENAMES, ALLOWED_EXTENSIONS, DEFAULT_CHUNK_MARKER, DEFAULT_MAX_FILE_BYTES,
    DEFAULT_MAX_FILE_LINES, DEFAULT_SHORT_CHUNK_MERGE_MAX_CHARS, IndexedChunk, IndexedDocument,
};
use super::util::{has_zg_component, modified_unix_ms, stable_hash};

const DEFAULT_MAX_INDEX_WORKERS: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChunkEstimate {
    pub(crate) candidate_files: usize,
    pub(crate) indexable_files: usize,
    pub(crate) chunk_count: usize,
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

    let chunks = if supports_code_symbol_indexing(path) {
        build_symbol_chunks(path, &body)
    } else {
        build_chunks(&body)?
    };
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

pub(crate) fn load_indexable_documents(
    paths: &[PathBuf],
) -> ZgResult<Vec<Option<IndexedDocument>>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let worker_count = resolve_index_parallelism(
        paths.len(),
        configured_index_parallelism(),
        default_available_parallelism(),
    );
    if worker_count == 1 {
        return paths
            .iter()
            .map(|path| load_indexable_document(path))
            .collect();
    }

    let chunk_size = paths.len().div_ceil(worker_count);
    thread::scope(|scope| -> ZgResult<Vec<Option<IndexedDocument>>> {
        let mut handles = Vec::new();
        for batch_start in (0..paths.len()).step_by(chunk_size) {
            let batch_end = usize::min(batch_start + chunk_size, paths.len());
            let batch = &paths[batch_start..batch_end];
            handles.push(scope.spawn(
                move || -> ZgResult<Vec<(usize, Option<IndexedDocument>)>> {
                    batch
                        .iter()
                        .enumerate()
                        .map(|(offset, path)| {
                            Ok((batch_start + offset, load_indexable_document(path)?))
                        })
                        .collect()
                },
            ));
        }

        let mut loaded = Vec::with_capacity(paths.len());
        for handle in handles {
            let batch = handle
                .join()
                .map_err(|_| crate::other("index worker panicked"))??;
            loaded.extend(batch);
        }

        // Preserve the caller's original path order so downstream upsert/delete/warning
        // behavior stays deterministic even though file reads happen in parallel.
        loaded.sort_by_key(|(index, _)| *index);
        Ok(loaded.into_iter().map(|(_, document)| document).collect())
    })
}

pub(crate) fn estimate_indexable_chunks(scope: &Path) -> ZgResult<ChunkEstimate> {
    let candidate_files = collect_candidate_files(scope)?;
    let loaded_documents = load_indexable_documents(&candidate_files)?;
    let mut indexable_files = 0usize;
    let mut chunk_count = 0usize;

    for document in loaded_documents {
        if let Some(document) = document {
            indexable_files += 1;
            chunk_count += document.chunks.len();
        }
    }

    Ok(ChunkEstimate {
        candidate_files: candidate_files.len(),
        indexable_files,
        chunk_count,
    })
}

pub(crate) fn build_chunks(body: &str) -> ZgResult<Vec<IndexedChunk>> {
    let mut pending = Vec::<ShortTextChunk>::new();
    let mut base_chunks = Vec::<ShortTextChunk>::new();
    for (line_index, line) in body.lines().enumerate() {
        if line.trim().is_empty() {
            if !pending.is_empty() {
                base_chunks.append(&mut pending);
            }
            continue;
        }

        for raw_segment in line.split(DEFAULT_CHUNK_MARKER) {
            let raw_text = raw_segment.trim();
            if raw_text.is_empty() {
                continue;
            }

            let cleaned = strip_line_decorator(raw_text);
            if normalize_query(cleaned).is_empty() {
                continue;
            }

            pending.push(ShortTextChunk {
                line_start: line_index + 1,
                line_end: line_index + 1,
                raw_text: cleaned.to_string(),
            });
        }

        if !pending.is_empty() {
            base_chunks.append(&mut pending);
        }
    }

    if !pending.is_empty() {
        base_chunks.append(&mut pending);
    }

    let merged = merge_short_text_chunks(base_chunks);

    Ok(merged
        .into_iter()
        .enumerate()
        .filter_map(|(chunk_index, pending)| {
            let search_text = normalize_query(&pending.raw_text);
            if search_text.is_empty() {
                return None;
            }

            Some(IndexedChunk {
                chunk_index,
                line_start: pending.line_start,
                line_end: pending.line_end,
                raw_text: pending.raw_text,
                normalized_text: search_text.clone(),
                shared_normalized_text: search_text.clone(),
                shared_normalized_text_hash: stable_hash(search_text.as_bytes()),
                chunk_kind: "text".to_string(),
                language: None,
                symbol_kind: None,
                container: None,
            })
        })
        .collect())
}

fn merge_short_text_chunks(chunks: Vec<ShortTextChunk>) -> Vec<ShortTextChunk> {
    let mut merged = Vec::new();
    let mut idx = 0usize;
    while idx < chunks.len() {
        if chunks[idx].raw_text.chars().count() >= DEFAULT_SHORT_CHUNK_MERGE_MAX_CHARS {
            merged.push(chunks[idx].clone());
            idx += 1;
            continue;
        }

        let mut current = chunks[idx].clone();
        idx += 1;
        while idx < chunks.len()
            && chunks[idx].raw_text.chars().count() < DEFAULT_SHORT_CHUNK_MERGE_MAX_CHARS
        {
            current.line_end = chunks[idx].line_end;
            current.raw_text.push('\n');
            current.raw_text.push_str(&chunks[idx].raw_text);
            idx += 1;
        }
        merged.push(current);
    }
    merged
}

#[derive(Clone)]
struct ShortTextChunk {
    line_start: usize,
    line_end: usize,
    raw_text: String,
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
        || supports_code_symbol_indexing(path)
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

fn default_available_parallelism() -> usize {
    thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1)
}

fn configured_index_parallelism() -> Option<usize> {
    std::env::var("ZG_INDEX_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn resolve_index_parallelism(
    task_count: usize,
    configured_workers: Option<usize>,
    available_parallelism: usize,
) -> usize {
    if task_count <= 1 {
        return 1;
    }

    let default_workers = available_parallelism.min(DEFAULT_MAX_INDEX_WORKERS).max(1);
    configured_workers
        .unwrap_or(default_workers)
        .min(task_count)
        .max(1)
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

    use super::{
        build_chunks, collect_candidate_files, estimate_indexable_chunks, load_indexable_document,
        load_indexable_documents, resolve_index_parallelism,
    };

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
    fn candidate_collection_indexes_documents_and_supported_code_languages() {
        let root = temp_dir("document-only");
        fs::write(root.join("notes.md"), "note").unwrap();
        fs::write(root.join("journal.rst"), "entry").unwrap();
        fs::write(root.join("code.rs"), "fn main() {}").unwrap();
        fs::write(root.join("config.toml"), "name = 'zg'").unwrap();

        let files = rel_names(&root, collect_candidate_files(&root).unwrap());
        assert_eq!(files, vec!["code.rs", "journal.rst", "notes.md"]);
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

    #[test]
    fn concurrent_document_loading_preserves_input_order() {
        let root = temp_dir("parallel-order");
        let third = root.join("third.md");
        let first = root.join("first.md");
        let second = root.join("second.md");
        fs::write(&third, "third document").unwrap();
        fs::write(&first, "first document").unwrap();
        fs::write(&second, "second document").unwrap();

        let documents =
            load_indexable_documents(&[third.clone(), first.clone(), second.clone()]).unwrap();
        let snippets = documents
            .into_iter()
            .map(|document| {
                document
                    .unwrap()
                    .chunks
                    .first()
                    .unwrap()
                    .normalized_text
                    .clone()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            snippets,
            vec!["third document", "first document", "second document"]
        );
    }

    #[test]
    fn index_parallelism_is_bounded_and_overridable() {
        assert_eq!(resolve_index_parallelism(0, None, 12), 1);
        assert_eq!(resolve_index_parallelism(1, None, 12), 1);
        assert_eq!(resolve_index_parallelism(3, None, 12), 3);
        assert_eq!(resolve_index_parallelism(10, None, 12), 8);
        assert_eq!(resolve_index_parallelism(10, Some(2), 12), 2);
        assert_eq!(resolve_index_parallelism(10, Some(20), 12), 10);
    }

    #[test]
    fn chunk_estimate_counts_indexable_chunks() {
        let root = temp_dir("chunk-estimate");
        fs::write(root.join("a.md"), "alpha\nbeta\n").unwrap();
        fs::write(root.join("b.rs"), "fn parse_query() {}\n").unwrap();
        fs::write(root.join("skip.bin"), "ignored").unwrap();

        let estimate = estimate_indexable_chunks(&root).unwrap();
        assert_eq!(estimate.candidate_files, 2);
        assert_eq!(estimate.indexable_files, 2);
        assert!(estimate.chunk_count >= 2);
    }

    #[test]
    #[ignore = "debug helper to inspect emitted markdown chunks"]
    fn dump_markdown_chunks_from_r2_doc() {
        let body = fs::read_to_string("docs/r2_tech_decision_code_symbol_index.md").unwrap();
        let chunks = build_chunks(&body).unwrap();
        for chunk in chunks.into_iter().take(12) {
            println!("---");
            println!("raw_text: {}", chunk.raw_text);
            println!("search_text: {}", chunk.normalized_text);
            println!("embed_text: {}", chunk.shared_normalized_text);
            println!("line_start: {}", chunk.line_start);
            println!("line_end: {}", chunk.line_end);
        }
    }
}
