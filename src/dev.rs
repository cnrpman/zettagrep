use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::index;
use crate::paths;
use crate::{ZgResult, other};

pub const DEFAULT_SAMPLE_VAULT_MANIFEST: &str =
    "resources/sample-vaults/ripgrep-14.1.1.json";
pub const DEFAULT_SEARCH_QUALITY_FIXTURE: &str =
    "resources/search-quality/ripgrep-14.1.1.fixtures.json";
pub const DEFAULT_SEARCH_QUALITY_GOLDEN: &str =
    "resources/search-quality/ripgrep-14.1.1.golden.json";

pub use index::{
    ChunkProbeReport, DbCacheProbeReport, SearchQualityFixtureSuite, SearchQualityGoldenSuite,
    SearchQualityReport, load_search_quality_fixture, load_search_quality_golden, probe_chunks,
    probe_db_cache, run_search_quality_suite, write_search_quality_golden,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SampleVaultManifest {
    pub id: String,
    pub description: String,
    pub repository: String,
    pub tag: String,
    pub commit: String,
    pub checkout_dir: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct EnsuredSampleVault {
    pub manifest_path: PathBuf,
    pub id: String,
    pub path: PathBuf,
    pub commit: String,
    pub status: String,
}

pub fn load_sample_vault_manifest(path: &Path) -> ZgResult<SampleVaultManifest> {
    let path = paths::resolve_existing_path(path)?;
    let bytes = fs::read(&path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn ensure_sample_vault(path: &Path, force: bool) -> ZgResult<EnsuredSampleVault> {
    let manifest_path = paths::resolve_existing_path(path)?;
    let manifest = load_sample_vault_manifest(&manifest_path)?;
    let checkout_dir = resolve_checkout_dir(&manifest_path, &manifest.checkout_dir);

    if checkout_dir.exists() {
        let head = git_head(&checkout_dir)?;
        if head == manifest.commit {
            return Ok(EnsuredSampleVault {
                manifest_path,
                id: manifest.id,
                path: checkout_dir,
                commit: head,
                status: "ready".to_string(),
            });
        }

        if !force {
            return Err(other(format!(
                "sample vault checkout at {} is on commit {}, expected {}; rerun with --force to replace it",
                checkout_dir.display(),
                head,
                manifest.commit
            )));
        }

        fs::remove_dir_all(&checkout_dir)?;
    }

    if let Some(parent) = checkout_dir.parent() {
        fs::create_dir_all(parent)?;
    }

    let clone_status = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--branch")
        .arg(&manifest.tag)
        .arg(&manifest.repository)
        .arg(&checkout_dir)
        .status()?;
    if !clone_status.success() {
        return Err(other(format!(
            "failed to clone sample vault from {} tag {}",
            manifest.repository, manifest.tag
        )));
    }

    let head = git_head(&checkout_dir)?;
    if head != manifest.commit {
        return Err(other(format!(
            "sample vault cloned to {}, but HEAD is {} instead of expected {}",
            checkout_dir.display(),
            head,
            manifest.commit
        )));
    }

    Ok(EnsuredSampleVault {
        manifest_path,
        id: manifest.id,
        path: checkout_dir,
        commit: head,
        status: "cloned".to_string(),
    })
}

pub fn resolve_fixture_vault(fixture_path: &Path, override_vault: Option<&Path>) -> ZgResult<PathBuf> {
    if let Some(path) = override_vault {
        return paths::resolve_existing_dir(path);
    }

    let fixture_path = paths::resolve_existing_path(fixture_path)?;
    let fixture = load_search_quality_fixture(&fixture_path)?;
    let manifest_path = fixture_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&fixture.sample_vault_manifest);
    Ok(ensure_sample_vault(&manifest_path, false)?.path)
}

fn resolve_checkout_dir(manifest_path: &Path, checkout_dir: &str) -> PathBuf {
    let candidate = PathBuf::from(checkout_dir);
    if candidate.is_absolute() {
        candidate
    } else {
        manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(candidate)
    }
}

fn git_output(args: &[&str]) -> ZgResult<String> {
    let output = Command::new("git").args(args).output()?;
    if !output.status.success() {
        return Err(other(format!(
            "git {} failed with status {}",
            args.join(" "),
            output.status
        )));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn git_head(path: &Path) -> ZgResult<String> {
    let checkout_arg = path.display().to_string();
    git_output(&["-C", &checkout_arg, "rev-parse", "HEAD"])
}
