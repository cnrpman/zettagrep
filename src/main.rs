use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use zg::index::{self, IndexStatus};
use zg::messages;
use zg::search;
use zg::{ZgResult, other};

#[derive(Debug, PartialEq)]
enum SearchMode {
    Regex,
    Indexed,
}

#[derive(Debug, Parser, PartialEq)]
#[command(
    name = "zg",
    disable_help_subcommand = true,
    disable_version_flag = true,
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(value_name = "QUERY", required = true, allow_hyphen_values = true)]
    query: Option<String>,

    #[arg(value_name = "PATH", allow_hyphen_values = true)]
    path: Option<PathBuf>,
}

#[derive(Debug, Subcommand, PartialEq)]
enum Commands {
    Grep {
        #[arg(value_name = "PATTERN", allow_hyphen_values = true)]
        pattern: String,
        #[arg(value_name = "PATH", allow_hyphen_values = true)]
        path: Option<PathBuf>,
    },
    Search {
        #[arg(value_name = "QUERY", allow_hyphen_values = true)]
        query: String,
        #[arg(value_name = "PATH", allow_hyphen_values = true)]
        path: Option<PathBuf>,
    },
    Index {
        #[command(subcommand)]
        command: IndexCommands,
    },
}

#[derive(Debug, Subcommand, PartialEq)]
enum IndexCommands {
    Init {
        #[arg(value_name = "PATH", allow_hyphen_values = true)]
        path: Option<PathBuf>,
    },
    Status {
        #[arg(value_name = "PATH", allow_hyphen_values = true)]
        path: Option<PathBuf>,
    },
    Rebuild {
        #[arg(value_name = "PATH", allow_hyphen_values = true)]
        path: Option<PathBuf>,
    },
    Delete {
        #[arg(value_name = "PATH", allow_hyphen_values = true)]
        path: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let rendered = error.to_string();
            if rendered.starts_with("zg:") {
                eprintln!("{rendered}");
            } else {
                eprintln!("error: {rendered}");
            }
            ExitCode::FAILURE
        }
    }
}

fn run() -> ZgResult<()> {
    let args = env::args_os().collect::<Vec<_>>();
    if args.len() == 1 {
        print_help()?;
        return Ok(());
    }

    match parse_cli_from(args) {
        Ok(cli) => run_cli(cli),
        Err(error) if matches!(error.kind(), ErrorKind::DisplayHelp) => {
            print!("{error}");
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn parse_cli_from<I, T>(args: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    Cli::try_parse_from(args)
}

fn run_cli(cli: Cli) -> ZgResult<()> {
    match cli.command {
        Some(Commands::Grep { pattern, path }) => run_grep(&pattern, path.as_deref()),
        Some(Commands::Search { query, path }) => run_search(&query, path.as_deref()),
        Some(Commands::Index { command }) => run_index_command(command),
        None => {
            let query = cli.query.ok_or_else(|| other("missing query"))?;
            match resolve_search_mode(&query) {
                SearchMode::Regex => run_grep(&query, cli.path.as_deref()),
                SearchMode::Indexed => run_search(&query, cli.path.as_deref()),
            }
        }
    }
}

fn run_index_command(command: IndexCommands) -> ZgResult<()> {
    match command {
        IndexCommands::Init { path } => {
            let root = resolve_dir_arg(path.as_deref())?;
            let stats = index::init_index(&root)?;
            println!("{}", messages::initialized_index(&root, &stats));
            if let Some(note) = index::best_effort_overlap_note(&root)? {
                println!("{note}");
            }
            Ok(())
        }
        IndexCommands::Status { path } => {
            let target = resolve_path_arg(path.as_deref())?;
            let status = index::load_status(&target)?;
            print_status(&status);
            Ok(())
        }
        IndexCommands::Rebuild { path } => {
            let root = resolve_dir_arg(path.as_deref())?;
            let stats = index::rebuild_index(&root)?;
            println!("{}", messages::rebuilt_index(&root, &stats));
            Ok(())
        }
        IndexCommands::Delete { path } => {
            let root = resolve_dir_arg(path.as_deref())?;
            if index::delete_index(&root)? {
                println!("{}", messages::deleted_local_cache(&root));
            } else {
                println!("{}", messages::no_local_cache(&root));
            }
            Ok(())
        }
    }
}

fn run_grep(pattern: &str, path: Option<&Path>) -> ZgResult<()> {
    let root = resolve_path_arg(path)?;
    for hit in search::regex_search(pattern, &root)? {
        println!("{}:{}:{}", hit.path.display(), hit.line_number, hit.line);
    }
    Ok(())
}

fn run_search(query: &str, path: Option<&Path>) -> ZgResult<()> {
    let requested = resolve_path_arg(path)?;
    match resolve_search_mode(query) {
        SearchMode::Regex => run_grep(query, Some(&requested)),
        SearchMode::Indexed => {
            let (root, init_stats) = index::ensure_index_root_for_search(&requested)?;
            if let Some(stats) = init_stats {
                eprintln!("{}", messages::implicit_init_note(&root, &stats));
                eprintln!("{}", messages::cache_delete_note(&root));
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

fn resolve_search_mode(query: &str) -> SearchMode {
    if search::is_probably_regex(query) {
        return SearchMode::Regex;
    }

    SearchMode::Indexed
}

fn resolve_dir_arg(path: Option<&Path>) -> ZgResult<PathBuf> {
    let candidate = match path {
        Some(value) => value.to_path_buf(),
        None => env::current_dir()?,
    };
    zg::paths::resolve_existing_dir(&candidate)
}

fn resolve_path_arg(path: Option<&Path>) -> ZgResult<PathBuf> {
    let candidate = match path {
        Some(value) => value.to_path_buf(),
        None => env::current_dir()?,
    };
    zg::paths::resolve_existing_path(&candidate)
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

fn print_help() -> ZgResult<()> {
    let mut command = Cli::command();
    command.print_help()?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        Cli, Commands, IndexCommands, SearchMode, format_status, parse_cli_from,
        resolve_search_mode,
    };
    use zg::index;
    use zg::messages;

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

        let mode = resolve_search_mode(r"TODO|FIXME");
        assert_eq!(mode, SearchMode::Regex);
    }

    #[test]
    fn plain_queries_use_indexed_hybrid_mode_on_indexed_roots() {
        let root = temp_dir("fts-mode");
        mark_indexed(&root);

        let mode = resolve_search_mode("sqlite vector");
        assert_eq!(mode, SearchMode::Indexed);
    }

    #[test]
    fn plain_queries_select_indexed_search_pipeline_without_manual_index_setup() {
        let root = temp_dir("lazy-init-mode");
        fs::write(root.join("alpha.txt"), "sqlite vector adapter").unwrap();

        // CLI dispatch is intentionally simple: non-regex input enters the indexed-search
        // pipeline, which then reuses the nearest ancestor .zg or creates one for the
        // directory search scope before reconcile/embed work runs.
        let mode = resolve_search_mode("sqlite vector");
        assert_eq!(mode, SearchMode::Indexed);
    }

    #[test]
    fn default_entrypoint_parses_query_and_path() {
        let cli = parse_cli_from(["zg", "sqlite vector", "docs"]).unwrap();
        assert_eq!(
            cli,
            Cli {
                command: None,
                query: Some("sqlite vector".to_string()),
                path: Some(PathBuf::from("docs")),
            }
        );
    }

    #[test]
    fn index_subcommands_parse_through_clap() {
        let cli = parse_cli_from(["zg", "index", "status", "docs"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::Index {
                command: IndexCommands::Status {
                    path: Some(PathBuf::from("docs")),
                },
            })
        );
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

    #[test]
    fn implicit_init_note_mentions_delete_command() {
        let root = temp_dir("implicit-note");
        let stats = index::RebuildStats {
            scanned_files: 3,
            indexed_files: 2,
            chunks_indexed: 7,
        };

        let init_note = messages::implicit_init_note(&root, &stats);
        let delete_note = messages::cache_delete_note(&root);

        assert!(init_note.contains("initialized local cache"));
        assert!(delete_note.contains("zg index delete"));
        assert!(delete_note.contains(&root.display().to_string()));
    }
}
