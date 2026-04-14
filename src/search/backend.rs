use std::path::{Path, PathBuf};

use crate::ZgResult;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SearchContext {
    pub before: usize,
    pub after: usize,
}

impl SearchContext {
    pub fn has_context(self) -> bool {
        self.before > 0 || self.after > 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrepHit {
    pub path: PathBuf,
    pub line_number: usize,
    pub line: String,
}

pub trait ScanBackend {
    fn regex_search(
        &self,
        root: &Path,
        pattern: &str,
        context: SearchContext,
    ) -> ZgResult<Vec<GrepHit>>;
    fn literal_search(&self, root: &Path, pattern: &str) -> ZgResult<Vec<GrepHit>>;
}
