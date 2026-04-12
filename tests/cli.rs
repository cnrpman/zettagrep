use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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
    Command::new(env!("CARGO_BIN_EXE_zg"))
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
