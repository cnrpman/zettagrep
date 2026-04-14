use std::path::{Path, PathBuf};

use crate::ZgResult;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrepHit {
    pub path: PathBuf,
    pub line_number: usize,
    pub line: String,
}

pub trait ScanBackend {
    fn regex_search(&self, root: &Path, pattern: &str) -> ZgResult<Vec<GrepHit>>;
    fn literal_search(&self, root: &Path, pattern: &str) -> ZgResult<Vec<GrepHit>>;
}
