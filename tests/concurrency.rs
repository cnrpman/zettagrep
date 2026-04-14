use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

fn temp_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zg-concurrency-{name}-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn zg() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zg"));
    command.env("ZG_TEST_FAKE_EMBEDDINGS", "1");
    command
}

fn count_embed_records(path: &std::path::Path, prefix: &str) -> usize {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.split('\t').next() == Some(prefix))
        .count()
}

fn wait_for_embed_record(path: &std::path::Path, prefix: &str, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if count_embed_records(path, prefix) >= expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} {prefix} embed records in {}",
            path.display()
        );
        sleep(Duration::from_millis(10));
    }
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

    let output = zg().arg("sqlite adapter").arg(&root).output().unwrap();

    writer.execute_batch("ROLLBACK").unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], "alpha.md:[f] 1: sqlite vector adapter");
}

#[test]
fn concurrent_dirty_searches_do_not_duplicate_passage_embeddings() {
    let root = temp_dir("dirty-search-dedup");
    let file = root.join("alpha.md");
    fs::write(&file, "baseline long line abcdefghijklmnop").unwrap();

    let init = zg()
        .args(["index", "init", "--level", "fts+vector"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(init.status.success());

    fs::write(&file, "updated sqlite recall long line abcdefghijklmnop").unwrap();

    let log_path = root.join("embed.log");

    let mut first = zg();
    first.env("ZG_TEST_EMBED_LOG_PATH", &log_path);
    first.env("ZG_TEST_PASSAGE_EMBED_DELAY_MS", "250");
    let first = first
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .arg("sqlite recall")
        .arg(&root)
        .spawn()
        .unwrap();

    wait_for_embed_record(&log_path, "passage", 1);

    let second = zg()
        .env("ZG_TEST_EMBED_LOG_PATH", &log_path)
        .arg("sqlite recall")
        .arg(&root)
        .output()
        .unwrap();

    let first = first.wait_with_output().unwrap();

    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );

    assert_eq!(count_embed_records(&log_path, "passage"), 1);
    assert_eq!(count_embed_records(&log_path, "query"), 2);

    let first_stdout = String::from_utf8(first.stdout).unwrap();
    let second_stdout = String::from_utf8(second.stdout).unwrap();
    assert!(first_stdout.contains("alpha.md:[rfv] 1: updated sqlite recall"));
    assert!(second_stdout.contains("alpha.md:[rfv] 1: updated sqlite recall"));
}

#[test]
fn dirty_search_reconciles_modified_scope_from_requested_path() {
    let root = temp_dir("dirty-scope-reconcile");
    let nested = root.join("notes");
    fs::create_dir_all(&nested).unwrap();
    let file = nested.join("alpha.md");
    fs::write(&file, "first line").unwrap();

    let init = zg()
        .args(["index", "init", "--level", "fts+vector"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(init.status.success());

    fs::write(&file, "updated sqlite recall").unwrap();

    let output = zg().arg("sqlite recall").arg(&nested).output().unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines, vec!["notes/alpha.md:[rfv] 1: updated sqlite recall"]);
}

#[test]
fn dirty_reconcile_batches_embedding_work_across_documents() {
    let root = temp_dir("dirty-reconcile-batch");
    fs::write(
        root.join("alpha.md"),
        "alpha baseline long line abcdefghijklmnop",
    )
    .unwrap();
    fs::write(
        root.join("beta.md"),
        "beta baseline long line abcdefghijklmnop",
    )
    .unwrap();

    let init = zg()
        .args(["index", "init", "--level", "fts+vector"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(init.status.success());

    fs::write(
        root.join("alpha.md"),
        "alpha changed long line one abcdefghijklmnop",
    )
    .unwrap();
    fs::write(
        root.join("beta.md"),
        "beta changed long line two abcdefghijklmnop",
    )
    .unwrap();

    let log_path = root.join("embed.log");
    let output = zg()
        .env("ZG_TEST_EMBED_LOG_PATH", &log_path)
        .arg("changed")
        .arg(&root)
        .output()
        .unwrap();

    assert!(output.status.success());
    let log = fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.lines().any(|line| line == "passage\t2"),
        "expected one batched passage embed for two dirty documents, got: {log}"
    );
}
