use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use grep::regex::RegexMatcher;
use grep::searcher::sinks::UTF8;
use grep::searcher::{BinaryDetection, SearcherBuilder};
use ignore::{DirEntry, WalkBuilder, WalkState};

use crate::paths;
use crate::walk;
use crate::{DynError, ZgResult, other};

use super::backend::{GrepHit, ScanBackend};

pub struct RipgrepScanBackend;

impl ScanBackend for RipgrepScanBackend {
    fn regex_search(&self, root: &Path, pattern: &str) -> ZgResult<Vec<GrepHit>> {
        let root = paths::resolve_existing_path(root)?;
        let matcher = RegexMatcher::new(pattern)?;

        let mut hits = if root.is_file() {
            let mut searcher = build_searcher();
            let mut hits = Vec::new();
            if scan_file_allowed(&root) {
                search_path(&mut searcher, &matcher, &root, &mut hits)?;
            }
            hits
        } else {
            search_paths_parallel(&root, matcher)?
        };
        hits.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.line_number.cmp(&right.line_number))
                .then_with(|| left.line.cmp(&right.line))
        });

        Ok(hits)
    }
}

fn search_paths_parallel(root: &Path, matcher: RegexMatcher) -> ZgResult<Vec<GrepHit>> {
    let shared = Arc::new(SharedSearchState::default());
    let mut builder = WalkBuilder::new(root);
    walk::apply_content_filters(&mut builder);

    builder.build_parallel().visit(&mut SearchVisitorBuilder {
        matcher,
        shared: Arc::clone(&shared),
    });

    if let Some(error) = shared.take_error()? {
        return Err(error);
    }

    shared.into_hits()
}

fn build_searcher() -> grep::searcher::Searcher {
    SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .build()
}

fn search_path(
    searcher: &mut grep::searcher::Searcher,
    matcher: &RegexMatcher,
    path: &Path,
    hits: &mut Vec<GrepHit>,
) -> ZgResult<()> {
    let path = path.to_path_buf();
    let mut sink = UTF8(|line_number, line| {
        hits.push(GrepHit {
            path: path.clone(),
            line_number: line_number as usize,
            line: line.trim_end_matches('\n').to_string(),
        });
        Ok(true)
    });
    searcher.search_path(matcher, &path, &mut sink)?;
    Ok(())
}

#[derive(Default)]
struct SharedSearchState {
    hits: Mutex<Vec<GrepHit>>,
    error: Mutex<Option<DynError>>,
    failed: AtomicBool,
}

impl SharedSearchState {
    fn record_error(&self, error: DynError) {
        self.failed.store(true, Ordering::Relaxed);
        if let Ok(mut slot) = self.error.lock() {
            if slot.is_none() {
                *slot = Some(error);
            }
        }
    }

    fn has_error(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    fn merge_hits(&self, mut hits: Vec<GrepHit>) -> ZgResult<()> {
        let mut shared = self
            .hits
            .lock()
            .map_err(|_| other("parallel regex hit buffer lock poisoned"))?;
        shared.append(&mut hits);
        Ok(())
    }

    fn take_error(&self) -> ZgResult<Option<DynError>> {
        let mut slot = self
            .error
            .lock()
            .map_err(|_| other("parallel regex error lock poisoned"))?;
        Ok(slot.take())
    }

    fn into_hits(self: Arc<Self>) -> ZgResult<Vec<GrepHit>> {
        match Arc::try_unwrap(self) {
            Ok(state) => state
                .hits
                .into_inner()
                .map_err(|_| other("parallel regex hit buffer lock poisoned")),
            Err(_) => Err(other("parallel regex state still has active references")),
        }
    }
}

struct SearchVisitorBuilder {
    matcher: RegexMatcher,
    shared: Arc<SharedSearchState>,
}

impl<'s> ignore::ParallelVisitorBuilder<'s> for SearchVisitorBuilder {
    fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
        Box::new(SearchVisitor {
            matcher: self.matcher.clone(),
            searcher: build_searcher(),
            shared: Arc::clone(&self.shared),
            local_hits: Vec::new(),
        })
    }
}

struct SearchVisitor {
    matcher: RegexMatcher,
    searcher: grep::searcher::Searcher,
    shared: Arc<SharedSearchState>,
    local_hits: Vec<GrepHit>,
}

impl ignore::ParallelVisitor for SearchVisitor {
    fn visit(&mut self, entry: Result<DirEntry, ignore::Error>) -> WalkState {
        if self.shared.has_error() {
            return WalkState::Quit;
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                self.shared.record_error(error.into());
                return WalkState::Quit;
            }
        };

        let path = entry.path();
        let Some(file_type) = entry.file_type() else {
            return WalkState::Continue;
        };

        if file_type.is_symlink() || has_zg_component(path) {
            return WalkState::Continue;
        };
        if !file_type.is_file() || !scan_file_allowed(path) {
            return WalkState::Continue;
        }

        match search_path(
            &mut self.searcher,
            &self.matcher,
            path,
            &mut self.local_hits,
        ) {
            Ok(()) => WalkState::Continue,
            Err(error) => {
                self.shared.record_error(error);
                WalkState::Quit
            }
        }
    }
}

impl Drop for SearchVisitor {
    fn drop(&mut self) {
        if self.local_hits.is_empty() {
            return;
        }

        let local_hits = std::mem::take(&mut self.local_hits);
        if let Err(error) = self.shared.merge_hits(local_hits) {
            self.shared.record_error(error);
        }
    }
}

fn scan_file_allowed(path: &Path) -> bool {
    !has_zg_component(path)
}

fn has_zg_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::Normal(name) if name == ".zg"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::RipgrepScanBackend;
    use crate::search::backend::ScanBackend;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zg-scan-{name}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn regex_search_uses_ripgrep_style_visibility_rules() {
        let root = temp_dir("visibility");
        let child = root.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::write(root.join(".ignore"), "ignored.md\n").unwrap();
        fs::write(child.join(".hidden.md"), "needle hidden").unwrap();
        fs::write(child.join("ignored.md"), "needle ignored").unwrap();
        fs::write(child.join("keep.md"), "needle visible").unwrap();

        let hits = RipgrepScanBackend.regex_search(&child, "needle").unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("keep.md"));
    }

    #[test]
    fn regex_search_returns_hits_sorted_by_path_and_line_number() {
        let root = temp_dir("sorted");
        fs::write(root.join("b.md"), "needle second\nneedle third").unwrap();
        fs::write(root.join("a.md"), "needle first").unwrap();

        let hits = RipgrepScanBackend.regex_search(&root, "needle").unwrap();
        let rendered = hits
            .iter()
            .map(|hit| {
                format!(
                    "{}:{}:{}",
                    hit.path.file_name().unwrap().to_string_lossy(),
                    hit.line_number,
                    hit.line
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "a.md:1:needle first",
                "b.md:1:needle second",
                "b.md:2:needle third",
            ]
        );
    }

    #[test]
    fn regex_search_on_file_scope_preserves_matching_lines() {
        let root = temp_dir("file-scope");
        let file = root.join("note.md");
        fs::write(&file, "alpha\nneedle one\nbeta\nneedle two").unwrap();

        let hits = RipgrepScanBackend.regex_search(&file, "needle").unwrap();
        let lines = hits
            .iter()
            .map(|hit| (hit.line_number, hit.line.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(lines, vec![(2, "needle one"), (4, "needle two")]);
        assert!(hits.iter().all(|hit| hit.path == file));
    }

    #[test]
    fn regex_search_visits_each_matching_file_once_under_parallel_walk() {
        let root = temp_dir("parallel");
        for idx in 0..32 {
            fs::write(
                root.join(format!("note-{idx:02}.md")),
                format!("prefix\nneedle {idx}\nsuffix\n"),
            )
            .unwrap();
        }

        let hits = RipgrepScanBackend.regex_search(&root, "needle").unwrap();
        assert_eq!(hits.len(), 32);

        let files = hits
            .into_iter()
            .map(|hit| hit.path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(files.len(), 32);
    }
}
