use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::index;
use crate::paths;
use crate::{ZgResult, other};

pub const DEFAULT_SAMPLE_VAULT_MANIFEST: &str = "resources/sample-vaults/ripgrep-14.1.1.json";
pub const DEFAULT_SEARCH_QUALITY_FIXTURE: &str =
    "resources/search-quality/ripgrep-14.1.1.fixtures.json";
pub const DEFAULT_SEARCH_QUALITY_GOLDEN: &str =
    "resources/search-quality/ripgrep-14.1.1.golden.json";

pub use index::{
    ChunkProbeReport, DbCacheProbeReport, SearchQualityFixtureSuite, SearchQualityGoldenSuite,
    SearchQualityReport, load_search_quality_fixture, load_search_quality_golden, probe_chunks,
    probe_db_cache, run_search_quality_suite, write_search_quality_golden,
};
use index::{IndexLevel, load_status};

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

#[derive(Clone, Debug, Serialize)]
pub struct SampleVaultBenchmarkReport {
    pub fixture_path: PathBuf,
    pub source_vault: PathBuf,
    pub scratch_root: PathBuf,
    pub fake_embeddings: bool,
    pub repeat: usize,
    pub levels: Vec<SampleVaultBenchmarkLevelReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SampleVaultBenchmarkLevelReport {
    pub level: IndexLevel,
    pub scratch_vault: PathBuf,
    pub init_elapsed_ms: u64,
    pub status_file_count: u64,
    pub status_chunk_count: u64,
    pub query_total_elapsed_ms: u64,
    pub query_mean_elapsed_ms: u64,
    pub query_p50_elapsed_ms: u64,
    pub query_p95_elapsed_ms: u64,
    pub cases: Vec<SampleVaultBenchmarkCaseReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SampleVaultBenchmarkCaseReport {
    pub id: String,
    pub query: String,
    pub scope: Option<String>,
    pub repeat_index: usize,
    pub elapsed_ms: u64,
    pub exit_code: i32,
    pub stdout_lines: usize,
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

pub fn resolve_fixture_vault(
    fixture_path: &Path,
    override_vault: Option<&Path>,
) -> ZgResult<PathBuf> {
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

pub fn run_sample_vault_benchmark(
    exe_path: &Path,
    fixture_path: &Path,
    override_vault: Option<&Path>,
    fake_embeddings: bool,
    repeat: usize,
    keep_scratch: bool,
    out_path: Option<&Path>,
) -> ZgResult<SampleVaultBenchmarkReport> {
    let fixture_path = paths::resolve_existing_path(fixture_path)?;
    let fixture = load_search_quality_fixture(&fixture_path)?;
    let source_vault = resolve_fixture_vault(&fixture_path, override_vault)?;
    let scratch_root = benchmark_scratch_root()?;
    fs::create_dir_all(&scratch_root)?;

    let mut levels = Vec::new();
    for level in [IndexLevel::Fts, IndexLevel::FtsVector] {
        let scratch_vault = scratch_root.join(level.as_str().replace('+', "-"));
        copy_tree_excluding(&source_vault, &scratch_vault, &[".git", ".zg"])?;

        let init_elapsed_ms = run_timed_command(
            exe_path,
            &[
                "index",
                "init",
                "--level",
                level.as_str(),
                &scratch_vault.display().to_string(),
            ],
            fake_embeddings,
        )?
        .elapsed_ms;
        let status = load_status(&scratch_vault)?;

        let mut cases = Vec::new();
        let mut query_times = Vec::new();
        for repeat_index in 0..repeat.max(1) {
            for case in &fixture.cases {
                let scope = case
                    .scope
                    .as_ref()
                    .map(|relative| scratch_vault.join(relative))
                    .unwrap_or_else(|| scratch_vault.clone());
                let command = run_timed_command(
                    exe_path,
                    &[
                        "search",
                        &case.query,
                        &scope.display().to_string(),
                    ],
                    fake_embeddings,
                )?;
                query_times.push(command.elapsed_ms);
                cases.push(SampleVaultBenchmarkCaseReport {
                    id: case.id.clone(),
                    query: case.query.clone(),
                    scope: case.scope.clone(),
                    repeat_index: repeat_index + 1,
                    elapsed_ms: command.elapsed_ms,
                    exit_code: command.exit_code,
                    stdout_lines: command.stdout_lines,
                });
            }
        }

        let (mean, p50, p95, total) = summarize_elapsed_ms(&query_times);
        levels.push(SampleVaultBenchmarkLevelReport {
            level,
            scratch_vault,
            init_elapsed_ms,
            status_file_count: status.file_count,
            status_chunk_count: status.chunk_count,
            query_total_elapsed_ms: total,
            query_mean_elapsed_ms: mean,
            query_p50_elapsed_ms: p50,
            query_p95_elapsed_ms: p95,
            cases,
        });
    }

    let report = SampleVaultBenchmarkReport {
        fixture_path,
        source_vault,
        scratch_root: scratch_root.clone(),
        fake_embeddings,
        repeat: repeat.max(1),
        levels,
    };
    if let Some(out_path) = out_path {
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(out_path, serde_json::to_string_pretty(&report)?)?;
    }
    if !keep_scratch {
        let _ = fs::remove_dir_all(&scratch_root);
    }
    Ok(report)
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

fn benchmark_scratch_root() -> ZgResult<PathBuf> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(std::env::temp_dir().join(format!("zg-bench-{unique}")))
}

fn copy_tree_excluding(src: &Path, dst: &Path, excluded_names: &[&str]) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let file_name = entry.file_name();
        if excluded_names.iter().any(|name| file_name == OsStr::new(name)) {
            continue;
        }

        let dst_path = dst.join(&file_name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_tree_excluding(&src_path, &dst_path, excluded_names)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct TimedCommand {
    elapsed_ms: u64,
    exit_code: i32,
    stdout_lines: usize,
}

fn run_timed_command(exe_path: &Path, args: &[&str], fake_embeddings: bool) -> ZgResult<TimedCommand> {
    let started = Instant::now();
    let mut command = Command::new(exe_path);
    command.args(args);
    if fake_embeddings {
        command.env("ZG_TEST_FAKE_EMBEDDINGS", "1");
    } else {
        command.env_remove("ZG_TEST_FAKE_EMBEDDINGS");
    }
    let output = command.output()?;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let exit_code = output.status.code().unwrap_or(-1);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(other(format!(
            "benchmark command `{}` failed with status {}: {}",
            args.join(" "),
            output.status,
            stderr.trim()
        )));
    }
    let stdout_lines = String::from_utf8_lossy(&output.stdout).lines().count();
    Ok(TimedCommand {
        elapsed_ms,
        exit_code,
        stdout_lines,
    })
}

fn summarize_elapsed_ms(values: &[u64]) -> (u64, u64, u64, u64) {
    if values.is_empty() {
        return (0, 0, 0, 0);
    }
    let total = values.iter().sum::<u64>();
    let mean = total / values.len() as u64;
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let p50 = percentile(&sorted, 0.50);
    let p95 = percentile(&sorted, 0.95);
    (mean, p50, p95, total)
}

fn percentile(sorted: &[u64], quantile: f64) -> u64 {
    let index = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[index]
}
