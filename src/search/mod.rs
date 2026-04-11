mod backend;
mod ripgrep_backend;

use std::path::Path;

use crate::ZgResult;
pub use backend::{GrepHit, ScanBackend};
use ripgrep_backend::RipgrepScanBackend;

pub fn regex_search(pattern: &str, root: &Path) -> ZgResult<Vec<GrepHit>> {
    RipgrepScanBackend.regex_search(root, pattern)
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
