use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use zg::index::{self, IndexStatus};
use zg::search;
use zg::{ZgResult, other};

#[derive(Debug, PartialEq)]
enum SearchMode {
    Regex,
    Indexed,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> ZgResult<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        print_help();
        return Ok(());
    }

    match args[0].as_str() {
        "-h" | "--help" => {
            print_help();
            Ok(())
        }
        "grep" => {
            let (pattern, path) = parse_query_and_path(&args[1..])?;
            run_grep(&pattern, path.as_deref())
        }
        "search" => {
            let (query, path) = parse_query_and_path(&args[1..])?;
            run_search(&query, path.as_deref())
        }
        "index" => run_index_command(&args[1..]),
        query => {
            let path = args.get(1).map(PathBuf::from);
            if search::is_probably_regex(query) {
                run_grep(query, path.as_deref())
            } else {
                run_search(query, path.as_deref())
            }
        }
    }
}

fn run_index_command(args: &[String]) -> ZgResult<()> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(other(
            "missing index subcommand: expected init|status|rebuild",
        ));
    };

    match command {
        "init" => {
            let root = resolve_dir_arg(args.get(1).map(String::as_str))?;
            let stats = index::init_index(&root)?;
            println!(
                "initialized {} (.zg/, SQLite, lazy-first index) [{} indexed / {} scanned / {} chunks]",
                root.display(),
                stats.indexed_files,
                stats.scanned_files,
                stats.chunks_indexed,
            );
            if let Some(note) = index::best_effort_overlap_note(&root)? {
                println!("{note}");
            }
            Ok(())
        }
        "status" => {
            let target = resolve_path_arg(args.get(1).map(String::as_str))?;
            let status = index::load_status(&target)?;
            print_status(&status);
            Ok(())
        }
        "rebuild" => {
            let root = resolve_dir_arg(args.get(1).map(String::as_str))?;
            let stats = index::rebuild_index(&root)?;
            println!(
                "rebuilt {} [{} indexed / {} scanned]",
                root.display(),
                stats.indexed_files,
                stats.scanned_files
            );
            Ok(())
        }
        command => Err(other(format!("unknown index subcommand: {command}"))),
    }
}

fn run_grep(pattern: &str, path: Option<&Path>) -> ZgResult<()> {
    let root = resolve_path_arg(path.and_then(|value| value.to_str()))?;
    for hit in search::regex_search(pattern, &root)? {
        println!("{}:{}:{}", hit.path.display(), hit.line_number, hit.line);
    }
    Ok(())
}

fn run_search(query: &str, path: Option<&Path>) -> ZgResult<()> {
    let requested = resolve_path_arg(path.and_then(|value| value.to_str()))?;
    match resolve_search_mode(query, &requested) {
        SearchMode::Regex => run_grep(query, Some(&requested)),
        SearchMode::Indexed => {
            let (root, init_stats) = index::ensure_index_root_for_search(&requested)?;
            if let Some(stats) = init_stats {
                eprintln!(
                    "note: no ancestor .zg index found; initializing local index at {} for this search ({} files / {} chunks)",
                    root.display(),
                    stats.indexed_files,
                    stats.chunks_indexed,
                );
                if let Some(note) = index::best_effort_overlap_note(&root)? {
                    eprintln!("{note}");
                }
            }
            index::reconcile_covering_roots(&requested)?;
            let hits = index::search_hybrid(&root, &requested, query, 20)?;
            for hit in hits {
                println!(
                    "{}  score={:.3}  lexical={:.3}  vector={:.3}  {}",
                    hit.rel_path, hit.score, hit.lexical_score, hit.vector_score, hit.snippet
                );
            }
            Ok(())
        }
    }
}

fn resolve_search_mode(query: &str, requested: &Path) -> SearchMode {
    if search::is_probably_regex(query) {
        return SearchMode::Regex;
    }

    let _ = requested;
    SearchMode::Indexed
}

fn resolve_dir_arg(path: Option<&str>) -> ZgResult<PathBuf> {
    let candidate = match path {
        Some(value) => PathBuf::from(value),
        None => env::current_dir()?,
    };
    zg::paths::resolve_existing_dir(&candidate)
}

fn resolve_path_arg(path: Option<&str>) -> ZgResult<PathBuf> {
    let candidate = match path {
        Some(value) => PathBuf::from(value),
        None => env::current_dir()?,
    };
    zg::paths::resolve_existing_path(&candidate)
}

fn parse_query_and_path(args: &[String]) -> ZgResult<(String, Option<PathBuf>)> {
    let Some(query) = args.first() else {
        return Err(other("missing query"));
    };

    Ok((query.clone(), args.get(1).map(PathBuf::from)))
}

fn print_status(status: &IndexStatus) {
    print!("{}", format_status(status));
}

fn format_status(status: &IndexStatus) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "requested path: {}",
        status.requested_path.display()
    ));
    lines.push(format!(
        "index root: {}",
        status
            .index_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string())
    ));
    lines.push(format!("indexed: {}", yes_no(status.indexed)));
    lines.push(format!("chunking: {}", status.chunk_mode));
    lines.push(format!("marker: {}", status.chunk_marker));
    lines.push(format!("scope policy: {}", status.scope_policy));
    lines.push(format!("walk policy: {}", status.walk_policy));
    lines.push(format!("dirty: {}", yes_no(status.dirty)));
    lines.push(format!(
        "dirty reason: {}",
        status
            .dirty_reason
            .clone()
            .unwrap_or_else(|| "none".to_string())
    ));
    lines.push(format!("files: {}", status.file_count));
    lines.push(format!("chunks: {}", status.chunk_count));
    lines.push(format!("fts ready: {}", yes_no(status.fts_ready)));
    lines.push(format!("vector ready: {}", yes_no(status.vector_ready)));
    lines.push(format!(
        "last sync unix ms: {}",
        status
            .last_sync_unix_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "never".to_string())
    ));
    lines.join("\n") + "\n"
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn print_help() {
    println!("zg <pattern-or-query> [path]");
    println!("zg grep <pattern> [path]");
    println!("zg search <query> [path]");
    println!("zg index init [path]");
    println!("zg index status [path]");
    println!("zg index rebuild [path]");
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{SearchMode, format_status, resolve_search_mode};
    use zg::index;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zg-main-{name}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mark_indexed(root: &std::path::Path) {
        fs::create_dir_all(root.join(".zg")).unwrap();
        fs::write(root.join(".zg/index.db"), "").unwrap();
    }

    #[test]
    fn regex_queries_keep_regex_semantics_on_indexed_roots() {
        let root = temp_dir("regex-mode");
        mark_indexed(&root);

        let mode = resolve_search_mode(r"TODO|FIXME", &root);
        assert_eq!(mode, SearchMode::Regex);
    }

    #[test]
    fn plain_queries_use_indexed_hybrid_mode_on_indexed_roots() {
        let root = temp_dir("fts-mode");
        mark_indexed(&root);

        let mode = resolve_search_mode("sqlite vector", &root);
        assert_eq!(mode, SearchMode::Indexed);
    }

    #[test]
    fn plain_queries_select_indexed_search_pipeline_without_manual_index_setup() {
        let root = temp_dir("lazy-init-mode");
        fs::write(root.join("alpha.txt"), "sqlite vector adapter").unwrap();

        // CLI dispatch is intentionally simple: non-regex input enters the indexed-search
        // pipeline, which then reuses the nearest ancestor .zg or creates one for the
        // directory search scope before reconcile/embed work runs.
        let mode = resolve_search_mode("sqlite vector", &root);
        assert_eq!(mode, SearchMode::Indexed);
    }

    #[test]
    fn formatted_status_exposes_walk_policy() {
        let root = temp_dir("status");
        let status = index::load_status(&root).unwrap();

        let rendered = format_status(&status);
        assert!(rendered.contains("walk policy: ripgrep-style:"));
        assert!(rendered.contains(".zgignore"));
        assert!(rendered.contains(".zg/ always skipped"));
    }
}
