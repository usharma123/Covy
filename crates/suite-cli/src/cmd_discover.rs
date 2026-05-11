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
    top_unsupported: Vec<UnsupportedCommand>,
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

    let session_files =
        collect_session_files_for_scan(&sessions_dir, args.limit, args.all, args.since)?;
    report.sessions_scanned = session_files.len();

    let mut unsupported_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut unsupported_tokens: BTreeMap<String, u64> = BTreeMap::new();

    for file in &session_files {
        if let Ok(commands) = extract_bash_commands(file) {
            for (cmd, est_tokens) in commands {
                for part in split_command_chain(&cmd) {
                    report.commands_found += 1;
                    let program = program_name(&part);
                    let part_tokens = ((part.len() as u64) / 4).max(1).min(est_tokens.max(1));
                    if command_part_supported(&part) {
                        report.supported_commands += 1;
                        let category = categorize_command(&program);
                        let entry = report.by_category.entry(category).or_default();
                        entry.count += 1;
                        entry.estimated_tokens += part_tokens;
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
    let mut files = Vec::new();
    if !dir.is_dir() {
        return Ok(files);
    }
    let cutoff = since_days.map(|days| {
        SystemTime::now()
            .checked_sub(Duration::from_secs(days.saturating_mul(86_400)))
            .unwrap_or(SystemTime::UNIX_EPOCH)
    });
    // Walk project directories looking for session JSONL files
    for entry in fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            // Look for sessions subdirectory or JSONL files directly
            let sessions_subdir = path.join("sessions");
            let scan_dir = if sessions_subdir.is_dir() {
                sessions_subdir
            } else {
                path
            };
            if let Ok(entries) = fs::read_dir(&scan_dir) {
                for sub_entry in entries.flatten() {
                    let sub_path = sub_entry.path();
                    if sub_path.extension().is_some_and(|ext| ext == "jsonl")
                        && session_file_in_since_window(&sub_path, cutoff)
                    {
                        files.push(sub_path);
                    }
                }
            }
        } else if path.extension().is_some_and(|ext| ext == "jsonl")
            && session_file_in_since_window(&path, cutoff)
        {
            files.push(path);
        }
        if !all && files.len() >= limit * 5 {
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

fn session_file_in_since_window(path: &Path, cutoff: Option<SystemTime>) -> bool {
    let Some(cutoff) = cutoff else {
        return true;
    };
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(|modified| modified >= cutoff)
        .unwrap_or(false)
}

pub(crate) fn extract_bash_commands(path: &Path) -> Result<Vec<(String, u64)>> {
    let mut commands = Vec::new();
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        // Look for assistant messages with tool_use content
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = value.get("message").and_then(|m| m.get("content")) else {
            continue;
        };
        let Some(blocks) = content.as_array() else {
            continue;
        };

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
                let est_tokens = (command.len() as u64) / 4;
                commands.push((command.to_string(), est_tokens));
            }
        }
    }

    Ok(commands)
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

fn command_part_supported(command: &str) -> bool {
    let program = program_name(command);
    matches!(
        program.as_str(),
        "Packet28" | "packet28" | "packet28-mcp" | "p28"
    ) || !matches!(
        crate::route_registry::decide_command_route(command).kind,
        crate::route_registry::RouteKind::RawPassthrough
    ) || is_known_reducible(&program)
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
        "bundle" | "rspec" | "rubocop" | "ruby" | "rails" => "ruby",
        "dotnet" => "dotnet",
        "ls" | "find" | "cat" | "head" | "tail" | "sed" | "diff" => "fs",
        _ => "other",
    }
    .to_string()
}
