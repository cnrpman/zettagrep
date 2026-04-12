mod backend;
mod ripgrep_backend;

use std::path::Path;

use regex_syntax::{
    hir::{Hir, HirKind},
    parse,
};

use crate::ZgResult;
pub use backend::{GrepHit, ScanBackend};
use ripgrep_backend::RipgrepScanBackend;

pub fn regex_search(pattern: &str, root: &Path) -> ZgResult<Vec<GrepHit>> {
    RipgrepScanBackend.regex_search(root, pattern)
}

pub fn is_probably_regex(input: &str) -> bool {
    if !contains_regex_signal(input) {
        return false;
    }
    if input.contains('\\') {
        return true;
    }

    match parse(input) {
        Ok(hir) => hir_contains_regex_semantics(&hir),
        Err(_) => contains_structural_regex_signal(input),
    }
}

fn contains_regex_signal(input: &str) -> bool {
    contains_structural_regex_signal(input)
        || input.contains(".*")
        || input.contains(".+")
        || input.contains(".?")
}

fn contains_structural_regex_signal(input: &str) -> bool {
    input.chars().any(|ch| {
        matches!(
            ch,
            '[' | ']' | '(' | ')' | '{' | '}' | '|' | '^' | '$' | '\\'
        )
    })
}

fn hir_contains_regex_semantics(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Empty | HirKind::Literal(_) => false,
        HirKind::Class(_)
        | HirKind::Look(_)
        | HirKind::Repetition(_)
        | HirKind::Capture(_)
        | HirKind::Alternation(_) => true,
        HirKind::Concat(parts) => parts.iter().any(hir_contains_regex_semantics),
    }
}

#[cfg(test)]
mod tests {
    use super::is_probably_regex;

    #[test]
    fn alternation_is_treated_as_regex() {
        assert!(is_probably_regex("TODO|FIXME"));
    }

    #[test]
    fn c_plus_plus_stays_plain_text() {
        assert!(!is_probably_regex("C++"));
    }

    #[test]
    fn dotted_versions_stay_plain_text() {
        assert!(!is_probably_regex("v1.2.3"));
    }

    #[test]
    fn repetition_with_braces_is_treated_as_regex() {
        assert!(is_probably_regex("colou?r{1,2}"));
    }
}
