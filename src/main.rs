use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use zg::index::{self, IndexStatus};
use zg::search;
use zg::{ZgResult, other};

#[derive(Debug, PartialEq)]
enum SearchMode {
    Regex,
    Indexed(PathBuf),
    Fallback,
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
        SearchMode::Indexed(root) => {
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
        SearchMode::Fallback => {
            let hits = search::fallback_query_search(query, &requested)?;
            for hit in hits {
                println!("{}:{}:{}", hit.path.display(), hit.line_number, hit.line);
            }
            println!(
                "note: {} has no ancestor .zg index; run `zg index init {}` if you want faster local hybrid recall here",
                requested.display(),
                requested.display()
            );
            Ok(())
        }
    }
}

fn resolve_search_mode(query: &str, requested: &Path) -> SearchMode {
    if search::is_probably_regex(query) {
        return SearchMode::Regex;
    }

    if let Some(root) = index_root_for_search(requested) {
        return SearchMode::Indexed(root);
    }

    SearchMode::Fallback
}

fn index_root_for_search(path: &Path) -> Option<PathBuf> {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir().ok()?.join(path)
    };
    zg::paths::find_index_root(&resolved)
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
    println!("requested path: {}", status.requested_path.display());
    println!(
        "index root: {}",
        status
            .index_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string())
    );
    println!("indexed: {}", yes_no(status.indexed));
    println!("chunking: {}", status.chunk_mode);
    println!("marker: {}", status.chunk_marker);
    println!("scope policy: {}", status.scope_policy);
    println!("dirty: {}", yes_no(status.dirty));
    println!(
        "dirty reason: {}",
        status
            .dirty_reason
            .clone()
            .unwrap_or_else(|| "none".to_string())
    );
    println!("files: {}", status.file_count);
    println!("chunks: {}", status.chunk_count);
    println!("fts ready: {}", yes_no(status.fts_ready));
    println!("vector ready: {}", yes_no(status.vector_ready));
    println!(
        "last sync unix ms: {}",
        status
            .last_sync_unix_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "never".to_string())
    );
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

    use super::{SearchMode, resolve_search_mode};
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

    #[test]
    fn regex_queries_keep_regex_semantics_on_indexed_roots() {
        let root = temp_dir("regex-mode");
        fs::write(root.join("alpha.txt"), "TODO: keep regex semantics").unwrap();
        index::init_index(&root).unwrap();

        let mode = resolve_search_mode(r"TODO|FIXME", &root);
        assert_eq!(mode, SearchMode::Regex);
    }

    #[test]
    fn plain_queries_use_fts_on_indexed_roots() {
        let root = temp_dir("fts-mode");
        fs::write(root.join("alpha.txt"), "sqlite vector adapter").unwrap();
        index::init_index(&root).unwrap();

        let mode = resolve_search_mode("sqlite vector", &root);
        assert_eq!(mode, SearchMode::Indexed(root));
    }
}
