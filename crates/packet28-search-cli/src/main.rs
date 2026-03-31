use std::ffi::OsString;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::thread;
use std::time::Instant;
#[cfg(unix)]
use std::time::{Duration, Instant as StdInstant};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use packet28_daemon_core::{
    log_path, read_runtime_info, read_socket_message, ready_path, resolve_workspace_root,
    socket_path, write_socket_message, DaemonRequest, DaemonResponse, Packet28SearchGuardResponse,
    Packet28SearchRequest as DaemonPacket28SearchRequest,
};
use packet28_reducer_core::{parse_region_for_path, SearchRequest, SearchResult};
use packet28_search_core::{
    guarded_fallback_reason, indexed_search, load_runtime, rebuild_full_index,
};
#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[cfg(unix)]
const DAEMON_SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Parser, Debug)]
#[command(name = "p28")]
#[command(about = "Instant indexed regex search for local codebases")]
struct SearchCli {
    pattern: String,
    #[arg(value_name = "path")]
    paths: Vec<String>,
    #[arg(long = "fixed-strings", visible_alias = "fixed-string")]
    fixed_strings: bool,
    #[arg(long)]
    ignore_case: bool,
    #[arg(long = "word-regexp", visible_alias = "whole-word")]
    word_regexp: bool,
    #[arg(long, value_enum, default_value_t = EngineMode::Auto)]
    engine: EngineMode,
    #[arg(long, value_enum, default_value_t = TransportMode::Auto)]
    transport: TransportMode,
    #[arg(long)]
    compact: bool,
    #[arg(long)]
    stats: bool,
    #[arg(long)]
    json: bool,
    #[arg(long, default_value_t = 20)]
    max_matches_per_file: usize,
    #[arg(long, default_value_t = 200)]
    max_total_matches: usize,
}

#[derive(Parser, Debug)]
#[command(name = "p28 debug")]
#[command(about = "Maintenance commands for the p28 search engine")]
struct DebugCli {
    #[command(subcommand)]
    command: DebugCommand,
}

#[derive(Subcommand, Debug)]
enum DebugCommand {
    Build(BuildArgs),
    Guard(DebugSearchArgs),
    Bench(DebugSearchArgs),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum EngineMode {
    Auto,
    Indexed,
    Legacy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum TransportMode {
    Auto,
    Inproc,
    Daemon,
}

#[derive(Args, Debug)]
struct BuildArgs {
    root: PathBuf,
    #[arg(long, default_value_t = true)]
    include_tests: bool,
}

#[derive(Args, Clone, Debug)]
struct CommonSearchArgs {
    #[arg(long = "path")]
    paths: Vec<String>,
    #[arg(long = "fixed-strings", visible_alias = "fixed-string")]
    fixed_strings: bool,
    #[arg(long)]
    ignore_case: bool,
    #[arg(long = "word-regexp", visible_alias = "whole-word")]
    word_regexp: bool,
    #[arg(long, value_enum, default_value_t = EngineMode::Auto)]
    engine: EngineMode,
    #[arg(long, value_enum, default_value_t = TransportMode::Auto)]
    transport: TransportMode,
    #[arg(long)]
    compact: bool,
    #[arg(long, default_value_t = 20)]
    max_matches_per_file: usize,
    #[arg(long, default_value_t = 200)]
    max_total_matches: usize,
}

#[derive(Args, Clone, Debug)]
struct DebugSearchArgs {
    root: PathBuf,
    pattern: String,
    #[command(flatten)]
    options: CommonSearchArgs,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_else(|| OsString::from("p28"));
    let collected = args.collect::<Vec<_>>();
    if matches!(
        collected.first().and_then(|arg| arg.to_str()),
        Some("debug")
    ) {
        let cli =
            DebugCli::parse_from(std::iter::once(program).chain(collected.into_iter().skip(1)));
        run_debug(cli.command)
    } else {
        let cli = SearchCli::parse_from(std::iter::once(program).chain(collected));
        run_search(cli)
    }
}

fn run_search(args: SearchCli) -> Result<()> {
    let root = std::env::current_dir().context("failed to resolve current directory")?;
    let request = search_request(
        &args.pattern,
        &args.paths,
        SearchOptions {
            fixed_strings: args.fixed_strings,
            ignore_case: args.ignore_case,
            word_regexp: args.word_regexp,
            max_matches_per_file: args.max_matches_per_file,
            max_total_matches: args.max_total_matches,
        },
    );
    let started = Instant::now();
    let (result, transport) = execute_search(&root, &request, args.engine, args.transport)?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    if args.json {
        emit_json(elapsed_ms, transport, result)?;
        return Ok(());
    }
    emit_human_hits(&root, &result, args.compact)?;
    if args.stats {
        emit_stats("p28", transport, elapsed_ms, &result);
    }
    Ok(())
}

fn run_debug(command: DebugCommand) -> Result<()> {
    match command {
        DebugCommand::Build(args) => run_build(args),
        DebugCommand::Guard(args) => run_guard(args),
        DebugCommand::Bench(args) => run_bench(args),
    }
}

fn run_build(args: BuildArgs) -> Result<()> {
    let started = Instant::now();
    let runtime = rebuild_full_index(&args.root, args.include_tests)?;
    println!(
        "build_ms={:.3} generation={} files={}",
        started.elapsed().as_secs_f64() * 1000.0,
        runtime.manifest.generation,
        runtime.manifest.indexed_files
    );
    Ok(())
}

fn run_guard(args: DebugSearchArgs) -> Result<()> {
    let request = debug_search_request(&args);
    match args.options.engine {
        EngineMode::Legacy => {
            println!("mode=fallback");
            println!("reason=forced legacy backend");
        }
        EngineMode::Indexed => {
            println!("mode=index");
            println!("reason=forced indexed backend");
        }
        EngineMode::Auto => match guard_reason(&args.root, &request, args.options.transport)? {
            Some(reason) => {
                println!("mode=fallback");
                println!("reason={reason}");
            }
            None => {
                println!("mode=index");
                println!("reason=selective");
            }
        },
    }
    Ok(())
}

fn run_bench(args: DebugSearchArgs) -> Result<()> {
    let request = debug_search_request(&args);
    let guard = match args.options.engine {
        EngineMode::Legacy => Some("forced legacy backend".to_string()),
        EngineMode::Indexed => None,
        EngineMode::Auto => guard_reason(&args.root, &request, args.options.transport)?,
    };

    let packet28_started = Instant::now();
    let (packet28, transport) = execute_search(
        &args.root,
        &request,
        args.options.engine,
        args.options.transport,
    )?;
    let packet28_ms = packet28_started.elapsed().as_secs_f64() * 1000.0;

    let reducer_started = Instant::now();
    let reducer = packet28_reducer_core::search(&args.root, &request)?;
    let reducer_ms = reducer_started.elapsed().as_secs_f64() * 1000.0;

    let packet28_hits = collect_hits(&packet28);
    let reducer_hits = collect_hits(&reducer);

    println!(
        "guard={}",
        guard.clone().unwrap_or_else(|| "index".to_string())
    );
    println!(
        "parity={}",
        if packet28_hits == reducer_hits {
            "exact"
        } else {
            "mismatch"
        }
    );
    if packet28_hits != reducer_hits {
        for missing in reducer_hits
            .iter()
            .filter(|hit| !packet28_hits.contains(*hit))
        {
            println!("missing={missing}");
        }
        for extra in packet28_hits
            .iter()
            .filter(|hit| !reducer_hits.contains(*hit))
        {
            println!("extra={extra}");
        }
    }
    print_debug_search_result(
        "p28",
        transport,
        packet28_ms,
        &packet28,
        args.options.compact,
    );
    print_debug_search_result(
        "legacy_rg",
        TransportMode::Inproc,
        reducer_ms,
        &reducer,
        args.options.compact,
    );
    Ok(())
}

#[derive(Clone, Copy)]
struct SearchOptions {
    fixed_strings: bool,
    ignore_case: bool,
    word_regexp: bool,
    max_matches_per_file: usize,
    max_total_matches: usize,
}

fn debug_search_request(args: &DebugSearchArgs) -> SearchRequest {
    search_request(
        &args.pattern,
        &args.options.paths,
        SearchOptions {
            fixed_strings: args.options.fixed_strings,
            ignore_case: args.options.ignore_case,
            word_regexp: args.options.word_regexp,
            max_matches_per_file: args.options.max_matches_per_file,
            max_total_matches: args.options.max_total_matches,
        },
    )
}

fn search_request(pattern: &str, paths: &[String], options: SearchOptions) -> SearchRequest {
    SearchRequest {
        query: pattern.to_string(),
        requested_paths: paths.to_vec(),
        fixed_string: options.fixed_strings,
        case_sensitive: Some(!options.ignore_case),
        whole_word: options.word_regexp,
        context_lines: Some(0),
        max_matches_per_file: Some(options.max_matches_per_file),
        max_total_matches: Some(options.max_total_matches),
    }
}

fn execute_search(
    root: &PathBuf,
    request: &SearchRequest,
    engine: EngineMode,
    transport: TransportMode,
) -> Result<(SearchResult, TransportMode)> {
    match transport {
        TransportMode::Inproc => execute_search_inproc(root, request, engine)
            .map(|result| (result, TransportMode::Inproc)),
        TransportMode::Daemon => {
            #[cfg(unix)]
            ensure_daemon(&resolve_workspace_root(root))?;
            execute_search_daemon(root, request, engine)
                .map(|result| (result, TransportMode::Daemon))
        }
        TransportMode::Auto => execute_search_auto(root, request, engine),
    }
}

#[cfg(unix)]
fn execute_search_auto(
    root: &PathBuf,
    request: &SearchRequest,
    engine: EngineMode,
) -> Result<(SearchResult, TransportMode)> {
    let workspace_root = resolve_workspace_root(root);
    if let Err(err) = ensure_daemon(&workspace_root) {
        let mut result = execute_search_inproc(root, request, engine)?;
        annotate_reason(&mut result, format!("daemon unavailable: {err}"));
        return Ok((result, TransportMode::Inproc));
    }
    match execute_search_daemon(root, request, engine) {
        Ok(result) => Ok((result, TransportMode::Daemon)),
        Err(err) => {
            let mut result =
                execute_search_inproc(root, request, engine).map_err(|fallback_err| {
                    anyhow!(
                    "daemon search failed: {err}; in-process fallback also failed: {fallback_err}"
                )
                })?;
            annotate_reason(&mut result, format!("daemon search failed: {err}"));
            Ok((result, TransportMode::Inproc))
        }
    }
}

#[cfg(not(unix))]
fn execute_search_auto(
    root: &PathBuf,
    request: &SearchRequest,
    engine: EngineMode,
) -> Result<(SearchResult, TransportMode)> {
    execute_search_inproc(root, request, engine).map(|result| (result, TransportMode::Inproc))
}

fn execute_search_inproc(
    root: &PathBuf,
    request: &SearchRequest,
    engine: EngineMode,
) -> Result<SearchResult> {
    match engine {
        EngineMode::Legacy => {
            let mut result = packet28_reducer_core::search(root, request)?;
            annotate_fallback(&mut result, "forced legacy backend".to_string());
            Ok(result)
        }
        EngineMode::Indexed => {
            let runtime = load_runtime(root)?;
            indexed_search(root, &runtime, request)
        }
        EngineMode::Auto => match load_runtime(root) {
            Ok(runtime) => match guarded_fallback_reason(root, &runtime, request)? {
                Some(reason) => {
                    let mut result = packet28_reducer_core::search(root, request)?;
                    annotate_fallback(&mut result, reason);
                    Ok(result)
                }
                None => indexed_search(root, &runtime, request),
            },
            Err(err) => {
                let mut result = packet28_reducer_core::search(root, request)?;
                annotate_fallback(&mut result, format!("regex index load failed: {err}"));
                Ok(result)
            }
        },
    }
}

fn execute_search_daemon(
    root: &PathBuf,
    request: &SearchRequest,
    engine: EngineMode,
) -> Result<SearchResult> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    let workspace_root = resolve_workspace_root(&canonical_root);
    let daemon_request = daemon_search_request(&canonical_root, &workspace_root, request)?;
    match engine {
        EngineMode::Indexed | EngineMode::Auto => {
            let result = send_daemon_search(
                &workspace_root,
                daemon_request,
                matches!(engine, EngineMode::Indexed),
            )?;
            normalize_daemon_result(&canonical_root, &workspace_root, result)
        }
        EngineMode::Legacy => {
            let mut result = packet28_reducer_core::search(root, request)?;
            annotate_fallback(&mut result, "forced legacy backend".to_string());
            Ok(result)
        }
    }
}

fn guard_reason(
    root: &PathBuf,
    request: &SearchRequest,
    transport: TransportMode,
) -> Result<Option<String>> {
    match transport {
        TransportMode::Daemon => {
            #[cfg(unix)]
            {
                ensure_daemon(&resolve_workspace_root(root))?;
            }
            daemon_guard_reason(root, request)
        }
        TransportMode::Inproc => {
            let runtime = load_runtime(root)?;
            guarded_fallback_reason(root, &runtime, request)
        }
        TransportMode::Auto => guard_reason_auto(root, request),
    }
}

#[cfg(unix)]
fn guard_reason_auto(root: &PathBuf, request: &SearchRequest) -> Result<Option<String>> {
    let workspace_root = resolve_workspace_root(root);
    if let Err(err) = ensure_daemon(&workspace_root) {
        let runtime = load_runtime(root)?;
        return guarded_fallback_reason(root, &runtime, request)
            .map(|reason| Some(reason.unwrap_or_else(|| format!("daemon unavailable: {err}"))));
    }
    daemon_guard_reason(root, request)
}

#[cfg(not(unix))]
fn guard_reason_auto(root: &PathBuf, request: &SearchRequest) -> Result<Option<String>> {
    let runtime = load_runtime(root)?;
    guarded_fallback_reason(root, &runtime, request)
}

fn daemon_search_request(
    root: &PathBuf,
    workspace_root: &PathBuf,
    request: &SearchRequest,
) -> Result<SearchRequest> {
    if root == workspace_root {
        return Ok(request.clone());
    }
    let relative_root = root
        .strip_prefix(workspace_root)
        .map_err(|_| {
            anyhow!(
                "root '{}' is not inside workspace '{}'",
                root.display(),
                workspace_root.display()
            )
        })?
        .to_string_lossy()
        .replace('\\', "/");
    let mut adjusted = request.clone();
    adjusted.requested_paths = if request.requested_paths.is_empty() {
        vec![relative_root]
    } else {
        request
            .requested_paths
            .iter()
            .map(|path| format!("{}/{}", relative_root, path.trim_start_matches("./")))
            .collect()
    };
    Ok(adjusted)
}

fn normalize_daemon_result(
    root: &PathBuf,
    workspace_root: &PathBuf,
    mut result: SearchResult,
) -> Result<SearchResult> {
    if root == workspace_root {
        return Ok(result);
    }
    let prefix = root
        .strip_prefix(workspace_root)
        .map_err(|_| {
            anyhow!(
                "root '{}' is not inside workspace '{}'",
                root.display(),
                workspace_root.display()
            )
        })?
        .to_string_lossy()
        .replace('\\', "/");
    let path_prefix = format!("{prefix}/");
    let strip = |value: String| -> String {
        value
            .strip_prefix(&path_prefix)
            .map(ToString::to_string)
            .unwrap_or(value)
    };
    result.resolved_paths = result.resolved_paths.into_iter().map(strip).collect();
    result.paths = result.paths.into_iter().map(strip).collect();
    result.regions = result.regions.into_iter().map(strip).collect();
    for group in &mut result.groups {
        group.path = strip(group.path.clone());
        for item in &mut group.matches {
            item.path = strip(item.path.clone());
        }
    }
    result.compact_preview = result.compact_preview.replace(&path_prefix, "");
    Ok(result)
}

#[cfg(unix)]
fn daemon_guard_reason(root: &PathBuf, request: &SearchRequest) -> Result<Option<String>> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    let workspace_root = resolve_workspace_root(&canonical_root);
    let daemon_request = daemon_search_request(&canonical_root, &workspace_root, request)?;
    let response = send_daemon_guard(&workspace_root, daemon_request)?;
    Ok(response.fallback_reason)
}

#[cfg(not(unix))]
fn daemon_guard_reason(_root: &PathBuf, _request: &SearchRequest) -> Result<Option<String>> {
    Err(anyhow!(
        "daemon transport is only supported on unix platforms"
    ))
}

#[cfg(unix)]
fn send_daemon_search(
    root: &PathBuf,
    request: SearchRequest,
    force_indexed: bool,
) -> Result<SearchResult> {
    let mut stream = connect_daemon_socket(&socket_path(root))?;
    let reader_stream = stream.try_clone()?;
    let mut writer = BufWriter::new(&mut stream);
    let mut reader = BufReader::new(reader_stream);
    write_socket_message(
        &mut writer,
        &DaemonRequest::Packet28Search {
            request: DaemonPacket28SearchRequest {
                request,
                force_indexed,
            },
        },
    )?;
    match read_socket_message(&mut reader)? {
        DaemonResponse::Packet28Search { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

#[cfg(not(unix))]
fn send_daemon_search(
    _root: &PathBuf,
    _request: SearchRequest,
    _force_indexed: bool,
) -> Result<SearchResult> {
    Err(anyhow!(
        "daemon transport is only supported on unix platforms"
    ))
}

#[cfg(unix)]
fn send_daemon_guard(
    root: &PathBuf,
    request: SearchRequest,
) -> Result<Packet28SearchGuardResponse> {
    let mut stream = connect_daemon_socket(&socket_path(root))?;
    let reader_stream = stream.try_clone()?;
    let mut writer = BufWriter::new(&mut stream);
    let mut reader = BufReader::new(reader_stream);
    write_socket_message(
        &mut writer,
        &DaemonRequest::Packet28SearchGuard {
            request: DaemonPacket28SearchRequest {
                request,
                force_indexed: false,
            },
        },
    )?;
    match read_socket_message(&mut reader)? {
        DaemonResponse::Packet28SearchGuard { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

#[cfg(not(unix))]
fn send_daemon_guard(
    _root: &PathBuf,
    _request: SearchRequest,
) -> Result<Packet28SearchGuardResponse> {
    Err(anyhow!(
        "daemon transport is only supported on unix platforms"
    ))
}

fn emit_human_hits(root: &Path, result: &SearchResult, compact: bool) -> Result<()> {
    let mut line_cache = std::collections::BTreeMap::<String, Vec<String>>::new();
    for region in &result.regions {
        let Some((path, line_no)) = parse_single_line_region(region) else {
            continue;
        };
        if compact {
            println!("{path}:{line_no}");
            continue;
        }
        let line = cached_line(root, &path, line_no, &mut line_cache)?;
        println!("{path}:{line_no}:{line}");
    }
    Ok(())
}

fn parse_single_line_region(region: &str) -> Option<(String, usize)> {
    let (path, _) = region.rsplit_once(':')?;
    let (start, end) = parse_region_for_path(region, path)?;
    (start == end).then(|| (path.to_string(), start))
}

fn cached_line(
    root: &Path,
    path: &str,
    line_no: usize,
    line_cache: &mut std::collections::BTreeMap<String, Vec<String>>,
) -> Result<String> {
    if !line_cache.contains_key(path) {
        let content = std::fs::read_to_string(root.join(path))
            .with_context(|| format!("failed to read '{}'", root.join(path).display()))?;
        line_cache.insert(
            path.to_string(),
            content.lines().map(str::to_string).collect(),
        );
    }
    let lines = line_cache
        .get(path)
        .ok_or_else(|| anyhow!("missing cached lines for '{path}'"))?;
    Ok(lines
        .get(line_no.saturating_sub(1))
        .cloned()
        .unwrap_or_default())
}

fn emit_stats(label: &str, transport: TransportMode, elapsed_ms: f64, result: &SearchResult) {
    let engine = result.engine.clone().unwrap_or_default();
    eprintln!(
        "{label}_ms={elapsed_ms:.3} transport={} backend={} matches={} files={} returned={} compact_tokens={} candidates={} verified={} lookups={} postings_bytes={}",
        transport.as_str(),
        engine.engine,
        result.match_count,
        result.paths.len(),
        result.returned_match_count,
        compact_token_estimate(result),
        engine.candidates_examined,
        engine.verified_files,
        engine.index_lookups,
        engine.postings_bytes_read
    );
    if let Some(reason) = engine.fallback_reason.as_deref() {
        eprintln!("fallback_reason={reason}");
    }
}

fn emit_json(elapsed_ms: f64, transport: TransportMode, result: SearchResult) -> Result<()> {
    let payload = serde_json::json!({
        "elapsed_ms": elapsed_ms,
        "transport": transport.as_str(),
        "result": result,
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn annotate_reason(result: &mut SearchResult, reason: String) {
    let engine = result.engine.get_or_insert_with(Default::default);
    match engine.fallback_reason.take() {
        Some(existing) if !existing.is_empty() => {
            engine.fallback_reason = Some(format!("{existing}; {reason}"));
        }
        _ => engine.fallback_reason = Some(reason),
    }
}

fn annotate_fallback(result: &mut SearchResult, reason: String) {
    let engine = result.engine.get_or_insert_with(Default::default);
    engine.engine = "legacy_rg".to_string();
    annotate_reason(result, reason);
}

fn compact_token_estimate(result: &SearchResult) -> usize {
    result.compact_preview.as_bytes().len().div_ceil(4)
}

fn collect_hits(result: &SearchResult) -> Vec<String> {
    let mut hits = Vec::new();
    for group in &result.groups {
        for item in &group.matches {
            hits.push(format!("{}:{}", item.path, item.line));
        }
    }
    hits.sort();
    hits.dedup();
    hits
}

fn print_debug_search_result(
    label: &str,
    transport: TransportMode,
    elapsed_ms: f64,
    result: &SearchResult,
    compact: bool,
) {
    let engine = result.engine.clone().unwrap_or_default();
    println!(
        "{label}_ms={elapsed_ms:.3} transport={} backend={} matches={} files={} returned={} compact_tokens={} candidates={} verified={} lookups={} postings_bytes={}",
        transport.as_str(),
        engine.engine,
        result.match_count,
        result.paths.len(),
        result.returned_match_count,
        compact_token_estimate(result),
        engine.candidates_examined,
        engine.verified_files,
        engine.index_lookups,
        engine.postings_bytes_read
    );
    if let Some(reason) = engine.fallback_reason.as_deref() {
        println!("fallback_reason={reason}");
    }
    for group in &result.groups {
        for item in &group.matches {
            if compact {
                println!("hit={}#L{}", item.path, item.line);
            } else {
                println!("hit={}#L{} {}", item.path, item.line, item.text.trim());
            }
        }
    }
    if !compact {
        for region in &result.regions {
            println!("region={region}");
        }
    }
    println!(
        "compact_preview={}",
        result.compact_preview.replace('\n', "\\n")
    );
}

impl TransportMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Inproc => "inproc",
            Self::Daemon => "daemon",
        }
    }
}

#[cfg(unix)]
fn ensure_daemon(root: &Path) -> Result<()> {
    let root = resolve_workspace_root(root);
    if daemon_status_existing(&root).is_ok() {
        return Ok(());
    }
    if socket_path(&root).exists() && connect_daemon_socket(&socket_path(&root)).is_err() {
        cleanup_unreachable_runtime_files(&root)?;
    }
    start_daemon(&root)?;
    wait_for_daemon(&root, Duration::from_secs(10))
}

#[cfg(unix)]
fn start_daemon(root: &Path) -> Result<()> {
    let binary = packet28d_binary()?;
    let root_arg = root.to_string_lossy().to_string();
    let log_path = log_path(root);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create daemon log dir '{}'", parent.display()))?;
    }
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open daemon log '{}'", log_path.display()))?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open daemon log '{}'", log_path.display()))?;
    Command::new(binary)
        .arg("serve")
        .arg("--root")
        .arg(root_arg)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .context("failed to spawn packet28d")?;
    Ok(())
}

#[cfg(unix)]
fn wait_for_daemon(root: &Path, timeout: Duration) -> Result<()> {
    let start = StdInstant::now();
    while start.elapsed() < timeout {
        if daemon_status_existing(root).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    if let Ok(runtime) = read_runtime_info(root) {
        return Err(anyhow!(
            "packet28d did not become ready; runtime file exists for pid {} at {} (log: {})",
            runtime.pid,
            runtime.socket_path,
            runtime.log_path
        ));
    }
    Err(anyhow!("packet28d did not become ready"))
}

#[cfg(unix)]
fn daemon_status_existing(root: &Path) -> Result<()> {
    match send_request_existing_daemon(root, &DaemonRequest::Status) {
        Ok(DaemonResponse::Status { .. }) => Ok(()),
        Ok(DaemonResponse::Error { message }) => Err(anyhow!(message)),
        Ok(other) => Err(anyhow!("unexpected daemon status response: {other:?}")),
        Err(err) => Err(err),
    }
}

#[cfg(unix)]
fn send_request_existing_daemon(root: &Path, request: &DaemonRequest) -> Result<DaemonResponse> {
    let stream = connect_daemon_socket(&socket_path(root))?;
    let reader_stream = stream.try_clone()?;
    let mut writer = BufWriter::new(stream);
    let mut reader = BufReader::new(reader_stream);
    write_socket_message(&mut writer, request)?;
    read_socket_message(&mut reader)
}

#[cfg(unix)]
fn connect_daemon_socket(socket: &Path) -> Result<UnixStream> {
    let stream = UnixStream::connect(socket)
        .with_context(|| format!("failed to connect to '{}'", socket.display()))?;
    stream
        .set_read_timeout(Some(DAEMON_SOCKET_TIMEOUT))
        .with_context(|| {
            format!(
                "failed to configure read timeout for '{}'",
                socket.display()
            )
        })?;
    stream
        .set_write_timeout(Some(DAEMON_SOCKET_TIMEOUT))
        .with_context(|| {
            format!(
                "failed to configure write timeout for '{}'",
                socket.display()
            )
        })?;
    Ok(stream)
}

#[cfg(unix)]
fn cleanup_unreachable_runtime_files(root: &Path) -> Result<()> {
    for path in [socket_path(root), ready_path(root)] {
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| {
                format!("failed to remove stale runtime file '{}'", path.display())
            })?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn packet28d_binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_packet28d") {
        return Ok(PathBuf::from(path));
    }
    let current = std::env::current_exe().context("failed to resolve current executable")?;
    let parent = current
        .parent()
        .ok_or_else(|| anyhow!("missing executable parent"))?;
    for candidate in [
        parent.join("packet28d"),
        parent
            .parent()
            .map(|dir| dir.join("packet28d"))
            .unwrap_or_default(),
    ] {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(anyhow!(
        "could not locate packet28d next to '{}'",
        current.display()
    ))
}
