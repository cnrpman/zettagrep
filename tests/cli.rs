use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use zg::dev;

fn temp_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zg-cli-{name}-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn zg() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zg"));
    command.env("ZG_TEST_FAKE_EMBEDDINGS", "1");
    command
}

fn long_chunk_body(line_count: usize) -> String {
    (0..line_count)
        .map(|index| {
            format!("line {index:05} xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[test]
fn help_prints_dual_entry_usage() {
    let output = zg().arg("--help").output().unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Local-first search CLI for note-heavy directories."));
    assert!(stdout.contains("Usage: zg [OPTIONS] <QUERY> [PATH]"));
    assert!(stdout.contains("zg <COMMAND>"));
    assert!(stdout.contains("Regex-shaped input uses grep semantics immediately."));
    assert!(stdout.contains("Run regex search immediately with ripgrep semantics"));
    assert!(stdout.contains("Manage the local `.zg/` search index"));
    assert!(!stdout.contains("search  Run indexed plain-text"));
    assert!(stdout.contains("-A, --after-context <NUM>"));
    assert!(stdout.contains("-B, --before-context <NUM>"));
    assert!(stdout.contains("-C, --context <NUM>"));
    assert!(stdout.contains("Examples:"));
    assert!(stdout.contains("zg index init notes/"));
}

#[test]
fn subcommand_help_prints_explanatory_text() {
    let grep_output = zg().args(["grep", "--help"]).output().unwrap();
    assert!(grep_output.status.success());
    assert!(grep_output.stderr.is_empty());
    let grep_stdout = String::from_utf8(grep_output.stdout).unwrap();
    assert!(grep_stdout.contains("Run regex search immediately with ripgrep semantics"));
    assert!(grep_stdout.contains("Regex pattern passed through to ripgrep"));
    assert!(grep_stdout.contains("-A, --after-context <NUM>"));
    assert!(grep_stdout.contains("-B, --before-context <NUM>"));
    assert!(grep_stdout.contains("-C, --context <NUM>"));

    let init_output = zg().args(["index", "init", "--help"]).output().unwrap();
    assert!(init_output.status.success());
    assert!(init_output.stderr.is_empty());
    let init_stdout = String::from_utf8(init_output.stdout).unwrap();
    assert!(init_stdout.contains("Create a local `.zg/` index for a directory"));
    assert!(
        init_stdout.contains(
            "Index level to build: `fts` for lexical only, `fts+vector` for hybrid recall"
        )
    );
    assert!(init_stdout.contains("Skip the large-index sanity check"));
}

#[test]
fn grep_subcommand_searches_file_scope_end_to_end() {
    let root = temp_dir("grep-file");
    let file = root.join("note.md");
    fs::write(&file, "alpha\nneedle one\nneedle two\n").unwrap();

    let output = zg().args(["grep", "needle"]).arg(&file).output().unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            format!("{}:2:needle one", file.display()),
            format!("{}:3:needle two", file.display()),
        ]
    );
}

#[test]
fn default_entrypoint_uses_regex_mode_for_regex_shaped_queries() {
    let root = temp_dir("default-regex");
    let file = root.join("note.md");
    fs::write(&file, "alpha\nTODO item\nFIXME item\n").unwrap();

    let output = zg().arg("TODO|FIXME").arg(&file).output().unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            format!("{}:2:TODO item", file.display()),
            format!("{}:3:FIXME item", file.display()),
        ]
    );
}

#[test]
fn grep_subcommand_supports_context_flags() {
    let root = temp_dir("grep-context");
    let file = root.join("note.md");
    fs::write(&file, "alpha\nneedle one\nomega\n").unwrap();

    let output = zg()
        .args(["grep", "-C", "1", "needle"])
        .arg(&file)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            format!("{}:1:alpha", file.display()),
            format!("{}:2:needle one", file.display()),
            format!("{}:3:omega", file.display()),
        ]
    );
}

#[test]
fn default_entrypoint_passes_context_flags_to_regex_mode() {
    let root = temp_dir("default-regex-context");
    let file = root.join("note.md");
    fs::write(&file, "alpha\nTODO item\nomega\n").unwrap();

    let output = zg()
        .args(["-C", "1", "TODO|FIXME"])
        .arg(&file)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            format!("{}:1:alpha", file.display()),
            format!("{}:2:TODO item", file.display()),
            format!("{}:3:omega", file.display()),
        ]
    );
}

#[test]
fn index_delete_is_a_noop_without_local_cache() {
    let root = temp_dir("index-delete");

    let output = zg().args(["index", "delete"]).arg(&root).output().unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        format!("no local cache at {}", root.join(".zg").display())
    );
}

#[test]
fn missing_path_returns_non_zero_exit_and_error_message() {
    let root = temp_dir("missing-path");
    let missing = root.join("does-not-exist");

    let output = zg().arg("needle|alpha").arg(&missing).output().unwrap();

    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error: path does not exist:"));
    assert!(stderr.contains(&missing.display().to_string()));
}

#[test]
fn plain_search_without_explicit_index_is_actionable() {
    let root = temp_dir("missing-index");
    fs::write(root.join("note.md"), "plain search target").unwrap();

    let output = zg().arg("plain search").arg(&root).output().unwrap();

    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("zg: no ancestor .zg index found"));
    assert!(stderr.contains("zg index init --level fts"));
    assert!(stderr.contains("zg index init --level fts+vector"));
    assert!(!stderr.contains("error:"));
}

#[test]
fn schema_mismatch_search_failure_tells_user_to_rebuild() {
    let root = temp_dir("schema-mismatch");
    let file = root.join("note.md");
    fs::write(&file, "hello world").unwrap();

    let init = zg().args(["index", "init"]).arg(&root).output().unwrap();
    assert!(init.status.success());

    let conn = Connection::open(root.join(".zg/index.db")).unwrap();
    conn.execute(
        "UPDATE settings SET value = '5' WHERE key = 'schema_version'",
        [],
    )
    .unwrap();

    let output = zg().arg("plain search").arg(&root).output().unwrap();
    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("index schema/version mismatch"));
    assert!(stderr.contains("zg index rebuild"));
    assert!(stderr.starts_with("zg:"));
}

#[test]
fn indexed_search_prints_plain_rg_like_result_lines() {
    let root = temp_dir("indexed-output");
    fs::write(root.join("alpha.md"), "sqlite vector adapter\n").unwrap();

    let init = zg().args(["index", "init"]).arg(&root).output().unwrap();
    assert!(init.status.success());

    let output = zg().arg("sqlite adapter").arg(&root).output().unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], "alpha.md:[f] 1: sqlite vector adapter");
}

#[test]
fn indexed_search_supports_chunk_context_flags() {
    let root = temp_dir("indexed-context");
    fs::write(
        root.join("alpha.md"),
        "before chunk xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\nneedle context xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\nafter chunk xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n",
    )
    .unwrap();

    let init = zg().args(["index", "init"]).arg(&root).output().unwrap();
    assert!(init.status.success());

    let output = zg()
        .args(["-C", "1", "needle context"])
        .arg(&root)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        "alpha.md:[rf] 1-3: before chunk xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\nneedle context xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\nafter chunk xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n"
    );
}

#[test]
fn indexed_search_uses_ripgrep_literal_recall_for_marker_queries() {
    let root = temp_dir("indexed-literal-recall");
    fs::write(root.join("alpha.md"), "alpha :: beta\n").unwrap();

    let init = zg().args(["index", "init"]).arg(&root).output().unwrap();
    assert!(init.status.success());

    let output = zg().arg("::").arg(&root).output().unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines, vec!["alpha.md:[r] 1: alpha :: beta"]);
}

#[test]
fn indexed_search_uses_case_insensitive_ripgrep_literal_recall() {
    let root = temp_dir("indexed-literal-ignore-case");
    fs::write(
        root.join("alpha.md"),
        "see Docs/R0_Product_Philosophy.md for source\n",
    )
    .unwrap();

    let init = zg().args(["index", "init"]).arg(&root).output().unwrap();
    assert!(init.status.success());

    let output = zg()
        .arg("docs/r0_product_philosophy.md")
        .arg(&root)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec!["alpha.md:[rf] 1: see Docs/R0_Product_Philosophy.md for source"]
    );
}

#[test]
fn index_init_defaults_to_fts_level() {
    let root = temp_dir("default-level-cli");
    fs::write(root.join("alpha.md"), "sqlite vector adapter\n").unwrap();

    let init = zg().args(["index", "init"]).arg(&root).output().unwrap();
    assert!(init.status.success());

    let status = zg().args(["index", "status"]).arg(&root).output().unwrap();
    assert!(status.status.success());

    let stdout = String::from_utf8(status.stdout).unwrap();
    assert!(stdout.contains("index level: fts"));
    assert!(stdout.contains("vector ready: no"));
}

#[test]
fn vector_index_init_prints_wait_note_to_stderr() {
    let root = temp_dir("vector-init-note");
    fs::write(root.join("alpha.md"), "sqlite vector adapter\n").unwrap();

    let output = zg()
        .args(["index", "init", "--level", "fts+vector"])
        .arg(&root)
        .output()
        .unwrap();

    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("starting `fts+vector` index init"));
    assert!(stderr.contains("may take a while"));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("initialized"));
    assert!(stdout.contains("level=fts+vector"));
}

#[test]
fn oversized_vector_init_requires_force() {
    let root = temp_dir("vector-init-force-required");
    fs::write(root.join("alpha.md"), long_chunk_body(10_241)).unwrap();

    let output = zg()
        .args(["index", "init", "--level", "fts+vector"])
        .arg(&root)
        .output()
        .unwrap();

    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("estimates 10241 chunks"));
    assert!(stderr.contains("> 10240 chunks (10x) requires `--force`"));
    assert!(stderr.contains("zg index init --level fts+vector --force"));
}

#[test]
fn vector_index_rebuild_prints_wait_note_to_stderr() {
    let root = temp_dir("vector-rebuild-note");
    fs::write(root.join("alpha.md"), "sqlite vector adapter\n").unwrap();

    let init = zg().args(["index", "init"]).arg(&root).output().unwrap();
    assert!(init.status.success());

    let output = zg()
        .args(["index", "rebuild", "--level", "fts+vector"])
        .arg(&root)
        .output()
        .unwrap();

    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("starting `fts+vector` index rebuild"));
    assert!(stderr.contains("may take a while"));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("rebuilt"));
    assert!(stdout.contains("level=fts+vector"));
}

#[test]
fn dev_probe_chunks_emits_json_report() {
    let root = temp_dir("dev-probe-chunks");
    let file = root.join("note.md");
    fs::write(&file, "- alpha\nbeta :: gamma\n").unwrap();

    let output = zg()
        .args(["dev", "probe", "chunks"])
        .arg(&file)
        .arg("--json")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"chunk_count\": 1"));
    assert!(stdout.contains("alpha\\nbeta\\ngamma"));
}

#[test]
fn dev_eval_search_quality_accepts_matching_fixture_and_golden() {
    let root = temp_dir("dev-eval-quality");
    fs::write(root.join("alpha.md"), "sqlite vector adapter").unwrap();

    let fixture = root.join("fixture.json");
    let golden = root.join("golden.json");
    fs::write(
        &fixture,
        serde_json::to_string_pretty(&serde_json::json!({
            "suite_id": "mini",
            "sample_vault_manifest": "sample.json",
            "default_limit": 2,
            "cases": [
                {
                    "id": "sqlite-adapter",
                    "query": "sqlite adapter",
                    "expectations": {
                        "must_include": [
                            {
                                "path": "alpha.md",
                                "within_top": 1,
                                "snippet_contains": "sqlite vector adapter"
                            }
                        ]
                    }
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    dev::write_search_quality_golden(&fixture, &golden, &root).unwrap();

    let output = zg()
        .args(["dev", "eval", "search-quality"])
        .arg("--fixture")
        .arg(&fixture)
        .arg("--golden")
        .arg(&golden)
        .arg("--vault")
        .arg(&root)
        .arg("--json")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"passed_cases\": 1"));
    assert!(stdout.contains("\"expectation_failures\": 0"));
    assert!(stdout.contains("\"golden_failures\": 0"));
}

#[test]
fn dev_bench_sample_vault_runs_on_tiny_fixture() {
    let root = temp_dir("dev-bench");
    fs::write(root.join("alpha.md"), "sqlite vector adapter\n").unwrap();
    fs::write(root.join("beta.md"), "haystack builder\n").unwrap();

    let fixture = root.join("fixture.json");
    fs::write(
        &fixture,
        serde_json::to_string_pretty(&serde_json::json!({
            "suite_id": "bench-mini",
            "sample_vault_manifest": "sample.json",
            "default_limit": 2,
            "cases": [
                {
                    "id": "sqlite-adapter",
                    "query": "sqlite adapter"
                },
                {
                    "id": "haystack-builder",
                    "query": "haystack builder"
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let output = zg()
        .args(["dev", "bench", "sample-vault"])
        .arg("--fixture")
        .arg(&fixture)
        .arg("--vault")
        .arg(&root)
        .arg("--fake-embeddings")
        .arg("--json")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"fake_embeddings\": true"));
    assert!(stdout.contains("\"level\": \"fts\""));
    assert!(stdout.contains("\"level\": \"fts+vector\""));
    assert!(stdout.contains("\"query_total_elapsed_ms\""));
}
