use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{ZgResult, other};

pub fn resolve_existing_dir(path: &Path) -> ZgResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };

    if !candidate.exists() {
        return Err(other(format!(
            "path does not exist: {}",
            candidate.display()
        )));
    }

    if !fs::metadata(&candidate)?.is_dir() {
        return Err(other(format!(
            "path is not a directory: {}",
            candidate.display()
        )));
    }

    Ok(candidate)
}

pub fn resolve_existing_path(path: &Path) -> ZgResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };

    if !candidate.exists() {
        return Err(other(format!(
            "path does not exist: {}",
            candidate.display()
        )));
    }

    Ok(candidate)
}

pub fn current_dir() -> ZgResult<PathBuf> {
    Ok(env::current_dir()?)
}

pub fn hidden_dir(root: &Path) -> PathBuf {
    root.join(".zg")
}

pub fn db_path(root: &Path) -> PathBuf {
    hidden_dir(root).join("index.db")
}

pub fn state_path(root: &Path) -> PathBuf {
    hidden_dir(root).join("state.json")
}

pub fn ensure_hidden_dir(root: &Path) -> ZgResult<PathBuf> {
    let hidden = hidden_dir(root);
    fs::create_dir_all(&hidden)?;
    Ok(hidden)
}

pub fn is_indexed_root(root: &Path) -> bool {
    db_path(root).exists()
}

pub fn find_index_root(path: &Path) -> Option<PathBuf> {
    let mut cursor = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };

    loop {
        if is_indexed_root(&cursor) {
            return Some(cursor);
        }

        if !cursor.pop() {
            return None;
        }
    }
}

pub fn covering_index_roots(path: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut cursor = if path.is_dir() {
        Some(path.to_path_buf())
    } else {
        path.parent().map(Path::to_path_buf)
    };

    while let Some(dir) = cursor {
        if is_indexed_root(&dir) {
            roots.push(dir.clone());
        }
        cursor = dir.parent().map(Path::to_path_buf);
    }

    roots
}

pub fn relative_path(root: &Path, path: &Path) -> Option<PathBuf> {
    path.strip_prefix(root).ok().map(Path::to_path_buf)
}
