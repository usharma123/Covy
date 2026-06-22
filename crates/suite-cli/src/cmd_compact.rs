use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use packet28_daemon_core::{
    load_task_registry, task_state_json_path, BrokerGetContextResponse, BrokerWriteOp,
    BrokerWriteStateRequest,
};
use serde::Serialize;
use serde_json::{json, Value};

use crate::memory_store::{record_feedback_with_metadata, FeedbackInput, FeedbackRecord};
use crate::route_registry::{
    build_route_rewrite, decide_command_route_with_cwd_and_root, NativeToolKind, RouteKind,
};
use crate::savings_analytics::{load_run_savings, reset_run_savings, RunSavingsRecord};

#[path = "cmd_compact_session.rs"]
mod compact_session;
pub use compact_session::run_session;
#[path = "cmd_compact_economics.rs"]
mod compact_economics;
pub use compact_economics::run_cc_economics;

#[derive(Args)]
pub struct CompactArgs {
    #[command(subcommand)]
    pub command: CompactCommands,
}

#[derive(Subcommand)]
pub enum CompactCommands {
    Tree(TreeArgs),
    Read(ReadArgs),
    Grep(GrepArgs),
    Json(JsonArgs),
    Env(EnvArgs),
    Deps(DepsArgs),
    Log(LogArgs),
    Summary(SummaryArgs),
    Err(SummaryArgs),
    Test(SummaryArgs),
    Rewrite(RewriteArgs),
    Discover(AnalyticsArgs),
    Session(SessionArgs),
    FetchRaw(FetchRawArgs),
}

#[derive(Args, Clone)]
pub struct TreeArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: Option<String>,
    #[arg(long)]
    pub cwd: Option<String>,
    #[arg(long, default_value_t = 3)]
    pub max_depth: usize,
    #[arg(long, default_value_t = 200)]
    pub max_entries: usize,
    #[arg(long, default_value_t = false)]
    pub hidden: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    #[arg(default_value = ".")]
    pub paths: Vec<String>,
}

#[derive(Args, Clone)]
pub struct ReadArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: Option<String>,
    #[arg(long)]
    pub cwd: Option<String>,
    #[arg(long)]
    pub line_start: Option<usize>,
    #[arg(long)]
    pub line_end: Option<usize>,
    #[arg(long)]
    pub last: Option<usize>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    pub path: String,
}

#[derive(Args, Clone)]
pub struct GrepArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: Option<String>,
    #[arg(long)]
    pub cwd: Option<String>,
    #[arg(long, default_value_t = false)]
    pub fixed_string: bool,
    #[arg(long, default_value_t = false)]
    pub ignore_case: bool,
    #[arg(long, default_value_t = false)]
    pub whole_word: bool,
    #[arg(long)]
    pub context_lines: Option<usize>,
    #[arg(long)]
    pub max_matches_per_file: Option<usize>,
    #[arg(long)]
    pub max_total_matches: Option<usize>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    pub query: String,
    #[arg(default_value = ".")]
    pub paths: Vec<String>,
}

#[derive(Args, Clone)]
pub struct JsonArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: Option<String>,
    #[arg(long, default_value_t = 64)]
    pub max_items: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    pub path: String,
}

#[derive(Args, Clone)]
pub struct EnvArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: Option<String>,
    #[arg(long)]
    pub prefix: Option<String>,
    #[arg(long, default_value_t = false)]
    pub show_values: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args, Clone)]
pub struct DepsArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: Option<String>,
    #[arg(long, default_value_t = 80)]
    pub max_items: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    #[arg(default_value = ".")]
    pub path: String,
}

#[derive(Args, Clone)]
pub struct LogArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: Option<String>,
    #[arg(long, default_value_t = 80)]
    pub max_lines: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    pub path: String,
}

#[derive(Args, Clone)]
#[command(trailing_var_arg = true)]
pub struct SummaryArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: Option<String>,
    #[arg(long)]
    pub cwd: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    #[arg(required = true, allow_hyphen_values = true)]
    pub command_argv: Vec<String>,
}

#[derive(Args, Clone)]
#[command(trailing_var_arg = true)]
pub struct RewriteArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long, default_value = ".")]
    pub cwd: String,
    #[arg(long, default_value = "task-compact-preview")]
    pub task_id: String,
    #[arg(long)]
    pub session_id: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    #[arg(required = true, allow_hyphen_values = true)]
    pub command_argv: Vec<String>,
}

#[derive(Args, Clone)]
pub struct AnalyticsArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: Option<String>,
    /// Path to Claude-style sessions directory for gain disabled-bypass warnings
    #[arg(long)]
    pub sessions_dir: Option<String>,
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
    /// Output format for analytics commands (text, json, csv, history, failures, daily, weekly, monthly, quota, graph, all)
    #[arg(short, long, default_value = "text")]
    pub format: String,
    /// Show route graph output (RTK-compatible alias for --format graph)
    #[arg(short, long)]
    pub graph: bool,
    /// Show recent command history (RTK-compatible alias for --format history)
    #[arg(short = 'H', long)]
    pub history: bool,
    /// Show failed or fallback runs (RTK-compatible alias for --format failures)
    #[arg(short = 'F', long)]
    pub failures: bool,
    /// Persist confirmed repeated failure advice into local feedback memory
    #[arg(long)]
    pub remember_advice: bool,
    /// Show daily savings buckets (RTK-compatible alias for --format daily)
    #[arg(short, long)]
    pub daily: bool,
    /// Show weekly savings buckets (RTK-compatible alias for --format weekly)
    #[arg(short, long)]
    pub weekly: bool,
    /// Show monthly savings buckets (RTK-compatible alias for --format monthly)
    #[arg(short, long)]
    pub monthly: bool,
    /// Show quota impact (RTK-compatible alias for --format quota)
    #[arg(short, long)]
    pub quota: bool,
    /// Show all analytics sections (RTK-compatible alias for --format all)
    #[arg(short, long)]
    pub all: bool,
    /// Token budget used by gain --format quota
    #[arg(long)]
    pub quota_tokens: Option<u64>,
    /// Subscription tier used by gain --quota when --quota-tokens is not set (pro, 5x, 20x)
    #[arg(short, long, default_value = "20x")]
    pub tier: String,
    /// Reset recorded Packet28 run savings
    #[arg(long)]
    pub reset: bool,
    /// Confirm gain --reset without an interactive prompt
    #[arg(long, requires = "reset")]
    pub yes: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args, Clone)]
pub struct CcEconomicsArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    /// Include daily periods
    #[arg(long)]
    pub daily: bool,
    /// Include weekly periods
    #[arg(long)]
    pub weekly: bool,
    /// Include monthly periods
    #[arg(long)]
    pub monthly: bool,
    /// Include daily, weekly, and monthly periods
    #[arg(long)]
    pub all: bool,
    /// Output format: text, json, or csv
    #[arg(long, default_value = "text")]
    pub format: String,
    /// Read ccusage JSON from this file instead of executing ccusage/npx
    #[arg(long)]
    pub ccusage_json: Option<String>,
    /// Limit Packet28 run savings records included in the merge
    #[arg(long, default_value_t = 10_000)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args, Clone)]
pub struct SessionArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: Option<String>,
    /// Scan Claude-style session JSONL files for Packet28 adoption instead of task registry state
    #[arg(long)]
    pub sessions_dir: Option<String>,
    /// Maximum task sessions or JSONL sessions to report
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
    /// Scan every matching JSONL session instead of truncating to --limit
    #[arg(long)]
    pub all: bool,
    /// Limit JSONL session adoption scan to files modified in the last N days
    #[arg(long)]
    pub since: Option<u64>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args, Clone)]
pub struct FetchRawArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: String,
    #[arg(long)]
    pub handle: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Serialize, Default)]
struct GainSummary {
    task_count: usize,
    invocation_count: usize,
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
    saved_est_tokens: u64,
    savings_pct: f64,
    by_route: BTreeMap<String, usize>,
    route_roi: BTreeMap<String, GainRouteRoi>,
    remembered_failure_advice_count: usize,
}

#[derive(Debug, Serialize, Default)]
struct GainRouteRoi {
    invocation_count: usize,
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
    saved_est_tokens: u64,
    savings_pct: f64,
    avg_saved_tokens: f64,
    downstream_followup_count: usize,
    avg_followups_per_invocation: f64,
}

#[derive(Debug, Serialize, Default)]
struct GainBucket {
    period: String,
    invocation_count: usize,
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
    saved_est_tokens: u64,
    savings_pct: f64,
}

#[derive(Debug, Serialize)]
struct DiscoverItem {
    task_id: String,
    tool_name: String,
    request: String,
    route: String,
    reason: Option<String>,
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
    raw_artifact_available: bool,
}

pub fn run(args: CompactArgs) -> Result<i32> {
    match args.command {
        CompactCommands::Tree(args) => run_tree(args),
        CompactCommands::Read(args) => run_read(args),
        CompactCommands::Grep(args) => run_grep(args),
        CompactCommands::Json(args) => run_json(args),
        CompactCommands::Env(args) => run_env(args),
        CompactCommands::Deps(args) => run_deps(args),
        CompactCommands::Log(args) => run_log(args),
        CompactCommands::Summary(args) => run_summary(args, "summary"),
        CompactCommands::Err(args) => run_summary(args, "err"),
        CompactCommands::Test(args) => run_summary(args, "test"),
        CompactCommands::Rewrite(args) => run_rewrite_command(args),
        CompactCommands::Discover(args) => run_discover(args),
        CompactCommands::Session(args) => run_session(args),
        CompactCommands::FetchRaw(args) => run_fetch_raw(args),
    }
}

pub fn run_gain_command(args: AnalyticsArgs) -> Result<i32> {
    run_gain(args)
}

fn run_tree(args: TreeArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let cwd = resolve_cwd(args.cwd.as_deref())?;
    let mut rendered = Vec::<String>::new();
    let mut paths = Vec::<String>::new();
    {
        let mut walk_state = TreeWalkState {
            max_depth: args.max_depth,
            max_entries: args.max_entries,
            hidden: args.hidden,
            rendered: &mut rendered,
            paths: &mut paths,
        };
        for raw_path in &args.paths {
            for resolved in expand_repo_paths(&root, &cwd, raw_path)? {
                walk_tree(&root, &resolved, 0, &mut walk_state)?;
            }
        }
    }
    paths.sort();
    paths.dedup();
    let preview = rendered.join("\n");
    let payload = json!({
        "paths": paths,
        "entry_count": paths.len(),
        "compact_preview": preview,
    });
    emit_or_print(&payload, &preview, args.json, args.pretty)?;
    record_tool_result(
        &root,
        CompactToolResultRecord {
            task_id: args.task_id.as_deref(),
            tool_name: "packet28.compact.tree",
            compact_path: "native_tool",
            request_summary: "tree".to_string(),
            result_summary: preview.clone(),
            raw_est_tokens: Some(estimate_tokens_for_strings(&paths)),
            reduced_est_tokens: Some(estimate_tokens_str(&preview)),
            paths: paths.clone(),
            symbols: Vec::new(),
        },
    )?;
    Ok(0)
}

fn run_read(args: ReadArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let cwd = resolve_cwd(args.cwd.as_deref())?;
    let resolved_path = resolve_repo_path(&root, &cwd, &args.path);
    let relative =
        packet28_reducer_core::normalize_capture_path(&root, &resolved_path.display().to_string());
    let text = fs::read_to_string(&resolved_path)
        .with_context(|| format!("failed to read '{}'", resolved_path.display()))?;
    let lines = text.lines().collect::<Vec<_>>();
    let (start, end, rendered_lines) =
        select_read_lines(&lines, args.line_start, args.line_end, args.last);
    let preview = rendered_lines
        .iter()
        .enumerate()
        .map(|(idx, line)| format!("{}|{}", start + idx, line))
        .collect::<Vec<_>>()
        .join("\n");
    let payload = json!({
        "path": relative,
        "line_start": start,
        "line_end": end,
        "line_count": rendered_lines.len(),
        "compact_preview": preview,
    });
    emit_or_print(&payload, &preview, args.json, args.pretty)?;
    record_tool_result(
        &root,
        CompactToolResultRecord {
            task_id: args.task_id.as_deref(),
            tool_name: "packet28.compact.read",
            compact_path: "native_tool",
            request_summary: format!("read {}", relative),
            result_summary: preview.clone(),
            raw_est_tokens: Some(estimate_tokens_str(&text)),
            reduced_est_tokens: Some(estimate_tokens_str(&preview)),
            paths: vec![relative],
            symbols: Vec::new(),
        },
    )?;
    Ok(0)
}

fn run_grep(args: GrepArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let cwd = resolve_cwd(args.cwd.as_deref())?;
    let query = normalize_grep_query(&args.query);
    let mut requested_paths = Vec::<String>::new();
    for raw_path in &args.paths {
        for path in expand_repo_paths(&root, &cwd, raw_path)? {
            requested_paths.push(packet28_reducer_core::normalize_capture_path(
                &root,
                &path.display().to_string(),
            ));
        }
    }
    requested_paths.sort();
    requested_paths.dedup();
    let result = packet28_reducer_core::search(
        &root,
        &packet28_reducer_core::SearchRequest {
            query: query.clone(),
            requested_paths,
            fixed_string: args.fixed_string,
            case_sensitive: Some(!args.ignore_case),
            whole_word: args.whole_word,
            context_lines: args.context_lines,
            max_matches_per_file: args.max_matches_per_file,
            max_total_matches: args.max_total_matches,
        },
    )?;
    let preview = result.compact_preview.clone();
    let payload = json!({
        "query": result.query,
        "match_count": result.match_count,
        "paths": result.paths,
        "regions": result.regions,
        "symbols": result.symbols,
        "compact_preview": preview,
    });
    emit_or_print(&payload, &preview, args.json, args.pretty)?;
    record_tool_result(
        &root,
        CompactToolResultRecord {
            task_id: args.task_id.as_deref(),
            tool_name: "packet28.compact.grep",
            compact_path: "native_tool",
            request_summary: format!("grep {}", query),
            result_summary: preview.clone(),
            raw_est_tokens: Some(estimate_tokens_for_value(&payload)),
            reduced_est_tokens: Some(estimate_tokens_str(&preview)),
            paths: result.paths,
            symbols: result.symbols,
        },
    )?;
    Ok(0)
}

fn normalize_grep_query(query: &str) -> String {
    query.replace(r"\|", "|")
}

fn run_json(args: JsonArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let cwd = crate::cmd_common::caller_cwd()?;
    let path = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.path, &cwd));
    let relative =
        packet28_reducer_core::normalize_capture_path(&root, &path.display().to_string());
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read '{}'", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse JSON '{}'", path.display()))?;
    let mut lines = vec![format!("JSON {}", relative)];
    describe_json(&value, "$", &mut lines, args.max_items);
    let preview = lines.join("\n");
    let payload = json!({
        "path": relative,
        "compact_preview": preview,
    });
    emit_or_print(&payload, &preview, args.json, args.pretty)?;
    record_tool_result(
        &root,
        CompactToolResultRecord {
            task_id: args.task_id.as_deref(),
            tool_name: "packet28.compact.json",
            compact_path: "native_tool",
            request_summary: format!("json {}", relative),
            result_summary: preview.clone(),
            raw_est_tokens: Some(estimate_tokens_str(&raw)),
            reduced_est_tokens: Some(estimate_tokens_str(&preview)),
            paths: vec![relative],
            symbols: Vec::new(),
        },
    )?;
    Ok(0)
}

fn run_env(args: EnvArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let prefix = args.prefix.clone().unwrap_or_default();
    let mut entries = std::env::vars()
        .filter(|(key, _)| prefix.is_empty() || key.starts_with(&prefix))
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let preview = entries
        .iter()
        .map(|(key, value)| {
            if args.show_values {
                format!("{key}={value}")
            } else {
                format!("{key}=<redacted:{}>", value.len())
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let payload = json!({
        "prefix": args.prefix,
        "count": entries.len(),
        "compact_preview": preview,
    });
    emit_or_print(&payload, &preview, args.json, args.pretty)?;
    record_tool_result(
        &root,
        CompactToolResultRecord {
            task_id: args.task_id.as_deref(),
            tool_name: "packet28.compact.env",
            compact_path: "native_tool",
            request_summary: if prefix.is_empty() {
                "env".to_string()
            } else {
                format!("env prefix={prefix}")
            },
            result_summary: preview.clone(),
            raw_est_tokens: None,
            reduced_est_tokens: Some(estimate_tokens_str(&preview)),
            paths: Vec::new(),
            symbols: Vec::new(),
        },
    )?;
    Ok(0)
}

fn run_deps(args: DepsArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let cwd = crate::cmd_common::caller_cwd()?;
    let start = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.path, &cwd));
    let manifests = collect_dependency_manifests(&start)?;
    let mut lines = Vec::<String>::new();
    for manifest in manifests.iter().take(args.max_items) {
        lines.extend(render_manifest_dependencies(manifest)?);
    }
    if lines.is_empty() {
        lines.push("No dependency manifests found.".to_string());
    }
    let preview = lines.join("\n");
    let payload = json!({
        "manifest_count": manifests.len(),
        "compact_preview": preview,
    });
    emit_or_print(&payload, &preview, args.json, args.pretty)?;
    record_tool_result(
        &root,
        CompactToolResultRecord {
            task_id: args.task_id.as_deref(),
            tool_name: "packet28.compact.deps",
            compact_path: "native_tool",
            request_summary: "deps".to_string(),
            result_summary: preview.clone(),
            raw_est_tokens: None,
            reduced_est_tokens: Some(estimate_tokens_str(&preview)),
            paths: manifests
                .iter()
                .map(|path| {
                    packet28_reducer_core::normalize_capture_path(
                        &root,
                        &path.display().to_string(),
                    )
                })
                .collect(),
            symbols: Vec::new(),
        },
    )?;
    Ok(0)
}

fn run_log(args: LogArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let cwd = crate::cmd_common::caller_cwd()?;
    let path = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.path, &cwd));
    let relative =
        packet28_reducer_core::normalize_capture_path(&root, &path.display().to_string());
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read '{}'", path.display()))?;
    let lines = raw.lines().rev().take(args.max_lines).collect::<Vec<_>>();
    let mut grouped = Vec::<(String, usize)>::new();
    for line in lines.into_iter().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((last, count)) = grouped.last_mut() {
            if last == trimmed {
                *count += 1;
                continue;
            }
        }
        grouped.push((trimmed.to_string(), 1));
    }
    let preview = grouped
        .into_iter()
        .map(|(line, count)| {
            if count > 1 {
                format!("[x{count}] {line}")
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let payload = json!({
        "path": relative,
        "compact_preview": preview,
    });
    emit_or_print(&payload, &preview, args.json, args.pretty)?;
    record_tool_result(
        &root,
        CompactToolResultRecord {
            task_id: args.task_id.as_deref(),
            tool_name: "packet28.compact.log",
            compact_path: "native_tool",
            request_summary: format!("log {}", relative),
            result_summary: preview.clone(),
            raw_est_tokens: Some(estimate_tokens_str(&raw)),
            reduced_est_tokens: Some(estimate_tokens_str(&preview)),
            paths: vec![relative],
            symbols: Vec::new(),
        },
    )?;
    Ok(0)
}

fn run_summary(args: SummaryArgs, label: &str) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let cwd = resolve_cwd(args.cwd.as_deref())?;
    let request = suite_proxy_core::ProxyRunRequest {
        argv: args.command_argv.clone(),
        cwd: Some(cwd.display().to_string()),
        ..suite_proxy_core::ProxyRunRequest::default()
    };
    match suite_proxy_core::run_and_reduce(request) {
        Ok(envelope) => {
            let preview = envelope.payload.highlights.join("\n");
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(&envelope)?, args.pretty)?;
            } else if preview.is_empty() {
                println!("{}", envelope.summary);
            } else {
                println!("{preview}");
            }
            record_tool_result(
                &root,
                CompactToolResultRecord {
                    task_id: args.task_id.as_deref(),
                    tool_name: &format!("packet28.compact.{label}"),
                    compact_path: "proxy_passthrough",
                    request_summary: args.command_argv.join(" "),
                    result_summary: envelope.summary.clone(),
                    raw_est_tokens: Some(((envelope.payload.bytes_in as f64) / 4.0).ceil() as u64),
                    reduced_est_tokens: Some(
                        ((envelope.payload.bytes_out as f64) / 4.0).ceil() as u64
                    ),
                    paths: envelope
                        .files
                        .iter()
                        .map(|file| file.path.clone())
                        .collect(),
                    symbols: Vec::new(),
                },
            )?;
            Ok(if envelope.payload.exit_code == 0 {
                0
            } else {
                1
            })
        }
        Err(error) => {
            record_tool_failure(
                &root,
                args.task_id.as_deref(),
                &format!("packet28.compact.{label}"),
                "proxy_passthrough",
                args.command_argv.join(" "),
                error.to_string(),
            )?;
            Err(anyhow!(error.to_string()))
        }
    }
}

pub fn run_rewrite_command(args: RewriteArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let command = args.command_argv.join(" ");
    let cwd_path = std::path::Path::new(&args.cwd);
    let decision = decide_command_route_with_cwd_and_root(&command, cwd_path, &root);
    let rewritten = build_route_rewrite(
        &root,
        &args.task_id,
        args.session_id.as_deref(),
        &args.cwd,
        &decision,
    );
    let native_kind = decision.native_tool.as_ref().map(|tool| match tool.kind {
        NativeToolKind::Tree => "tree",
        NativeToolKind::Read => "read",
        NativeToolKind::Grep => "grep",
        NativeToolKind::Env => "env",
    });
    let payload = json!({
        "command": command,
        "route": match decision.kind {
            RouteKind::ReducerRewrite => "reducer_rewrite",
            RouteKind::NativeTool => "native_tool",
            RouteKind::TomlFilterRewrite => "toml_filter_rewrite",
            RouteKind::CompoundRewrite => "compound_rewrite",
            RouteKind::ProxyPassthrough => "proxy_passthrough",
            RouteKind::RawPassthrough => "raw_passthrough",
        },
        "reason": decision.reason,
        "env_assignments": decision.env_assignments,
        "native_tool": native_kind,
        "rewritten_command": rewritten,
        "reducer_family": decision.reducer_spec.as_ref().map(|spec| spec.family.clone()),
        "reducer_kind": decision
            .reducer_spec
            .as_ref()
            .map(|spec| spec.canonical_kind.clone()),
    });
    if args.json {
        crate::cmd_common::emit_json(&payload, args.pretty)?;
    } else if let Some(rewritten) = payload["rewritten_command"].as_str() {
        println!("{rewritten}");
    }
    Ok(0)
}

fn run_gain(args: AnalyticsArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    if args.reset {
        if !args.yes {
            anyhow::bail!("gain --reset requires --yes in non-interactive Packet28 mode");
        }
        let cleared = reset_run_savings(&root)?;
        if args.json {
            crate::cmd_common::emit_json(
                &json!({
                    "reset": true,
                    "cleared_run_savings": cleared,
                }),
                args.pretty,
            )?;
        } else {
            println!("Token savings stats reset to zero.");
            println!("cleared_run_savings={cleared}");
        }
        return Ok(0);
    }
    let states = load_task_states(&root, args.task_id.as_deref(), args.limit)?;
    let mut summary = GainSummary {
        task_count: states.len(),
        ..GainSummary::default()
    };
    for (_, state) in states {
        for invocation in state.recent_tool_invocations {
            summary.invocation_count += 1;
            summary.raw_est_tokens += invocation.raw_est_tokens.unwrap_or(0);
            summary.reduced_est_tokens += invocation.reduced_est_tokens.unwrap_or(0);
            let route = invocation
                .compact_path
                .unwrap_or_else(|| "unknown".to_string());
            *summary.by_route.entry(route.clone()).or_insert(0) += 1;
            record_gain_route_roi(
                &mut summary.route_roi,
                &route,
                invocation.raw_est_tokens.unwrap_or(0),
                invocation.reduced_est_tokens.unwrap_or(0),
            );
        }
    }
    let run_savings = load_run_savings(&root, args.limit)?;
    if args.remember_advice {
        summary.remembered_failure_advice_count = remember_failure_advice(&run_savings)?.len();
    }
    for record in &run_savings {
        summary.invocation_count += 1;
        summary.raw_est_tokens += record.raw_est_tokens;
        summary.reduced_est_tokens += record.reduced_est_tokens;
        let route = gain_route_for_run_savings(record);
        *summary.by_route.entry(route.clone()).or_insert(0) += 1;
        record_gain_route_roi(
            &mut summary.route_roi,
            &route,
            record.raw_est_tokens,
            record.reduced_est_tokens,
        );
    }
    record_gain_route_followups(&mut summary.route_roi, &run_savings);
    summary.saved_est_tokens = summary
        .raw_est_tokens
        .saturating_sub(summary.reduced_est_tokens);
    summary.savings_pct = pct_saved(summary.raw_est_tokens, summary.reduced_est_tokens);
    let format = gain_output_format(&args);
    if args.json || format == "json" {
        crate::cmd_common::emit_json(&serde_json::to_value(summary)?, args.pretty)?;
    } else if format == "csv" {
        print_gain_csv(&summary);
    } else if format == "history" {
        print_gain_history(&run_savings);
    } else if format == "failures" {
        print_gain_failures(&run_savings);
        if args.remember_advice {
            println!(
                "remembered_failure_advice_count={}",
                summary.remembered_failure_advice_count
            );
        }
    } else if format == "daily" {
        print_gain_buckets(&run_savings, GainBucketKind::Daily);
    } else if format == "weekly" {
        print_gain_buckets(&run_savings, GainBucketKind::Weekly);
    } else if format == "monthly" {
        print_gain_buckets(&run_savings, GainBucketKind::Monthly);
    } else if format == "quota" {
        print_gain_quota(&summary, args.quota_tokens, &args.tier);
    } else if format == "graph" {
        print_gain_graph(&summary);
    } else if format == "all" {
        print_gain_all(&summary, &run_savings, args.quota_tokens, &args.tier);
    } else {
        if let Some(warning) = gain_disabled_bypass_warning(args.sessions_dir.as_deref()) {
            eprintln!("{warning}");
            eprintln!();
        }
        println!("tasks={}", summary.task_count);
        println!("invocations={}", summary.invocation_count);
        println!("raw_est_tokens={}", summary.raw_est_tokens);
        println!("reduced_est_tokens={}", summary.reduced_est_tokens);
        println!("saved_est_tokens={}", summary.saved_est_tokens);
        println!("savings_pct={:.1}", summary.savings_pct);
        for (route, count) in summary.by_route {
            println!("route.{route}={count}");
        }
    }
    Ok(0)
}

fn record_gain_route_roi(
    route_roi: &mut BTreeMap<String, GainRouteRoi>,
    route: &str,
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
) {
    let entry = route_roi.entry(route.to_string()).or_default();
    entry.invocation_count += 1;
    entry.raw_est_tokens = entry.raw_est_tokens.saturating_add(raw_est_tokens);
    entry.reduced_est_tokens = entry.reduced_est_tokens.saturating_add(reduced_est_tokens);
    entry.saved_est_tokens = entry
        .raw_est_tokens
        .saturating_sub(entry.reduced_est_tokens);
    entry.savings_pct = pct_saved(entry.raw_est_tokens, entry.reduced_est_tokens);
    entry.avg_saved_tokens = if entry.invocation_count == 0 {
        0.0
    } else {
        entry.saved_est_tokens as f64 / entry.invocation_count as f64
    };
    entry.avg_followups_per_invocation = if entry.invocation_count == 0 {
        0.0
    } else {
        entry.downstream_followup_count as f64 / entry.invocation_count as f64
    };
}

fn record_gain_route_followups(
    route_roi: &mut BTreeMap<String, GainRouteRoi>,
    records: &[RunSavingsRecord],
) {
    let mut ordered = records.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| a.timestamp_unix_ms.cmp(&b.timestamp_unix_ms));
    for (index, record) in ordered.iter().enumerate() {
        let followups = count_downstream_followups(&ordered, index);
        if followups == 0 {
            continue;
        }
        let route = gain_route_for_run_savings(record);
        let entry = route_roi.entry(route).or_default();
        entry.downstream_followup_count = entry.downstream_followup_count.saturating_add(followups);
        entry.avg_followups_per_invocation = if entry.invocation_count == 0 {
            0.0
        } else {
            entry.downstream_followup_count as f64 / entry.invocation_count as f64
        };
    }
}

fn count_downstream_followups(records: &[&RunSavingsRecord], index: usize) -> usize {
    const FOLLOWUP_WINDOW_MS: u128 = 10 * 60 * 1000;
    let Some(record) = records.get(index) else {
        return 0;
    };
    records
        .iter()
        .skip(index + 1)
        .take_while(|candidate| {
            candidate
                .timestamp_unix_ms
                .saturating_sub(record.timestamp_unix_ms)
                <= FOLLOWUP_WINDOW_MS
        })
        .filter(|candidate| candidate.cwd == record.cwd)
        .count()
}

fn gain_route_for_run_savings(record: &RunSavingsRecord) -> String {
    if let Some(reason) = &record.fallback_reason {
        format!("run_fallback:{reason}")
    } else {
        format!("run_reducer:{}", record.family)
    }
}

fn gain_disabled_bypass_warning(sessions_dir: Option<&str>) -> Option<String> {
    let sessions_dir = sessions_dir.map(PathBuf::from).unwrap_or_else(|| {
        #[cfg(unix)]
        {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(format!("{home}/.claude/projects"));
            }
        }
        PathBuf::from("/tmp/.claude/projects")
    });
    let files =
        crate::cmd_discover::collect_session_files_for_scan(&sessions_dir, 200, false, Some(7))
            .ok()?;
    if files.is_empty() || files.len() > 200 {
        return None;
    }

    let mut total = 0usize;
    let mut bypassed = 0usize;
    for path in files {
        let Ok(commands) = crate::cmd_discover::extract_bash_commands(&path) else {
            continue;
        };
        for (command, _) in commands {
            total += 1;
            if let Some(actual_command) =
                crate::cmd_discover::strip_active_disabled_prefix(&command)
            {
                let route = crate::route_registry::decide_command_route(&actual_command);
                if !matches!(route.kind, crate::route_registry::RouteKind::RawPassthrough) {
                    bypassed += 1;
                }
            }
        }
    }

    if total == 0 {
        return None;
    }
    let pct = (bypassed as f64 / total as f64) * 100.0;
    (pct > 10.0).then(|| {
        format!(
            "[warn] {bypassed} commands ({pct:.0}%) used PACKET28_DISABLED/RTK_DISABLED unnecessarily - run `Packet28 discover` for details"
        )
    })
}

fn gain_output_format(args: &AnalyticsArgs) -> String {
    let explicit = args.format.trim().to_ascii_lowercase();
    if args.json || explicit != "text" {
        return explicit;
    }
    if args.graph {
        "graph"
    } else if args.history {
        "history"
    } else if args.failures {
        "failures"
    } else if args.daily {
        "daily"
    } else if args.weekly {
        "weekly"
    } else if args.monthly {
        "monthly"
    } else if args.quota {
        "quota"
    } else if args.all {
        "all"
    } else {
        "text"
    }
    .to_string()
}

#[derive(Debug, Clone, Copy)]
enum GainBucketKind {
    Daily,
    Weekly,
    Monthly,
}

fn print_gain_buckets(records: &[RunSavingsRecord], kind: GainBucketKind) {
    let mut buckets = BTreeMap::<String, GainBucket>::new();
    for record in records {
        let label = gain_bucket_label(record.timestamp_unix_ms, kind);
        let bucket = buckets.entry(label.clone()).or_insert_with(|| GainBucket {
            period: label,
            ..GainBucket::default()
        });
        bucket.invocation_count += 1;
        bucket.raw_est_tokens += record.raw_est_tokens;
        bucket.reduced_est_tokens += record.reduced_est_tokens;
    }
    println!(
        "period,invocation_count,raw_est_tokens,reduced_est_tokens,saved_est_tokens,savings_pct"
    );
    for bucket in buckets.values_mut() {
        bucket.saved_est_tokens = bucket
            .raw_est_tokens
            .saturating_sub(bucket.reduced_est_tokens);
        bucket.savings_pct = pct_saved(bucket.raw_est_tokens, bucket.reduced_est_tokens);
        println!(
            "{},{},{},{},{},{:.1}",
            csv_cell(&bucket.period),
            bucket.invocation_count,
            bucket.raw_est_tokens,
            bucket.reduced_est_tokens,
            bucket.saved_est_tokens,
            bucket.savings_pct
        );
    }
}

fn print_gain_quota(summary: &GainSummary, quota_tokens: Option<u64>, tier: &str) {
    let (quota, tier_label) = quota_tokens
        .map(|tokens| (tokens, "custom"))
        .unwrap_or_else(|| tier_quota_tokens(tier));
    let used_pct = if quota == 0 {
        0.0
    } else {
        (summary.reduced_est_tokens as f64 / quota as f64) * 100.0
    };
    let avoided_pct = if quota == 0 {
        0.0
    } else {
        (summary.saved_est_tokens as f64 / quota as f64) * 100.0
    };
    println!("tier={tier_label}");
    println!("quota_tokens={quota}");
    println!("raw_est_tokens={}", summary.raw_est_tokens);
    println!("reduced_est_tokens={}", summary.reduced_est_tokens);
    println!("saved_est_tokens={}", summary.saved_est_tokens);
    println!("quota_used_pct={used_pct:.1}");
    println!("quota_avoided_pct={avoided_pct:.1}");
}

fn tier_quota_tokens(tier: &str) -> (u64, &'static str) {
    const PRO_MONTHLY_TOKENS: u64 = 6_000_000;
    match tier {
        "pro" => (PRO_MONTHLY_TOKENS, "pro"),
        "5x" => (PRO_MONTHLY_TOKENS * 5, "5x"),
        "20x" => (PRO_MONTHLY_TOKENS * 20, "20x"),
        _ => (PRO_MONTHLY_TOKENS, "pro"),
    }
}

fn print_gain_graph(summary: &GainSummary) {
    println!("Packet28 savings graph");
    println!("invocations={}", summary.invocation_count);
    println!("saved_est_tokens={}", summary.saved_est_tokens);
    println!("savings_pct={:.1}", summary.savings_pct);
    println!();
    println!("{:<36} {:>7} {:>7}  impact", "route", "count", "share");
    let total = summary.invocation_count.max(1);
    for (route, count) in &summary.by_route {
        let share = (*count as f64 / total as f64) * 100.0;
        let width = ((*count * 40) / total).max(1);
        println!(
            "{:<36} {:>7} {:>6.1}%  {}",
            graph_route_cell(route, 36),
            count,
            share,
            "#".repeat(width)
        );
    }
}

fn graph_route_cell(route: &str, max_width: usize) -> String {
    if route.len() <= max_width {
        return route.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    format!("{}...", &route[..max_width - 3])
}

fn print_gain_all(
    summary: &GainSummary,
    run_savings: &[RunSavingsRecord],
    quota_tokens: Option<u64>,
    tier: &str,
) {
    println!("[summary]");
    println!("tasks={}", summary.task_count);
    println!("invocations={}", summary.invocation_count);
    println!("raw_est_tokens={}", summary.raw_est_tokens);
    println!("reduced_est_tokens={}", summary.reduced_est_tokens);
    println!("saved_est_tokens={}", summary.saved_est_tokens);
    println!("savings_pct={:.1}", summary.savings_pct);
    println!();
    println!("[graph]");
    print_gain_graph(summary);
    println!();
    println!("[daily]");
    print_gain_buckets(run_savings, GainBucketKind::Daily);
    println!();
    println!("[weekly]");
    print_gain_buckets(run_savings, GainBucketKind::Weekly);
    println!();
    println!("[monthly]");
    print_gain_buckets(run_savings, GainBucketKind::Monthly);
    println!();
    println!("[quota]");
    print_gain_quota(summary, quota_tokens, tier);
    println!();
    println!("[failures]");
    print_gain_failures(run_savings);
}

fn print_gain_history(records: &[RunSavingsRecord]) {
    println!("timestamp_unix_ms,family,raw_est_tokens,reduced_est_tokens,savings_percent,exit_code,command");
    for record in records {
        println!(
            "{},{},{},{},{:.1},{},{}",
            record.timestamp_unix_ms,
            csv_cell(&record.family),
            record.raw_est_tokens,
            record.reduced_est_tokens,
            record.savings_percent,
            record.exit_code,
            csv_cell(&record.command)
        );
    }
}

fn print_gain_failures(records: &[RunSavingsRecord]) {
    let mut repeat_counts = BTreeMap::<String, usize>::new();
    for record in records {
        if let Some(fingerprint) = record.failure_fingerprint.as_deref() {
            *repeat_counts.entry(fingerprint.to_string()).or_insert(0) += 1;
        }
    }
    println!("timestamp_unix_ms,family,exit_code,fallback_reason,failure_fingerprint,repeat_count,next_success_command,next_success_changed_paths,fix_advice,command");
    for record in records
        .iter()
        .filter(|record| record.exit_code != 0 || record.fallback_reason.is_some())
    {
        let repeat_count = record
            .failure_fingerprint
            .as_ref()
            .and_then(|fingerprint| repeat_counts.get(fingerprint))
            .copied()
            .unwrap_or(0);
        let next_success = next_success_record(records, record);
        let next_success_command = next_success
            .map(|record| record.command.clone())
            .unwrap_or_default();
        let next_success_changed_paths = next_success
            .map(|record| record.changed_paths.join(";"))
            .unwrap_or_default();
        let fix_advice = failure_fix_advice(
            repeat_count,
            next_success_command.as_str(),
            next_success_changed_paths.as_str(),
        );
        println!(
            "{},{},{},{},{},{},{},{},{},{}",
            record.timestamp_unix_ms,
            csv_cell(&record.family),
            record.exit_code,
            csv_cell(record.fallback_reason.as_deref().unwrap_or("")),
            csv_cell(record.failure_fingerprint.as_deref().unwrap_or("")),
            repeat_count,
            csv_cell(&next_success_command),
            csv_cell(&next_success_changed_paths),
            csv_cell(&fix_advice),
            csv_cell(&record.command)
        );
    }
}

fn remember_failure_advice(records: &[RunSavingsRecord]) -> Result<Vec<FeedbackRecord>> {
    let repeat_counts = failure_repeat_counts(records);
    let mut remembered = Vec::new();
    let mut seen_fingerprints = BTreeMap::<String, ()>::new();
    for record in records
        .iter()
        .filter(|record| record.exit_code != 0 || record.fallback_reason.is_some())
    {
        let Some(fingerprint) = record.failure_fingerprint.as_deref() else {
            continue;
        };
        let repeat_count = repeat_counts.get(fingerprint).copied().unwrap_or(0);
        if repeat_count <= 1 || seen_fingerprints.contains_key(fingerprint) {
            continue;
        }
        let Some(next_success) = next_success_record(records, record) else {
            continue;
        };
        let changed_paths = next_success.changed_paths.join(";");
        let correction = failure_fix_advice(
            repeat_count,
            next_success.command.as_str(),
            changed_paths.as_str(),
        );
        let context = format!(
            "failure_fingerprint={fingerprint}\nrepeat_count={repeat_count}\nfailed_command={}\nnext_success_command={}\nnext_success_changed_paths={changed_paths}",
            record.command, next_success.command
        );
        let feedback = record_feedback_with_metadata(FeedbackInput {
            subject: &format!("failure_fingerprint:{fingerprint}"),
            correction: &correction,
            topic: Some("failure-advice"),
            context: Some(&context),
            predicted: Some(&record.command),
            reason: Some("confirmed by later successful same-directory run"),
            source: Some("packet28.gain.failures"),
            project: Some(&record.cwd),
        })?;
        seen_fingerprints.insert(fingerprint.to_string(), ());
        remembered.push(feedback);
    }
    Ok(remembered)
}

fn failure_repeat_counts(records: &[RunSavingsRecord]) -> BTreeMap<String, usize> {
    let mut repeat_counts = BTreeMap::<String, usize>::new();
    for record in records {
        if let Some(fingerprint) = record.failure_fingerprint.as_deref() {
            *repeat_counts.entry(fingerprint.to_string()).or_insert(0) += 1;
        }
    }
    repeat_counts
}

fn failure_fix_advice(
    repeat_count: usize,
    next_success_command: &str,
    next_success_changed_paths: &str,
) -> String {
    if next_success_command.is_empty() {
        return "no later successful same-directory run observed".to_string();
    }
    let prefix = if repeat_count > 1 {
        "repeated failure"
    } else {
        "failure"
    };
    if next_success_changed_paths.is_empty() {
        format!("{prefix}: retry with `{next_success_command}`")
    } else {
        format!("{prefix}: retry with `{next_success_command}` after inspecting {next_success_changed_paths}")
    }
}

fn next_success_record<'a>(
    records: &'a [RunSavingsRecord],
    failed: &RunSavingsRecord,
) -> Option<&'a RunSavingsRecord> {
    records
        .iter()
        .filter(|candidate| {
            candidate.timestamp_unix_ms > failed.timestamp_unix_ms
                && candidate.cwd == failed.cwd
                && candidate.exit_code == 0
        })
        .min_by_key(|candidate| candidate.timestamp_unix_ms)
}

fn gain_bucket_label(timestamp_unix_ms: u128, kind: GainBucketKind) -> String {
    let day = (timestamp_unix_ms / 86_400_000) as i64;
    match kind {
        GainBucketKind::Daily => format_ymd(day),
        GainBucketKind::Weekly => {
            let weekday_monday_zero = (day + 3).rem_euclid(7);
            let week_start = day - weekday_monday_zero;
            format!("week_of_{}", format_ymd(week_start))
        }
        GainBucketKind::Monthly => {
            let (year, month, _) = civil_from_unix_day(day);
            format!("{year:04}-{month:02}")
        }
    }
}

fn format_ymd(unix_day: i64) -> String {
    let (year, month, day) = civil_from_unix_day(unix_day);
    format!("{year:04}-{month:02}-{day:02}")
}

fn civil_from_unix_day(unix_day: i64) -> (i32, u32, u32) {
    let z = unix_day + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn print_gain_csv(summary: &GainSummary) {
    println!("kind,name,value");
    println!("summary,task_count,{}", summary.task_count);
    println!("summary,invocation_count,{}", summary.invocation_count);
    println!("summary,raw_est_tokens,{}", summary.raw_est_tokens);
    println!("summary,reduced_est_tokens,{}", summary.reduced_est_tokens);
    println!("summary,saved_est_tokens,{}", summary.saved_est_tokens);
    println!("summary,savings_pct,{:.1}", summary.savings_pct);
    for (route, count) in &summary.by_route {
        println!("route,{},{}", csv_cell(route), count);
    }
}

fn csv_cell(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn run_discover(args: AnalyticsArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let states = load_task_states(&root, args.task_id.as_deref(), args.limit)?;
    let mut items = Vec::<DiscoverItem>::new();
    for (task_id, state) in states {
        for invocation in state.recent_tool_invocations {
            let route = invocation
                .compact_path
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let raw = invocation.raw_est_tokens.unwrap_or(0);
            let reduced = invocation.reduced_est_tokens.unwrap_or(0);
            let missed = route == "raw_passthrough"
                || invocation.passthrough_reason.is_some()
                || (raw > 0 && pct_saved(raw, reduced) < 50.0);
            if missed {
                items.push(DiscoverItem {
                    task_id: task_id.clone(),
                    tool_name: invocation.tool_name,
                    request: invocation
                        .request_summary
                        .unwrap_or_else(|| "no request summary".to_string()),
                    route,
                    reason: invocation.passthrough_reason,
                    raw_est_tokens: raw,
                    reduced_est_tokens: reduced,
                    raw_artifact_available: invocation.raw_artifact_available,
                });
            }
        }
    }
    items.sort_by(|a, b| b.raw_est_tokens.cmp(&a.raw_est_tokens));
    items.truncate(args.limit.max(1));
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(items)?, args.pretty)?;
    } else if items.is_empty() {
        println!("No missed-savings candidates found.");
    } else {
        for item in items {
            println!(
                "{} {} route={} raw={} reduced={} reason={}",
                item.task_id,
                item.tool_name,
                item.route,
                item.raw_est_tokens,
                item.reduced_est_tokens,
                item.reason.unwrap_or_else(|| "n/a".to_string())
            );
        }
    }
    Ok(0)
}

fn run_fetch_raw(args: FetchRawArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let handle = PathBuf::from(&args.handle);
    let path = if handle.is_absolute() {
        handle
    } else {
        let candidate = root.join(handle);
        if candidate.exists() {
            candidate
        } else {
            task_state_json_path(&root, &args.task_id)
                .parent()
                .unwrap_or(&root)
                .join(&args.handle)
        }
    };
    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read raw artifact '{}'", path.display()))?;
    if args.json {
        crate::cmd_common::emit_json(
            &json!({
                "task_id": args.task_id,
                "handle": args.handle,
                "path": path.display().to_string(),
                "content": text,
            }),
            args.pretty,
        )?;
    } else {
        print!("{text}");
    }
    Ok(0)
}

fn emit_or_print(payload: &Value, preview: &str, json: bool, pretty: bool) -> Result<()> {
    if json {
        crate::cmd_common::emit_json(payload, pretty)
    } else {
        println!("{preview}");
        Ok(())
    }
}

fn resolve_root(root: &str) -> Result<PathBuf> {
    let cwd = crate::cmd_common::caller_cwd()?;
    Ok(PathBuf::from(crate::cmd_common::resolve_path_from_cwd(
        root, &cwd,
    )))
}

fn resolve_cwd(cwd: Option<&str>) -> Result<PathBuf> {
    let caller_cwd = crate::cmd_common::caller_cwd()?;
    Ok(match cwd {
        Some(path) => PathBuf::from(crate::cmd_common::resolve_path_from_cwd(path, &caller_cwd)),
        None => caller_cwd,
    })
}

fn resolve_repo_path(_root: &Path, cwd: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    let absolute = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    absolute.canonicalize().unwrap_or(absolute)
}

fn expand_repo_paths(root: &Path, cwd: &Path, value: &str) -> Result<Vec<PathBuf>> {
    if !looks_like_glob_pattern(value) {
        return Ok(vec![resolve_repo_path(root, cwd, value)]);
    }
    let pattern = if Path::new(value).is_absolute() {
        value.to_string()
    } else {
        cwd.join(value).display().to_string()
    };
    let mut matches = glob::glob(&pattern)
        .with_context(|| format!("invalid glob pattern '{value}'"))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    matches.sort();
    matches.dedup();
    if matches.is_empty() {
        Ok(vec![resolve_repo_path(root, cwd, value)])
    } else {
        Ok(matches)
    }
}

fn looks_like_glob_pattern(value: &str) -> bool {
    value.contains('*') || value.contains('?') || value.contains('[')
}

struct TreeWalkState<'a> {
    max_depth: usize,
    max_entries: usize,
    hidden: bool,
    rendered: &'a mut Vec<String>,
    paths: &'a mut Vec<String>,
}

fn walk_tree(root: &Path, path: &Path, depth: usize, state: &mut TreeWalkState<'_>) -> Result<()> {
    if state.rendered.len() >= state.max_entries {
        return Ok(());
    }
    let relative = packet28_reducer_core::normalize_capture_path(root, &path.display().to_string());
    if !relative.is_empty() {
        let name = if depth == 0 {
            relative.clone()
        } else {
            format!("{}{}", "  ".repeat(depth), relative)
        };
        state.rendered.push(name);
        state.paths.push(relative.clone());
    }
    if depth >= state.max_depth || !path.is_dir() {
        return Ok(());
    }
    let mut children = fs::read_dir(path)
        .with_context(|| format!("failed to read '{}'", path.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    children.sort();
    for child in children {
        let name = child
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if (!state.hidden && name.starts_with('.')) || name == ".git" || name == ".packet28" {
            continue;
        }
        walk_tree(root, &child, depth + 1, state)?;
        if state.rendered.len() >= state.max_entries {
            break;
        }
    }
    Ok(())
}

fn select_read_lines<'a>(
    lines: &'a [&'a str],
    line_start: Option<usize>,
    line_end: Option<usize>,
    last: Option<usize>,
) -> (usize, usize, Vec<&'a str>) {
    if let Some(last) = last.filter(|value| *value > 0) {
        let start_idx = lines.len().saturating_sub(last);
        let rendered = lines[start_idx..].to_vec();
        return (start_idx + 1, lines.len(), rendered);
    }
    let start = line_start.unwrap_or(1).max(1);
    let end = line_end.unwrap_or(start + 79).max(start);
    let start_idx = start.saturating_sub(1).min(lines.len());
    let end_idx = end.min(lines.len());
    (start, end_idx, lines[start_idx..end_idx].to_vec())
}

fn describe_json(value: &Value, path: &str, lines: &mut Vec<String>, max_items: usize) {
    if lines.len() >= max_items {
        return;
    }
    match value {
        Value::Object(map) => {
            lines.push(format!("{path}: object({})", map.len()));
            for (key, child) in map.iter().take(max_items.saturating_sub(lines.len())) {
                describe_json(child, &format!("{path}.{key}"), lines, max_items);
            }
        }
        Value::Array(items) => {
            lines.push(format!("{path}: array({})", items.len()));
            if let Some(first) = items.first() {
                describe_json(first, &format!("{path}[0]"), lines, max_items);
            }
        }
        Value::String(_) => lines.push(format!("{path}: string")),
        Value::Number(_) => lines.push(format!("{path}: number")),
        Value::Bool(_) => lines.push(format!("{path}: bool")),
        Value::Null => lines.push(format!("{path}: null")),
    }
}

fn collect_dependency_manifests(start: &Path) -> Result<Vec<PathBuf>> {
    let mut manifests = Vec::<PathBuf>::new();
    walk_manifests(start, 0, 4, &mut manifests)?;
    manifests.sort();
    manifests.dedup();
    Ok(manifests)
}

fn walk_manifests(
    path: &Path,
    depth: usize,
    max_depth: usize,
    manifests: &mut Vec<PathBuf>,
) -> Result<()> {
    if depth > max_depth {
        return Ok(());
    }
    if path.is_file() {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if matches!(name, "Cargo.toml" | "package.json" | "pyproject.toml") {
            manifests.push(path.to_path_buf());
        }
        return Ok(());
    }
    for entry in
        fs::read_dir(path).with_context(|| format!("failed to read '{}'", path.display()))?
    {
        let child = entry?.path();
        let name = child
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if matches!(name, ".git" | ".packet28" | "node_modules" | "target") {
            continue;
        }
        walk_manifests(&child, depth + 1, max_depth, manifests)?;
    }
    Ok(())
}

fn render_manifest_dependencies(path: &Path) -> Result<Vec<String>> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read '{}'", path.display()))?;
    let mut lines = vec![format!("manifest {}", path.display())];
    match name {
        "package.json" => {
            let value: Value = serde_json::from_str(&raw)?;
            for section in ["dependencies", "devDependencies", "peerDependencies"] {
                if let Some(map) = value.get(section).and_then(Value::as_object) {
                    let deps = map
                        .iter()
                        .take(12)
                        .map(|(key, value)| {
                            format!("- {section}: {key}@{}", value.as_str().unwrap_or("?"))
                        })
                        .collect::<Vec<_>>();
                    lines.extend(deps);
                }
            }
        }
        "Cargo.toml" | "pyproject.toml" => {
            let value: toml::Value = toml::from_str(&raw)?;
            for section in [
                "dependencies",
                "dev-dependencies",
                "build-dependencies",
                "project",
            ] {
                if let Some(table) = value.get(section).and_then(toml::Value::as_table) {
                    for (key, value) in table.iter().take(12) {
                        lines.push(format!("- {section}: {key}={}", render_toml_value(value)));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(lines)
}

fn render_toml_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        toml::Value::Table(table) => table
            .iter()
            .take(3)
            .map(|(key, value)| format!("{key}={}", render_toml_value(value)))
            .collect::<Vec<_>>()
            .join(","),
        other => other.to_string(),
    }
}

fn load_task_state(root: &Path, task_id: &str) -> Result<BrokerGetContextResponse> {
    let bytes = fs::read(task_state_json_path(root, task_id))
        .with_context(|| format!("failed to read task state for '{}'", task_id))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn load_task_states(
    root: &Path,
    task_id: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, BrokerGetContextResponse)>> {
    let registry = load_task_registry(root)?;
    let mut task_ids = registry.tasks.into_keys().collect::<Vec<_>>();
    task_ids.sort();
    if let Some(task_id) = task_id {
        task_ids.retain(|candidate| candidate == task_id);
    }
    let mut states = Vec::<(String, BrokerGetContextResponse)>::new();
    for task_id in task_ids.into_iter().take(limit.max(1)) {
        if let Ok(state) = load_task_state(root, &task_id) {
            states.push((task_id, state));
        }
    }
    Ok(states)
}

struct CompactToolResultRecord<'a> {
    task_id: Option<&'a str>,
    tool_name: &'a str,
    compact_path: &'a str,
    request_summary: String,
    result_summary: String,
    raw_est_tokens: Option<u64>,
    reduced_est_tokens: Option<u64>,
    paths: Vec<String>,
    symbols: Vec<String>,
}

fn record_tool_result(root: &Path, record: CompactToolResultRecord<'_>) -> Result<()> {
    let Some(task_id) = record.task_id.filter(|value| !value.trim().is_empty()) else {
        return Ok(());
    };
    crate::broker_client::write_state(
        root,
        BrokerWriteStateRequest {
            task_id: task_id.to_string(),
            op: Some(BrokerWriteOp::ToolResult),
            tool_name: Some(record.tool_name.to_string()),
            request_summary: Some(record.request_summary),
            result_summary: Some(record.result_summary),
            compact_path: Some(record.compact_path.to_string()),
            raw_est_tokens: record.raw_est_tokens,
            reduced_est_tokens: record.reduced_est_tokens,
            paths: record.paths,
            symbols: record.symbols,
            raw_artifact_available: Some(false),
            refresh_context: Some(false),
            ..BrokerWriteStateRequest::default()
        },
    )?;
    Ok(())
}

fn record_tool_failure(
    root: &Path,
    task_id: Option<&str>,
    tool_name: &str,
    compact_path: &str,
    request_summary: String,
    error_message: String,
) -> Result<()> {
    let Some(task_id) = task_id.filter(|value| !value.trim().is_empty()) else {
        return Ok(());
    };
    crate::broker_client::write_state(
        root,
        BrokerWriteStateRequest {
            task_id: task_id.to_string(),
            op: Some(BrokerWriteOp::ToolInvocationFailed),
            tool_name: Some(tool_name.to_string()),
            request_summary: Some(request_summary),
            compact_path: Some(compact_path.to_string()),
            error_class: Some("command_failed".to_string()),
            error_message: Some(error_message),
            raw_artifact_available: Some(false),
            refresh_context: Some(false),
            ..BrokerWriteStateRequest::default()
        },
    )?;
    Ok(())
}

fn estimate_tokens_for_value(value: &Value) -> u64 {
    let bytes = serde_json::to_vec(value).unwrap_or_default().len() as f64;
    (bytes / 4.0).ceil() as u64
}

fn estimate_tokens_for_strings(values: &[String]) -> u64 {
    estimate_tokens_str(&values.join("\n"))
}

fn estimate_tokens_str(value: &str) -> u64 {
    ((value.len() as f64) / 4.0).ceil() as u64
}

fn pct_saved(raw: u64, reduced: u64) -> f64 {
    if raw == 0 {
        0.0
    } else {
        ((raw.saturating_sub(reduced)) as f64 / raw as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_grep_normalizes_basic_alternation() {
        assert_eq!(
            normalize_grep_query(r"fn classify\|Mutation\|fn classify_command"),
            "fn classify|Mutation|fn classify_command"
        );
    }
}
