use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;

use crate::paths;
use crate::{ZgResult, other};

use super::types::ScopeKind;

pub(crate) fn relative_path_string(root: &Path, path: &Path) -> ZgResult<String> {
    let rel_path = paths::relative_path(root, path).ok_or_else(|| {
        other(format!(
            "{} is not under {}",
            path.display(),
            root.display()
        ))
    })?;

    Ok(rel_path.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn has_zg_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::Normal(name) if name == ".zg"))
}

pub(crate) fn ancestor_index_root(root: &Path) -> Option<PathBuf> {
    let mut cursor = root.parent().map(Path::to_path_buf);
    while let Some(dir) = cursor {
        if paths::is_indexed_root(&dir) {
            return Some(dir);
        }
        cursor = dir.parent().map(Path::to_path_buf);
    }
    None
}

pub(crate) fn descendant_index_root(root: &Path) -> ZgResult<Option<PathBuf>> {
    let root = paths::resolve_existing_dir(root)?;
    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(false)
        .standard_filters(true)
        .max_depth(Some(8));

    for entry in builder.build() {
        let entry = entry?;
        let path = entry.path();
        if path == root {
            continue;
        }
        if entry.file_type().is_some_and(|kind| kind.is_dir()) && paths::is_indexed_root(path) {
            return Ok(Some(path.to_path_buf()));
        }
    }

    Ok(None)
}

pub(crate) fn stable_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn modified_unix_ms(metadata: &fs::Metadata) -> ZgResult<u64> {
    Ok(metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64)
}

pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub(crate) fn scope_kind(root: &Path, scope: &Path) -> ZgResult<ScopeKind> {
    if scope == root {
        return Ok(ScopeKind::Root);
    }

    let rel_path = relative_path_string(root, scope)?;
    if scope.is_file() {
        return Ok(ScopeKind::File(rel_path));
    }

    Ok(ScopeKind::Directory(
        rel_path.clone(),
        format!("{rel_path}/%"),
    ))
}
