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

pub fn bundled_embedding_model_dir() -> Option<PathBuf> {
    if let Some(path) = env::var_os("ZG_MODEL_DIR").map(PathBuf::from) {
        return Some(path);
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(prefix) = exe.parent().and_then(Path::parent) {
            let shared = prefix.join("share").join("zg").join("models");
            if shared.exists() {
                return Some(shared);
            }
        }
    }

    let app_support = macos_app_support_dir().ok()?.join("models");
    if app_support.exists() {
        return Some(app_support);
    }

    None
}

fn macos_app_support_dir() -> ZgResult<PathBuf> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| other("HOME is not set"))?;

    Ok(home.join("Library").join("Application Support").join("zg"))
}
