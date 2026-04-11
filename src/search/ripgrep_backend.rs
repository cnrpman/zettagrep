use std::path::{Component, Path};

use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;
use grep_searcher::sinks::UTF8;
use ignore::WalkBuilder;

use crate::ZgResult;
use crate::paths;
use crate::walk;

use super::backend::{GrepHit, ScanBackend};

pub struct RipgrepScanBackend;

impl ScanBackend for RipgrepScanBackend {
    fn regex_search(&self, root: &Path, pattern: &str) -> ZgResult<Vec<GrepHit>> {
        let root = paths::resolve_existing_path(root)?;
        let matcher = RegexMatcher::new(pattern)?;
        let mut hits = Vec::new();

        for path in scan_paths(&root)? {
            let mut searcher = SearcherBuilder::new()
                .line_number(true)
                .binary_detection(grep_searcher::BinaryDetection::quit(b'\x00'))
                .build();
            let mut sink = UTF8(|line_number, line| {
                hits.push(GrepHit {
                    path: path.clone(),
                    line_number: line_number as usize,
                    line: line.trim_end_matches('\n').to_string(),
                });
                Ok(true)
            });

            searcher.search_path(&matcher, &path, &mut sink)?;
        }

        Ok(hits)
    }
}

fn scan_paths(root: &Path) -> ZgResult<Vec<std::path::PathBuf>> {
    if root.is_file() {
        return Ok(if scan_file_allowed(root) {
            vec![root.to_path_buf()]
        } else {
            Vec::new()
        });
    }

    let mut builder = WalkBuilder::new(root);
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
        if file_type.is_file() && scan_file_allowed(path) {
            files.push(path.to_path_buf());
        }
    }

    Ok(files)
}

fn scan_file_allowed(path: &Path) -> bool {
    path.exists() && !has_zg_component(path)
}

fn has_zg_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::Normal(name) if name == ".zg"))
}

#[cfg(test)]
mod tests {
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
}
