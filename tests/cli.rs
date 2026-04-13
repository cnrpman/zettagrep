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

#[test]
fn help_prints_dual_entry_usage() {
    let output = zg().arg("--help").output().unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage: zg <QUERY> [PATH]"));
    assert!(stdout.contains("zg <COMMAND>"));
    assert!(stdout.contains("grep"));
    assert!(stdout.contains("search"));
    assert!(stdout.contains("index"));
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
fn implicit_init_large_scope_refusal_is_actionable() {
    let root = temp_dir("implicit-guard");
    for idx in 0..2000 {
        fs::write(root.join(format!("note-{idx:04}.md")), "note").unwrap();
    }

    let output = zg().arg("plain search").arg(&root).output().unwrap();

    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("zg: refusing to auto-create index"));
    assert!(stderr.contains("2000 files"));
    assert!(stderr.contains("zg index init"));
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
fn indexed_search_prints_stable_result_line_shape() {
    let root = temp_dir("indexed-output");
    fs::write(root.join("alpha.md"), "sqlite vector adapter\n").unwrap();

    let init = zg().args(["index", "init"]).arg(&root).output().unwrap();
    assert!(init.status.success());

    let output = zg()
        .args(["search", "sqlite adapter"])
        .arg(&root)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("alpha.md  score="));
    assert!(lines[0].contains("  lexical="));
    assert!(lines[0].contains("  vector="));
    assert!(lines[0].ends_with("sqlite vector adapter"));
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
fn indexed_search_stays_available_while_another_writer_holds_the_db_lock() {
    let root = temp_dir("indexed-reader-while-write-locked");
    fs::write(root.join("alpha.md"), "sqlite vector adapter\n").unwrap();

    let init = zg().args(["index", "init"]).arg(&root).output().unwrap();
    assert!(init.status.success());

    let writer = Connection::open(root.join(".zg/index.db")).unwrap();
    writer
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             BEGIN IMMEDIATE;
             UPDATE state SET dirty = dirty WHERE id = 1;",
        )
        .unwrap();

    let output = zg()
        .args(["search", "sqlite adapter"])
        .arg(&root)
        .output()
        .unwrap();

    writer.execute_batch("ROLLBACK").unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("alpha.md  score="));
    assert!(lines[0].ends_with("sqlite vector adapter"));
}
