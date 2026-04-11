use std::path::{Path, PathBuf};

use crate::index;
use crate::{Query, ZgResult};
use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;
use grep_searcher::sinks::UTF8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrepHit {
    pub path: PathBuf,
    pub line_number: usize,
    pub line: String,
}

pub fn regex_search(pattern: &str, root: &Path) -> ZgResult<Vec<GrepHit>> {
    let matcher = RegexMatcher::new(pattern)?;
    search_with_matcher(root, matcher)
}

pub fn fallback_query_search(query: &str, root: &Path) -> ZgResult<Vec<GrepHit>> {
    let query = Query::new(query);
    let mut hits = Vec::new();

    for path in index::collect_scan_files(root)? {
        let raw = std::fs::read_to_string(&path);
        let Ok(raw) = raw else {
            continue;
        };

        for (line_number, line) in raw.lines().enumerate() {
            if query.matches(line) {
                hits.push(GrepHit {
                    path: path.clone(),
                    line_number: line_number + 1,
                    line: line.to_string(),
                });
            }
        }
    }

    Ok(hits)
}

pub fn is_probably_regex(input: &str) -> bool {
    input.contains('\\')
        || input.contains('[')
        || input.contains(']')
        || input.contains('(')
        || input.contains(')')
        || input.contains('{')
        || input.contains('}')
        || input.contains('|')
        || input.contains('^')
        || input.contains('$')
        || input.contains(".*")
        || input.contains(".+")
        || input.contains(".?")
}

fn search_with_matcher(root: &Path, matcher: RegexMatcher) -> ZgResult<Vec<GrepHit>> {
    let mut hits = Vec::new();

    for path in index::collect_scan_files(root)? {
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
