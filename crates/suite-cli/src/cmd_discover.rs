//! Discover module: scan Claude session JSONL for command patterns and savings opportunities.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use serde_json::Value;

use crate::savings_analytics::load_run_savings;

#[derive(Args, Clone)]
pub struct DiscoverArgs {
    /// Workspace root for local Packet28 run analytics
    #[arg(long, default_value = ".")]
    pub root: String,

    /// Filter Claude-style sessions by project path substring
    #[arg(short, long)]
    pub project: Option<String>,

    /// Path to Claude projects directory
    #[arg(long)]
    pub sessions_dir: Option<String>,

    /// Maximum sessions to scan
    #[arg(long, default_value_t = 20)]
    pub limit: usize,

    /// Scan every matching session instead of truncating to --limit
    #[arg(long)]
    pub all: bool,

    /// Limit sessions to files modified in the last N days
    #[arg(long)]
    pub since: Option<u64>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Serialize, Default)]
struct DiscoverReport {
    sessions_scanned: usize,
    commands_found: usize,
    supported_commands: usize,
    unsupported_commands: usize,
    by_category: BTreeMap<String, CategoryStats>,
    missed_opportunities: Vec<MissedOpportunity>,
    top_unsupported: Vec<UnsupportedCommand>,
    disabled_bypass_count: usize,
    disabled_bypass_examples: Vec<String>,
    missed_savings: Vec<MissedSavingsCommand>,
}

#[derive(Debug, Serialize, Default)]
struct CategoryStats {
    count: usize,
    estimated_tokens: u64,
}

#[derive(Debug, Serialize)]
struct UnsupportedCommand {
    command: String,
    count: usize,
    estimated_tokens: u64,
}

#[derive(Debug, Serialize)]
struct MissedOpportunity {
    command: String,
    count: usize,
    packet28_equivalent: String,
    category: String,
    raw_est_tokens: u64,
    estimated_savings_tokens: u64,
    estimated_savings_percent: f64,
}

#[derive(Debug, Serialize)]
struct MissedSavingsCommand {
    command: String,
    reason: String,
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
    savings_percent: f64,
}

pub fn run(args: DiscoverArgs) -> Result<i32> {
    let root = crate::cmd_daemon::resolve_root_arg(&args.root);
    let sessions_dir = args
        .sessions_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_sessions_dir);

    let mut report = DiscoverReport::default();
    add_run_savings_misses(&root, args.limit, &mut report)?;

    if !sessions_dir.exists() {
        if args.json {
            crate::cmd_common::emit_json(&serde_json::to_value(&report)?, args.pretty)?;
        } else {
            println!("No sessions directory found at {}", sessions_dir.display());
        }
        return Ok(0);
    }

    let session_files = collect_session_files_for_scan_with_project(
        &sessions_dir,
        args.project.as_deref(),
        args.limit,
        args.all,
        args.since,
    )?;
    report.sessions_scanned = session_files.len();

    let mut unsupported_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut unsupported_tokens: BTreeMap<String, u64> = BTreeMap::new();
    let mut opportunity_buckets: BTreeMap<String, MissedOpportunityBucket> = BTreeMap::new();
    let mut disabled_bypass_examples: BTreeMap<String, usize> = BTreeMap::new();

    for file in &session_files {
        if let Ok(commands) = extract_bash_commands(file) {
            for (cmd, est_tokens) in commands {
                let parts = split_command_chain(&cmd);
                let part_count = parts.len().max(1) as u64;
                let shared_tokens = est_tokens.saturating_div(part_count).max(1);
                for part in parts {
                    report.commands_found += 1;
                    let program = program_name(&part);
                    let part_tokens = shared_tokens.max(estimate_text_tokens(&part));
                    if let Some(actual_command) = strip_active_disabled_prefix(&part) {
                        let route = crate::route_registry::decide_command_route(&actual_command);
                        if command_part_supported_with_route(&actual_command, &route) {
                            report.disabled_bypass_count += 1;
                            let example = truncate_command_example(&actual_command);
                            *disabled_bypass_examples.entry(example).or_insert(0) += 1;
                        }
                        continue;
                    }
                    let route = crate::route_registry::decide_command_route(&part);
                    if command_part_supported_with_route(&part, &route) {
                        report.supported_commands += 1;
                        let category = categorize_command(&program);
                        let entry = report.by_category.entry(category.clone()).or_default();
                        entry.count += 1;
                        entry.estimated_tokens += part_tokens;
                        if !is_packet28_command(&part) {
                            let equivalent = packet28_equivalent(&part, &route);
                            let key = format!("{category}\0{equivalent}");
                            let bucket = opportunity_buckets.entry(key).or_insert_with(|| {
                                MissedOpportunityBucket {
                                    example: truncate_command_example(&part),
                                    equivalent,
                                    category,
                                    count: 0,
                                    raw_est_tokens: 0,
                                }
                            });
                            bucket.count += 1;
                            bucket.raw_est_tokens =
                                bucket.raw_est_tokens.saturating_add(part_tokens);
                        }
                    } else {
                        report.unsupported_commands += 1;
                        *unsupported_counts.entry(program.clone()).or_insert(0) += 1;
                        *unsupported_tokens.entry(program).or_insert(0) += part_tokens;
                    }
                }
            }
        }
    }

    for (program, count) in &unsupported_counts {
        let tokens = unsupported_tokens.get(program).copied().unwrap_or(0);
        report.top_unsupported.push(UnsupportedCommand {
            command: program.clone(),
            count: *count,
            estimated_tokens: tokens,
        });
    }

    report.missed_opportunities = opportunity_buckets
        .into_values()
        .map(|bucket| {
            let savings = estimate_discover_savings_tokens(bucket.raw_est_tokens);
            MissedOpportunity {
                command: bucket.example,
                count: bucket.count,
                packet28_equivalent: bucket.equivalent,
                category: bucket.category,
                raw_est_tokens: bucket.raw_est_tokens,
                estimated_savings_tokens: savings,
                estimated_savings_percent: discover_savings_percent(),
            }
        })
        .collect();
    report.missed_opportunities.sort_by(|a, b| {
        b.estimated_savings_tokens
            .cmp(&a.estimated_savings_tokens)
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| a.command.cmp(&b.command))
    });
    report.missed_opportunities.truncate(20);

    let mut disabled_examples = disabled_bypass_examples.into_iter().collect::<Vec<_>>();
    disabled_examples.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    report.disabled_bypass_examples = disabled_examples
        .into_iter()
        .take(5)
        .map(|(command, count)| format!("{command} ({count}x)"))
        .collect();

    report
        .top_unsupported
        .sort_by(|a, b| b.estimated_tokens.cmp(&a.estimated_tokens));
    report.top_unsupported.truncate(20);

    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(&report)?, args.pretty)?;
    } else {
        println!("Sessions scanned: {}", report.sessions_scanned);
        println!("Commands found: {}", report.commands_found);
        println!("Supported: {}", report.supported_commands);
        println!("Unsupported: {}", report.unsupported_commands);
        if !report.by_category.is_empty() {
            println!("\nBy category:");
            for (category, stats) in &report.by_category {
                println!(
                    "  {category}: {} commands, ~{} tokens",
                    stats.count,
                    crate::economics::format_tokens(stats.estimated_tokens)
                );
            }
        }
        if !report.missed_opportunities.is_empty() {
            println!("\nMissed Packet28 opportunities:");
            for item in report.missed_opportunities.iter().take(10) {
                println!(
                    "  {}: {}x -> {} (~{} saveable)",
                    item.command,
                    item.count,
                    item.packet28_equivalent,
                    crate::economics::format_tokens(item.estimated_savings_tokens)
                );
            }
        }
        if report.disabled_bypass_count > 0 {
            println!(
                "\nDisabled bypasses: {} commands ran without Packet28 reduction",
                report.disabled_bypass_count
            );
            if !report.disabled_bypass_examples.is_empty() {
                println!("  {}", report.disabled_bypass_examples.join(", "));
            }
            println!("  Remove PACKET28_DISABLED/RTK_DISABLED to recover savings");
        }
        if !report.top_unsupported.is_empty() {
            println!("\nTop unsupported commands:");
            for cmd in report.top_unsupported.iter().take(10) {
                println!(
                    "  {}: {}x (~{} tokens)",
                    cmd.command,
                    cmd.count,
                    crate::economics::format_tokens(cmd.estimated_tokens)
                );
            }
        }
        if !report.missed_savings.is_empty() {
            println!("Missed local savings:");
            for item in &report.missed_savings {
                println!(
                    "  {} reason={} raw={} reduced={} savings={:.1}%",
                    item.command,
                    item.reason,
                    item.raw_est_tokens,
                    item.reduced_est_tokens,
                    item.savings_percent
                );
            }
        }
    }

    Ok(0)
}

struct MissedOpportunityBucket {
    example: String,
    equivalent: String,
    category: String,
    count: usize,
    raw_est_tokens: u64,
}

fn add_run_savings_misses(root: &Path, limit: usize, report: &mut DiscoverReport) -> Result<()> {
    for record in load_run_savings(root, limit)? {
        let missed = record.fallback_reason.is_some()
            || (record.raw_est_tokens > 0 && record.savings_percent < 10.0);
        if missed {
            report.missed_savings.push(MissedSavingsCommand {
                command: record.command,
                reason: record
                    .fallback_reason
                    .unwrap_or_else(|| "low_savings".to_string()),
                raw_est_tokens: record.raw_est_tokens,
                reduced_est_tokens: record.reduced_est_tokens,
                savings_percent: record.savings_percent,
            });
        }
    }
    Ok(())
}

fn default_sessions_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(format!("{home}/.claude/projects"));
        }
    }
    PathBuf::from("/tmp/.claude/projects")
}

pub(crate) fn collect_session_files_for_scan(
    dir: &Path,
    limit: usize,
    all: bool,
    since_days: Option<u64>,
) -> Result<Vec<PathBuf>> {
    collect_session_files_for_scan_with_project(dir, None, limit, all, since_days)
}

pub(crate) fn collect_session_files_for_scan_with_project(
    dir: &Path,
    project: Option<&str>,
    limit: usize,
    all: bool,
    since_days: Option<u64>,
) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !dir.is_dir() {
        return Ok(files);
    }
    let max_scan_files = if all {
        None
    } else {
        Some(limit.saturating_mul(5).max(limit))
    };
    let cutoff = since_days.map(|days| {
        SystemTime::now()
            .checked_sub(Duration::from_secs(days.saturating_mul(86_400)))
            .unwrap_or(SystemTime::UNIX_EPOCH)
    });
    // Walk project directories looking for session JSONL files
    for entry in fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if let Some(project) = project {
            if !path.to_string_lossy().contains(project) {
                continue;
            }
        }
        if path.is_dir() {
            collect_session_jsonl_files(&path, cutoff, max_scan_files, &mut files)?;
        } else if path.extension().is_some_and(|ext| ext == "jsonl")
            && session_file_in_since_window(&path, cutoff)
        {
            files.push(path);
        }
        if max_scan_files.is_some_and(|max| files.len() >= max) {
            break;
        }
    }
    // Sort by modification time, newest first
    files.sort_by(|a, b| {
        let a_time = fs::metadata(a).and_then(|m| m.modified()).ok();
        let b_time = fs::metadata(b).and_then(|m| m.modified()).ok();
        b_time.cmp(&a_time)
    });
    if !all {
        files.truncate(limit);
    }
    Ok(files)
}

fn collect_session_jsonl_files(
    dir: &Path,
    cutoff: Option<SystemTime>,
    max_files: Option<usize>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if max_files.is_some_and(|max| files.len() >= max) {
        return Ok(());
    }
    let entries = fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_session_jsonl_files(&path, cutoff, max_files, files)?;
        } else if path.extension().is_some_and(|ext| ext == "jsonl")
            && session_file_in_since_window(&path, cutoff)
        {
            files.push(path);
        }
        if max_files.is_some_and(|max| files.len() >= max) {
            break;
        }
    }
    Ok(())
}

fn session_file_in_since_window(path: &Path, cutoff: Option<SystemTime>) -> bool {
    let Some(cutoff) = cutoff else {
        return true;
    };
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(|modified| modified >= cutoff)
        .unwrap_or(false)
}

pub(crate) fn is_subagent_session_path(path: &Path) -> bool {
    path.to_string_lossy().contains("subagents")
}

pub(crate) fn extract_bash_commands(path: &Path) -> Result<Vec<(String, u64)>> {
    let mut commands = Vec::<(String, u64)>::new();
    let mut pending_by_id = BTreeMap::<String, usize>::new();
    let mut pending_without_id = Vec::<usize>::new();
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        let Some(content) = value.get("message").and_then(|m| m.get("content")) else {
            continue;
        };
        let Some(blocks) = content.as_array() else {
            continue;
        };

        if value.get("type").and_then(Value::as_str) == Some("assistant") {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                let tool_name = block.get("name").and_then(Value::as_str).unwrap_or("");
                if tool_name != "Bash" {
                    continue;
                }
                if let Some(command) = block
                    .get("input")
                    .and_then(|input| input.get("command"))
                    .and_then(Value::as_str)
                {
                    let index = commands.len();
                    let est_tokens = estimate_text_tokens(command);
                    commands.push((command.to_string(), est_tokens));
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        pending_by_id.insert(id.to_string(), index);
                    } else {
                        pending_without_id.push(index);
                    }
                }
            }
        } else if value.get("type").and_then(Value::as_str) == Some("user") {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                    continue;
                }
                let result_tokens =
                    estimate_value_tokens(block.get("content").unwrap_or(&Value::Null));
                let matched_index = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .and_then(|id| pending_by_id.remove(id))
                    .or_else(|| {
                        if pending_without_id.is_empty() {
                            None
                        } else {
                            Some(pending_without_id.remove(0))
                        }
                    });
                if let Some(index) = matched_index {
                    if let Some((_, est_tokens)) = commands.get_mut(index) {
                        *est_tokens = result_tokens.max(*est_tokens);
                    }
                }
            }
        }
    }

    Ok(commands)
}

fn estimate_value_tokens(value: &Value) -> u64 {
    match value {
        Value::String(text) => estimate_text_tokens(text),
        Value::Array(items) => items.iter().map(estimate_value_tokens).sum::<u64>().max(1),
        Value::Object(_) => estimate_text_tokens(&value.to_string()),
        Value::Null | Value::Bool(_) | Value::Number(_) => estimate_text_tokens(&value.to_string()),
    }
}

fn estimate_text_tokens(text: &str) -> u64 {
    ((text.len() as u64) / 4).max(1)
}

fn is_known_reducible(program: &str) -> bool {
    matches!(
        program,
        "git"
            | "cargo"
            | "gh"
            | "go"
            | "golangci-lint"
            | "docker"
            | "kubectl"
            | "curl"
            | "python"
            | "python3"
            | "pytest"
            | "ruff"
            | "mypy"
            | "pip"
            | "pip3"
            | "uv"
            | "npm"
            | "pnpm"
            | "yarn"
            | "npx"
            | "tsc"
            | "eslint"
            | "vitest"
            | "prettier"
            | "next"
            | "prisma"
            | "playwright"
            | "ls"
            | "find"
            | "cat"
            | "head"
            | "tail"
            | "sed"
            | "diff"
            | "aws"
    )
}

fn command_part_supported_with_route(
    command: &str,
    route: &crate::route_registry::RouteDecision,
) -> bool {
    let program = program_name(command);
    matches!(
        program.as_str(),
        "Packet28" | "packet28" | "packet28-mcp" | "p28"
    ) || !matches!(route.kind, crate::route_registry::RouteKind::RawPassthrough)
        || is_known_reducible(&program)
}

fn is_packet28_command(command: &str) -> bool {
    matches!(
        program_name(command).as_str(),
        "Packet28" | "packet28" | "packet28-mcp" | "p28"
    )
}

pub(crate) fn strip_active_disabled_prefix(command: &str) -> Option<String> {
    let argv = shell_words::split(command).ok()?;
    let mut index = usize::from(argv.first().is_some_and(|arg| arg == "env"));
    let mut disabled = false;
    while let Some(arg) = argv.get(index) {
        let Some((key, value)) = split_env_assignment(arg) else {
            break;
        };
        if matches!(key, "PACKET28_DISABLED" | "RTK_DISABLED")
            && !matches!(value.trim(), "" | "0" | "false" | "FALSE" | "False")
        {
            disabled = true;
        }
        index += 1;
    }
    if disabled && index < argv.len() {
        Some(argv[index..].join(" "))
    } else {
        None
    }
}

fn split_env_assignment(arg: &str) -> Option<(&str, &str)> {
    if arg.starts_with('-') {
        return None;
    }
    let (key, value) = arg.split_once('=')?;
    if key.is_empty()
        || !key
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        || key.chars().next().is_some_and(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    Some((key, value))
}

fn packet28_equivalent(command: &str, route: &crate::route_registry::RouteDecision) -> String {
    match route.kind {
        crate::route_registry::RouteKind::NativeTool => {
            let name = route
                .native_tool
                .as_ref()
                .map(|tool| match tool.kind {
                    crate::route_registry::NativeToolKind::Tree => "tree",
                    crate::route_registry::NativeToolKind::Read => "read",
                    crate::route_registry::NativeToolKind::Grep => "grep",
                    crate::route_registry::NativeToolKind::Env => "env",
                })
                .unwrap_or("run");
            format!("Packet28 {name}")
        }
        crate::route_registry::RouteKind::ReducerRewrite
        | crate::route_registry::RouteKind::TomlFilterRewrite
        | crate::route_registry::RouteKind::ProxyPassthrough
        | crate::route_registry::RouteKind::CompoundRewrite => "Packet28 run".to_string(),
        crate::route_registry::RouteKind::RawPassthrough => {
            format!("Packet28 {}", program_name(command))
        }
    }
}

fn discover_savings_percent() -> f64 {
    70.0
}

fn estimate_discover_savings_tokens(raw_tokens: u64) -> u64 {
    ((raw_tokens as f64) * discover_savings_percent() / 100.0).round() as u64
}

fn truncate_command_example(command: &str) -> String {
    let mut parts = command.split_whitespace();
    let Some(first) = parts.next() else {
        return String::new();
    };
    let Some(second) = parts.next() else {
        return first.to_string();
    };
    format!("{first} {second}")
}

fn program_name(command: &str) -> String {
    command.split_whitespace().next().unwrap_or("").to_string()
}

fn split_command_chain(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut chars = command.char_indices().peekable();
    let mut quote = None::<char>;
    while let Some((idx, ch)) = chars.next() {
        if matches!(ch, '\'' | '"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            }
            continue;
        }
        if quote.is_some() {
            continue;
        }
        let next = chars.peek().map(|(_, ch)| *ch);
        let delimiter_len = match (ch, next) {
            ('&', Some('&')) | ('|', Some('|')) => 2,
            (';', _) => 1,
            _ => 0,
        };
        if delimiter_len == 0 {
            continue;
        }
        push_command_part(command, start, idx, &mut parts);
        start = idx + delimiter_len;
        if delimiter_len == 2 {
            let _ = chars.next();
        }
    }
    push_command_part(command, start, command.len(), &mut parts);
    parts
}

fn push_command_part(command: &str, start: usize, end: usize, parts: &mut Vec<String>) {
    let part = command[start..end].trim();
    if !part.is_empty() {
        parts.push(part.to_string());
    }
}

fn categorize_command(program: &str) -> String {
    match program {
        "git" => "git",
        "cargo" => "rust",
        "gh" => "github",
        "go" | "golangci-lint" => "go",
        "docker" | "kubectl" | "curl" | "aws" => "infra",
        "python" | "python3" | "pytest" | "ruff" | "mypy" | "pip" | "pip3" | "uv" => "python",
        "npm" | "pnpm" | "yarn" | "npx" | "tsc" | "eslint" | "vitest" | "prettier" | "next"
        | "prisma" | "playwright" => "javascript",
        "gradle" | "gradlew" | "./gradlew" | "gradlew.bat" => "jvm",
        "bundle" | "rspec" | "rubocop" | "ruby" | "rails" | "rake" => "ruby",
        "dotnet" => "dotnet",
        "ls" | "find" | "cat" | "head" | "tail" | "sed" | "diff" => "fs",
        _ => "other",
    }
    .to_string()
}
