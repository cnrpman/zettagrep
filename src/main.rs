use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use serde::Serialize;
use zg::dev;
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
    #[command(hide = true)]
    Dev {
        #[command(subcommand)]
        command: DevCommands,
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

#[derive(Debug, Subcommand, PartialEq)]
enum DevCommands {
    SampleVault {
        #[command(subcommand)]
        command: SampleVaultCommands,
    },
    Eval {
        #[command(subcommand)]
        command: EvalCommands,
    },
    Probe {
        #[command(subcommand)]
        command: ProbeCommands,
    },
}

#[derive(Debug, Subcommand, PartialEq)]
enum SampleVaultCommands {
    Ensure {
        #[arg(
            long,
            value_name = "MANIFEST",
            default_value = dev::DEFAULT_SAMPLE_VAULT_MANIFEST
        )]
        manifest: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand, PartialEq)]
enum EvalCommands {
    SearchQuality {
        #[arg(
            long,
            value_name = "FIXTURE",
            default_value = dev::DEFAULT_SEARCH_QUALITY_FIXTURE
        )]
        fixture: PathBuf,
        #[arg(
            long,
            value_name = "GOLDEN",
            default_value = dev::DEFAULT_SEARCH_QUALITY_GOLDEN
        )]
        golden: PathBuf,
        #[arg(long, value_name = "PATH")]
        vault: Option<PathBuf>,
        #[arg(long)]
        update_golden: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand, PartialEq)]
enum ProbeCommands {
    Chunks {
        #[arg(value_name = "PATH", allow_hyphen_values = true)]
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    DbCache {
        #[arg(value_name = "PATH", allow_hyphen_values = true)]
        path: PathBuf,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        json: bool,
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
        Some(Commands::Dev { command }) => run_dev_command(command),
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

fn run_dev_command(command: DevCommands) -> ZgResult<()> {
    match command {
        DevCommands::SampleVault { command } => match command {
            SampleVaultCommands::Ensure {
                manifest,
                force,
                json,
            } => {
                let ensured = dev::ensure_sample_vault(&manifest, force)?;
                if json {
                    print_json(&ensured)?;
                } else {
                    println!(
                        "{} {} at {} ({})",
                        ensured.id,
                        ensured.status,
                        ensured.path.display(),
                        ensured.commit
                    );
                }
                Ok(())
            }
        },
        DevCommands::Eval { command } => match command {
            EvalCommands::SearchQuality {
                fixture,
                golden,
                vault,
                update_golden,
                json,
            } => {
                let vault_root = dev::resolve_fixture_vault(&fixture, vault.as_deref())?;
                if update_golden {
                    let suite = dev::write_search_quality_golden(&fixture, &golden, &vault_root)?;
                    if json {
                        print_json(&suite)?;
                    } else {
                        println!(
                            "updated golden {} [{} cases] against {}",
                            golden.display(),
                            suite.cases.len(),
                            vault_root.display()
                        );
                    }
                    return Ok(());
                }

                let report = dev::run_search_quality_suite(&fixture, Some(&golden), &vault_root)?;
                if json {
                    print_json(&report)?;
                } else {
                    print!("{}", format_search_quality_report(&report));
                }
                if report.passed() {
                    Ok(())
                } else {
                    Err(other("search quality evaluation failed"))
                }
            }
        },
        DevCommands::Probe { command } => match command {
            ProbeCommands::Chunks { path, json } => {
                let report = dev::probe_chunks(&path)?;
                if json {
                    print_json(&report)?;
                } else {
                    print!("{}", format_chunk_probe(&report));
                }
                Ok(())
            }
            ProbeCommands::DbCache { path, limit, json } => {
                let report = dev::probe_db_cache(&path, limit)?;
                if json {
                    print_json(&report)?;
                } else {
                    print!("{}", format_db_cache_probe(&report));
                }
                Ok(())
            }
        },
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
                    "{}",
                    format_search_result(
                        &hit.rel_path,
                        hit.score,
                        hit.lexical_score,
                        hit.vector_score,
                        &hit.snippet,
                    )
                );
            }
            Ok(())
        }
    }
}

fn format_search_result(
    rel_path: &str,
    score: f64,
    lexical_score: f64,
    vector_score: f64,
    snippet: &str,
) -> String {
    format!(
        "{rel_path}  score={score:.3}  lexical={lexical_score:.3}  vector={vector_score:.3}  {snippet}"
    )
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

fn print_json<T: Serialize>(value: &T) -> ZgResult<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
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

fn format_search_quality_report(report: &dev::SearchQualityReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "suite: {}  cases={}/{}  vault={}",
        report.suite_id,
        report.passed_cases,
        report.total_cases,
        report.vault_root.display()
    ));
    lines.push(format!(
        "fixture: {}",
        report.fixture_path.display()
    ));
    if let Some(golden_path) = &report.golden_path {
        lines.push(format!("golden: {}", golden_path.display()));
    }
    lines.push(format!(
        "expectation failures: {}  golden failures: {}",
        report.expectation_failures, report.golden_failures
    ));

    for case in &report.cases {
        lines.push(String::new());
        lines.push(format!(
            "[{}] {}  {}",
            if case.passed { "pass" } else { "fail" },
            case.id,
            case.query
        ));
        if let Some(scope) = &case.scope {
            lines.push(format!("scope: {}", scope));
        }
        if let Some(notes) = &case.notes {
            lines.push(format!("notes: {}", notes));
        }
        for failure in &case.expectation_failures {
            lines.push(format!("expectation failure: {failure}"));
        }
        for failure in &case.golden_failures {
            lines.push(format!("golden failure: {failure}"));
        }
        for hit in &case.hits {
            lines.push(format!(
                "#{:02} {}:{}-{} score={:.3} lexical={:.3} vector={:.3} {}",
                hit.rank,
                hit.rel_path,
                hit.line_start,
                hit.line_end,
                hit.score,
                hit.lexical_score,
                hit.vector_score,
                hit.snippet
            ));
        }
    }

    lines.join("\n") + "\n"
}

fn format_chunk_probe(report: &dev::ChunkProbeReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("path: {}", report.path.display()));
    lines.push(format!("chunks: {}", report.chunk_count));
    for chunk in &report.chunks {
        lines.push(String::new());
        lines.push(format!(
            "#{} {} {}-{}",
            chunk.chunk_index, chunk.chunk_kind, chunk.line_start, chunk.line_end
        ));
        if let Some(language) = &chunk.language {
            lines.push(format!("language: {}", language));
        }
        if let Some(symbol_kind) = &chunk.symbol_kind {
            lines.push(format!("symbol kind: {}", symbol_kind));
        }
        if let Some(container) = &chunk.container {
            lines.push(format!("container: {}", container));
        }
        lines.push(format!("raw: {:?}", chunk.raw_text));
        lines.push(format!("normalized: {:?}", chunk.normalized_text));
        lines.push(format!(
            "shared normalized: {:?}",
            chunk.shared_normalized_text
        ));
        lines.push(format!("shared hash: {}", chunk.shared_normalized_text_hash));
    }
    lines.join("\n") + "\n"
}

fn format_db_cache_probe(report: &dev::DbCacheProbeReport) -> String {
    let mut lines = vec![
        format!("requested path: {}", report.requested_path.display()),
        format!("index root: {}", report.index_root.display()),
        format!("indexed: {}", yes_no(report.status.indexed)),
        format!("dirty: {}", yes_no(report.status.dirty)),
        format!(
            "totals: files={} chunk_refs={} shared_chunks={} vec_chunks={} vec_index_rows={} fts_rows={}",
            report.totals.files,
            report.totals.chunk_refs,
            report.totals.shared_chunks,
            report.totals.vec_chunks,
            report.totals.vec_index_rows,
            report.totals.fts_rows
        ),
    ];

    lines.push("chunk kinds:".to_string());
    for item in &report.chunk_kinds {
        lines.push(format!("  {} {}", item.key, item.count));
    }

    lines.push("symbol languages:".to_string());
    for item in &report.symbol_languages {
        lines.push(format!("  {} {}", item.key, item.count));
    }

    lines.push("symbol kinds:".to_string());
    for item in &report.symbol_kinds {
        lines.push(format!("  {} {}", item.key, item.count));
    }

    lines.push("top files by chunks:".to_string());
    for item in &report.top_files_by_chunks {
        lines.push(format!("  {} {}", item.chunk_count, item.rel_path));
    }

    lines.push("top shared chunks:".to_string());
    for item in &report.top_shared_chunks {
        lines.push(format!(
            "  ref_count={} {:?}",
            item.ref_count, item.normalized_text_preview
        ));
    }

    if let Some(last_run) = &report.last_index_run {
        lines.push(format!(
            "last index run: {} [{} indexed / {} scanned / {} chunks]",
            last_run.status, last_run.indexed_files, last_run.scanned_files, last_run.chunks_indexed
        ));
        if let Some(error) = &last_run.error {
            lines.push(format!("last index run error: {}", error));
        }
    }

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
        Cli, Commands, IndexCommands, SearchMode, format_search_result, format_status,
        parse_cli_from, resolve_search_mode,
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

    #[test]
    fn formatted_search_result_keeps_user_facing_layout() {
        let rendered = format_search_result(
            "notes/alpha.md",
            0.1239,
            1.0,
            0.4561,
            "sqlite vector adapter",
        );

        assert_eq!(
            rendered,
            "notes/alpha.md  score=0.124  lexical=1.000  vector=0.456  sqlite vector adapter"
        );
    }
}
