use std::env;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use zg::dev;
use zg::index::{self, IndexLevel, IndexStatus};
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
    about = "Local-first search CLI for note-heavy directories",
    long_about = "Local-first search CLI for note-heavy directories.\n\nRegex-shaped input uses grep semantics immediately. Plain-text search uses an explicit local `.zg/` index.",
    after_help = "Examples:\n  zg 'TODO|FIXME' .\n  zg \"sqlite adapter\" notes/\n  zg index init notes/\n  zg index status notes/",
    disable_help_subcommand = true,
    disable_version_flag = true,
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(
        value_name = "QUERY",
        required = true,
        allow_hyphen_values = true,
        help = "Search text or regex pattern"
    )]
    query: Option<String>,

    #[arg(
        value_name = "PATH",
        allow_hyphen_values = true,
        help = "File or directory to search; defaults to the current directory"
    )]
    path: Option<PathBuf>,
}

#[derive(Debug, Subcommand, PartialEq)]
enum Commands {
    #[command(about = "Run regex search immediately with ripgrep semantics")]
    Grep {
        #[arg(
            value_name = "PATTERN",
            allow_hyphen_values = true,
            help = "Regex pattern passed through to ripgrep"
        )]
        pattern: String,
        #[arg(
            value_name = "PATH",
            allow_hyphen_values = true,
            help = "File or directory to search; defaults to the current directory"
        )]
        path: Option<PathBuf>,
    },
    #[command(about = "Run indexed plain-text search inside the nearest ancestor `.zg/` root")]
    Search {
        #[arg(
            value_name = "QUERY",
            allow_hyphen_values = true,
            help = "Plain-text query to resolve against the local `.zg/` index"
        )]
        query: String,
        #[arg(
            value_name = "PATH",
            allow_hyphen_values = true,
            help = "File or directory scope to search; defaults to the current directory"
        )]
        path: Option<PathBuf>,
    },
    #[command(about = "Manage the local `.zg/` search index")]
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
    #[command(about = "Create a local `.zg/` index for a directory")]
    Init {
        #[arg(
            long,
            value_enum,
            default_value_t = CliIndexLevel::Fts,
            help = "Index level to build: `fts` for lexical only, `fts+vector` for hybrid recall"
        )]
        level: CliIndexLevel,
        #[arg(
            value_name = "PATH",
            allow_hyphen_values = true,
            help = "Directory that should own the `.zg/` root; defaults to the current directory"
        )]
        path: Option<PathBuf>,
    },
    #[command(about = "Show index status for a path or its nearest ancestor `.zg/` root")]
    Status {
        #[arg(
            value_name = "PATH",
            allow_hyphen_values = true,
            help = "File or directory whose nearest `.zg/` root should be inspected"
        )]
        path: Option<PathBuf>,
    },
    #[command(about = "Rebuild an existing `.zg/` index")]
    Rebuild {
        #[arg(
            long,
            value_enum,
            help = "Optionally switch index level while rebuilding"
        )]
        level: Option<CliIndexLevel>,
        #[arg(
            value_name = "PATH",
            allow_hyphen_values = true,
            help = "Directory that owns the `.zg/` root; defaults to the current directory"
        )]
        path: Option<PathBuf>,
    },
    #[command(about = "Delete a local `.zg/` index directory")]
    Delete {
        #[arg(
            value_name = "PATH",
            allow_hyphen_values = true,
            help = "Directory that owns the `.zg/` root; defaults to the current directory"
        )]
        path: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CliIndexLevel {
    #[value(name = "fts")]
    Fts,
    #[value(name = "fts+vector")]
    FtsVector,
}

impl From<CliIndexLevel> for IndexLevel {
    fn from(value: CliIndexLevel) -> Self {
        match value {
            CliIndexLevel::Fts => IndexLevel::Fts,
            CliIndexLevel::FtsVector => IndexLevel::FtsVector,
        }
    }
}

#[derive(Debug, Subcommand, PartialEq)]
enum DevCommands {
    SampleVault {
        #[command(subcommand)]
        command: SampleVaultCommands,
    },
    Bench {
        #[command(subcommand)]
        command: BenchCommands,
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
enum BenchCommands {
    SampleVault {
        #[arg(
            long,
            value_name = "FIXTURE",
            default_value = dev::DEFAULT_SEARCH_QUALITY_FIXTURE
        )]
        fixture: PathBuf,
        #[arg(long, value_name = "PATH")]
        vault: Option<PathBuf>,
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        #[arg(long, default_value_t = 1)]
        repeat: usize,
        #[arg(long)]
        fake_embeddings: bool,
        #[arg(long)]
        keep_scratch: bool,
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
        IndexCommands::Init { level, path } => {
            let root = resolve_dir_arg(path.as_deref())?;
            let index_level = IndexLevel::from(level);
            maybe_print_vector_index_start_notice(&root, index_level, "init");
            let stats = index::init_index_with_level(&root, index_level)?;
            println!(
                "{}",
                messages::initialized_index(&root, index_level, &stats)
            );
            if let Some(note) =
                messages::index_level_follow_up(&root, index_level, stats.chunks_indexed)
            {
                println!("{note}");
            }
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
        IndexCommands::Rebuild { level, path } => {
            let root = resolve_dir_arg(path.as_deref())?;
            let index_level = level.map(IndexLevel::from);
            if let Some(effective_level) = effective_rebuild_level(&root, index_level) {
                maybe_print_vector_index_start_notice(&root, effective_level, "rebuild");
            }
            let stats = index::rebuild_index_with_level(&root, index_level)?;
            let status = index::load_status(&root)?;
            println!(
                "{}",
                messages::rebuilt_index(&root, status.index_level, &stats)
            );
            if let Some(note) =
                messages::index_level_follow_up(&root, status.index_level, stats.chunks_indexed)
            {
                println!("{note}");
            }
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

fn effective_rebuild_level(
    root: &Path,
    index_level_override: Option<IndexLevel>,
) -> Option<IndexLevel> {
    index_level_override.or_else(|| {
        let status = index::load_status(root).ok()?;
        (status.index_root.as_deref() == Some(root)).then_some(status.index_level)
    })
}

fn maybe_print_vector_index_start_notice(root: &Path, index_level: IndexLevel, operation: &str) {
    if index_level == IndexLevel::FtsVector {
        eprintln!("{}", messages::vector_index_start_notice(root, operation));
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
        DevCommands::Bench { command } => match command {
            BenchCommands::SampleVault {
                fixture,
                vault,
                out,
                repeat,
                fake_embeddings,
                keep_scratch,
                json,
            } => {
                let exe_path = std::env::current_exe()?;
                let report = dev::run_sample_vault_benchmark(
                    &exe_path,
                    &fixture,
                    vault.as_deref(),
                    fake_embeddings,
                    repeat,
                    keep_scratch,
                    out.as_deref(),
                )?;
                if json {
                    print_json(&report)?;
                } else {
                    print!("{}", format_sample_vault_benchmark(&report));
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
    let hits = search::regex_search(pattern, &root)?
        .into_iter()
        .map(|hit| RenderedSearchHit {
            path: hit.path.display().to_string(),
            line_label: hit.line_number.to_string(),
            preview: hit.line,
            classification: None,
        })
        .collect::<Vec<_>>();
    print!("{}", render_search_hits(&hits, SearchOutputStyle::detect()));
    Ok(())
}

fn run_search(query: &str, path: Option<&Path>) -> ZgResult<()> {
    let requested = resolve_path_arg(path)?;
    match resolve_search_mode(query) {
        SearchMode::Regex => run_grep(query, Some(&requested)),
        SearchMode::Indexed => {
            let root = index::require_index_root_for_search(&requested)?;
            index::reconcile_covering_roots(&requested)?;
            let hits = index::search_indexed(&root, &requested, query, 20)?;
            let hits = hits
                .into_iter()
                .map(|hit| {
                    format_search_result(
                        &hit.rel_path,
                        hit.line_start,
                        hit.line_end,
                        hit.indexed_text_match,
                        hit.partial_text_match,
                        hit.vector_score > f64::EPSILON,
                        &hit.snippet,
                    )
                })
                .collect::<Vec<_>>();
            print!("{}", render_search_hits(&hits, SearchOutputStyle::detect()));
            Ok(())
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct RenderedSearchHit {
    path: String,
    line_label: String,
    preview: String,
    classification: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SearchOutputStyle {
    headings: bool,
    color: bool,
}

impl SearchOutputStyle {
    fn detect() -> Self {
        let term_dumb = env::var_os("TERM").is_some_and(|value| value == "dumb");
        let terminal = std::io::stdout().is_terminal() && !term_dumb;
        let force_color = env::var_os("CLICOLOR_FORCE").is_some_and(|value| value != "0");
        let color = if env::var_os("NO_COLOR").is_some() {
            false
        } else if force_color {
            true
        } else {
            terminal
        };

        Self {
            headings: terminal,
            color,
        }
    }
}

fn format_search_result(
    rel_path: &str,
    line_start: usize,
    line_end: usize,
    indexed_text_match: bool,
    partial_text_match: bool,
    semantic_match: bool,
    snippet: &str,
) -> RenderedSearchHit {
    RenderedSearchHit {
        path: rel_path.to_string(),
        line_label: format_line_label(line_start, line_end),
        preview: inline_preview(snippet),
        classification: Some(display_channel(
            indexed_text_match,
            partial_text_match,
            semantic_match,
        )),
    }
}

fn render_search_hits(hits: &[RenderedSearchHit], style: SearchOutputStyle) -> String {
    let mut lines = Vec::new();
    if style.headings {
        let mut last_path: Option<&str> = None;
        for hit in hits {
            if last_path != Some(hit.path.as_str()) {
                if last_path.is_some() {
                    lines.push(String::new());
                }
                lines.push(paint(&hit.path, style, "\x1b[1;35m"));
                last_path = Some(&hit.path);
            }
            lines.push(render_hit_body(hit, style));
        }
    } else {
        for hit in hits {
            lines.push(format!(
                "{}:{}",
                paint(&hit.path, style, "\x1b[1;35m"),
                render_hit_body(hit, style)
            ));
        }
    }

    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    }
}

fn render_hit_body(hit: &RenderedSearchHit, style: SearchOutputStyle) -> String {
    match &hit.classification {
        Some(classification) => format!(
            "{} {}: {}",
            paint(&format_hit_prefix(classification), style, "\x1b[2;33m"),
            paint(&hit.line_label, style, "\x1b[32m"),
            hit.preview
        ),
        None => format!(
            "{}:{}",
            paint(&hit.line_label, style, "\x1b[32m"),
            hit.preview
        ),
    }
}

fn paint(value: &str, style: SearchOutputStyle, code: &str) -> String {
    if style.color {
        format!("{code}{value}\x1b[0m")
    } else {
        value.to_string()
    }
}

fn format_line_label(line_start: usize, line_end: usize) -> String {
    if line_start == line_end {
        line_start.to_string()
    } else {
        format!("{line_start}-{line_end}")
    }
}

fn inline_preview(snippet: &str) -> String {
    snippet.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn display_channel(
    indexed_text_match: bool,
    partial_text_match: bool,
    semantic_match: bool,
) -> String {
    let mut label = String::new();
    if partial_text_match {
        label.push('r');
    }
    if indexed_text_match {
        label.push('f');
    }
    if semantic_match {
        label.push('v');
    }
    if label.is_empty() {
        label.push('?');
    }
    label
}

fn format_hit_prefix(classification: &str) -> String {
    format!("[{classification}]")
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
    lines.push(format!("index level: {}", status.index_level));
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
    if let Some(last_status) = &status.last_index_run_status {
        lines.push(format!("last index run status: {}", last_status));
    }
    if let Some(duration_ms) = status.last_index_run_duration_ms {
        lines.push(format!("last index run duration ms: {}", duration_ms));
    }
    if let Some(root) = &status.index_root {
        if let Some(hint) =
            messages::status_level_hint(root, status.index_level, status.chunk_count)
        {
            lines.push(hint);
        }
    }
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
    lines.push(format!("fixture: {}", report.fixture_path.display()));
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

fn format_sample_vault_benchmark(report: &dev::SampleVaultBenchmarkReport) -> String {
    let mut lines = vec![
        format!("fixture: {}", report.fixture_path.display()),
        format!("source vault: {}", report.source_vault.display()),
        format!("fake embeddings: {}", yes_no(report.fake_embeddings)),
        format!("repeat: {}", report.repeat),
    ];
    if !report.scratch_root.as_os_str().is_empty() {
        lines.push(format!("scratch root: {}", report.scratch_root.display()));
    }

    for level in &report.levels {
        lines.push(String::new());
        lines.push(format!("[{}]", level.level));
        lines.push(format!("init ms: {}", level.init_elapsed_ms));
        lines.push(format!(
            "status: files={} chunks={}",
            level.status_file_count, level.status_chunk_count
        ));
        lines.push(format!(
            "queries: total={}ms mean={}ms p50={}ms p95={}ms",
            level.query_total_elapsed_ms,
            level.query_mean_elapsed_ms,
            level.query_p50_elapsed_ms,
            level.query_p95_elapsed_ms
        ));
        for case in &level.cases {
            lines.push(format!(
                "  {}#{} {}ms lines={} {}",
                case.id, case.repeat_index, case.elapsed_ms, case.stdout_lines, case.query
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
        lines.push(format!(
            "shared hash: {}",
            chunk.shared_normalized_text_hash
        ));
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
            last_run.status,
            last_run.indexed_files,
            last_run.scanned_files,
            last_run.chunks_indexed
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
        Cli, Commands, IndexCommands, SearchMode, SearchOutputStyle, format_search_result,
        format_status, parse_cli_from, render_search_hits, resolve_search_mode,
    };
    use zg::index::{self, IndexLevel};
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
        // pipeline. Whether a usable explicit index exists is checked later in run_search.
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
        assert!(rendered.contains("index level: fts"));
        assert!(rendered.contains("walk policy: ripgrep-style:"));
        assert!(rendered.contains(".zgignore"));
        assert!(rendered.contains(".zg/ always skipped"));
    }

    #[test]
    fn formatted_status_includes_last_index_run_when_present() {
        let root = temp_dir("status-last-run");
        fs::write(root.join("alpha.md"), "sqlite vector adapter\n").unwrap();
        index::init_index(&root).unwrap();

        let status = index::load_status(&root).unwrap();
        let rendered = format_status(&status);
        assert!(rendered.contains("last index run status:"));
        assert!(rendered.contains("last index run duration ms:"));
    }

    #[test]
    fn explicit_index_required_message_mentions_both_levels() {
        let root = temp_dir("implicit-note");
        let rendered = messages::explicit_index_required_error(&root, 32, true, true);

        assert!(rendered.contains("no ancestor .zg index found"));
        assert!(rendered.contains("estimated chunks: 32"));
        assert!(rendered.contains("quick path: `zg index init --level fts"));
        assert!(rendered.contains("semantic path: `zg index init --level fts+vector"));
        assert!(rendered.contains(&root.display().to_string()));
    }

    #[test]
    fn large_missing_index_message_does_not_upsell_vector() {
        let root = temp_dir("implicit-large");
        let rendered = messages::explicit_index_required_error(&root, 5000, false, false);

        assert!(rendered.contains("run `zg index init"));
        assert!(!rendered.contains("semantic path:"));
    }

    #[test]
    fn status_hint_shows_upgrade_for_small_fts_indexes() {
        let root = temp_dir("status-hint");
        let mut status = index::load_status(&root).unwrap();
        status.index_root = Some(root.clone());
        status.index_level = IndexLevel::Fts;
        status.chunk_count = 128;

        let rendered = format_status(&status);
        assert!(rendered.contains("upgrade hint: `zg index rebuild --level fts+vector"));
    }

    #[test]
    fn vector_index_build_has_no_post_success_follow_up_note() {
        let root = temp_dir("vector-follow-up");
        let rendered = messages::index_level_follow_up(&root, IndexLevel::FtsVector, 128);

        assert!(rendered.is_none());
    }

    #[test]
    fn init_subcommand_accepts_vector_level() {
        let cli = parse_cli_from(["zg", "index", "init", "--level", "fts+vector", "docs"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::Index {
                command: IndexCommands::Init {
                    level: super::CliIndexLevel::FtsVector,
                    path: Some(PathBuf::from("docs")),
                },
            })
        );
    }

    #[test]
    fn rebuilt_message_includes_level() {
        let root = temp_dir("rebuild-note");
        let stats = index::RebuildStats {
            scanned_files: 3,
            indexed_files: 2,
            chunks_indexed: 7,
        };

        let rendered = messages::rebuilt_index(&root, IndexLevel::FtsVector, &stats);
        assert!(rendered.contains("level=fts+vector"));
    }

    #[test]
    fn formatted_search_result_uses_plain_rg_like_layout() {
        let rendered = render_search_hits(
            &[format_search_result(
                "notes/alpha.md",
                7,
                7,
                true,
                false,
                true,
                "sqlite vector adapter",
            )],
            SearchOutputStyle {
                headings: false,
                color: false,
            },
        );

        assert_eq!(rendered, "notes/alpha.md:[fv] 7: sqlite vector adapter\n");
    }

    #[test]
    fn semantic_only_hits_include_channel_label() {
        let rendered = render_search_hits(
            &[format_search_result(
                "README.md",
                94,
                94,
                false,
                false,
                true,
                "Search semantics",
            )],
            SearchOutputStyle {
                headings: false,
                color: false,
            },
        );

        assert_eq!(rendered, "README.md:[v] 94: Search semantics\n");
    }

    #[test]
    fn terminal_render_groups_consecutive_hits_under_file_headings() {
        let rendered = render_search_hits(
            &[
                format_search_result(
                    "AGENTS.md",
                    4,
                    4,
                    true,
                    true,
                    true,
                    "e.g. R0_product_philosophy.md, R1_tech_decision_blabla.md",
                ),
                format_search_result(
                    "AGENTS.md",
                    20,
                    20,
                    false,
                    false,
                    true,
                    "Observation flow, from actual implementation, to principle level of intent",
                ),
                format_search_result("README.md", 94, 94, false, false, true, "Search semantics"),
            ],
            SearchOutputStyle {
                headings: true,
                color: false,
            },
        );

        assert_eq!(
            rendered,
            "AGENTS.md\n[rfv] 4: e.g. R0_product_philosophy.md, R1_tech_decision_blabla.md\n[v] 20: Observation flow, from actual implementation, to principle level of intent\n\nREADME.md\n[v] 94: Search semantics\n"
        );
    }
}
