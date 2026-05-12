//! Learn module: detect error → correction patterns in session JSONL.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use serde_json::Value;

use crate::memory_store::learn_project_graph;

#[derive(Args, Clone)]
pub struct LearnArgs {
    /// Claude project filter. Defaults to current project unless --all is set.
    #[arg(long)]
    pub project: Option<String>,

    /// Scan all projects under --sessions-dir
    #[arg(long)]
    pub all: bool,

    /// Limit session scan to files modified in the last N days
    #[arg(long, default_value_t = 30)]
    pub since: u64,

    /// Learn a project directory into the local Packet28 concept graph
    #[arg(long)]
    pub project_dir: Option<String>,

    /// Project name to use for graph learning
    #[arg(long)]
    pub project_name: Option<String>,

    /// Memoir/graph container to write learned project concepts into
    #[arg(long)]
    pub memoir: Option<String>,

    /// Maximum dependencies/modules/entrypoints/configs to learn per category
    #[arg(long, default_value_t = 20)]
    pub project_limit: usize,

    /// Path to Claude projects directory
    #[arg(long)]
    pub sessions_dir: Option<String>,

    /// Maximum sessions to scan
    #[arg(long, default_value_t = 20)]
    pub limit: usize,

    /// Minimum frequency to include a correction
    #[arg(long, visible_alias = "min-occurrences", default_value_t = 2)]
    pub min_frequency: usize,

    /// Minimum confidence to include a correction
    #[arg(long, default_value_t = 0.0)]
    pub min_confidence: f64,

    /// Output format: text or json
    #[arg(long, default_value = "text")]
    pub format: String,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,

    /// Write corrections to .claude/rules/cli-corrections.md
    #[arg(long)]
    pub write_rules: bool,
}

#[derive(Debug, Serialize, Default)]
struct LearnReport {
    sessions_scanned: usize,
    corrections_found: usize,
    corrections: Vec<Correction>,
}

#[derive(Debug, Serialize, Clone)]
struct Correction {
    failed_command: String,
    successful_command: String,
    error_type: String,
    base_command: String,
    frequency: usize,
    confidence: f64,
}

#[derive(Debug, Clone)]
struct CommandExecution {
    command: String,
    failed: bool,
    output: String,
}

#[derive(Debug, Clone)]
struct CorrectionAggregate {
    correction: Correction,
}

pub fn run(args: LearnArgs) -> Result<i32> {
    if let Some(project_dir) = args.project_dir.as_deref() {
        let report = learn_project_graph(
            Path::new(project_dir),
            args.project_name.as_deref(),
            args.memoir.as_deref(),
            args.project_limit,
        )?;
        if args.json {
            crate::cmd_common::emit_json(&serde_json::to_value(&report)?, args.pretty)?;
        } else {
            println!(
                "Learned {}: {} concepts, {} links",
                report.project_name, report.total_concepts, report.link_count
            );
        }
        return Ok(0);
    }

    let sessions_dir = args
        .sessions_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_sessions_dir);

    let mut report = LearnReport::default();

    if !sessions_dir.exists() {
        if args.json {
            crate::cmd_common::emit_json(&serde_json::to_value(&report)?, args.pretty)?;
        } else {
            println!("No sessions directory found at {}", sessions_dir.display());
        }
        return Ok(0);
    }

    let session_files = collect_session_files(
        &sessions_dir,
        args.project.as_deref(),
        args.all,
        args.limit,
        args.since,
    )?;
    report.sessions_scanned = session_files.len();

    let mut correction_groups: BTreeMap<(String, String, String), CorrectionAggregate> =
        BTreeMap::new();

    for file in &session_files {
        if let Ok(corrections) = extract_corrections(file) {
            for correction in corrections {
                let diff_token =
                    extract_diff_token(&correction.failed_command, &correction.successful_command);
                let key = (
                    correction.base_command.clone(),
                    correction.error_type.clone(),
                    diff_token,
                );
                correction_groups
                    .entry(key)
                    .and_modify(|existing| {
                        existing.correction.frequency += 1;
                        if correction.confidence > existing.correction.confidence {
                            let frequency = existing.correction.frequency;
                            existing.correction = correction.clone();
                            existing.correction.frequency = frequency;
                        }
                    })
                    .or_insert_with(|| CorrectionAggregate { correction });
            }
        }
    }

    for aggregate in correction_groups.values() {
        let correction = &aggregate.correction;
        if correction.frequency >= args.min_frequency
            && correction.confidence >= args.min_confidence
        {
            report.corrections.push(correction.clone());
        }
    }

    report.corrections.sort_by(|a, b| {
        b.frequency
            .cmp(&a.frequency)
            .then_with(|| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.failed_command.cmp(&b.failed_command))
    });
    report.corrections_found = report.corrections.len();

    if args.write_rules && !report.corrections.is_empty() {
        write_corrections_rules(&report.corrections)?;
    }

    if args.json || args.format == "json" {
        crate::cmd_common::emit_json(&serde_json::to_value(&report)?, args.pretty)?;
    } else {
        println!("Sessions scanned: {}", report.sessions_scanned);
        println!("Corrections found: {}", report.corrections_found);
        for correction in report.corrections.iter().take(20) {
            println!(
                "  {} -> {} ({}x, {}, confidence: {:.0}%)",
                correction.failed_command,
                correction.successful_command,
                correction.frequency,
                correction.error_type,
                correction.confidence * 100.0
            );
        }
        if args.write_rules {
            println!("\nCorrections written to .claude/rules/cli-corrections.md");
        }
    }

    Ok(0)
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

fn collect_session_files(
    dir: &Path,
    project: Option<&str>,
    all: bool,
    limit: usize,
    since_days: u64,
) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !dir.is_dir() {
        return Ok(files);
    }
    for entry in fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.to_string_lossy().contains("subagents") {
            continue;
        }
        if !all {
            if let Some(project) = project {
                if !path.to_string_lossy().contains(project) {
                    continue;
                }
            }
        }
        if path.is_dir() {
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
                        && session_file_is_recent(&sub_path, since_days)
                    {
                        files.push(sub_path);
                    }
                }
            }
        }
        if files.len() >= limit * 5 {
            break;
        }
    }
    files.sort_by(|a, b| {
        let a_time = fs::metadata(a).and_then(|m| m.modified()).ok();
        let b_time = fs::metadata(b).and_then(|m| m.modified()).ok();
        b_time.cmp(&a_time)
    });
    files.truncate(limit);
    Ok(files)
}

fn session_file_is_recent(path: &Path, since_days: u64) -> bool {
    if since_days == 0 {
        return true;
    }
    let Ok(modified) = fs::metadata(path).and_then(|metadata| metadata.modified()) else {
        return true;
    };
    let elapsed = std::time::SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    elapsed.as_secs() <= since_days.saturating_mul(86_400)
}

fn extract_corrections(path: &Path) -> Result<Vec<Correction>> {
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;

    let mut executions = Vec::<CommandExecution>::new();
    let mut pending_command = None::<String>;

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        // Look for tool results from Bash commands
        if value.get("type").and_then(Value::as_str) == Some("user") {
            if let Some(content) = value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_array)
            {
                for block in content {
                    if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                        let is_error = block
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let result_content =
                            block.get("content").and_then(Value::as_str).unwrap_or("");
                        if let Some(cmd) = pending_command.take() {
                            executions.push(CommandExecution {
                                command: cmd,
                                failed: is_error || output_looks_like_error(result_content),
                                output: result_content.to_string(),
                            });
                        }
                    }
                }
            }
        }

        // Also look for assistant tool_use with Bash
        if value.get("type").and_then(Value::as_str) == Some("assistant") {
            if let Some(content) = value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_array)
            {
                for block in content {
                    if block.get("type").and_then(Value::as_str) == Some("tool_use")
                        && block.get("name").and_then(Value::as_str) == Some("Bash")
                    {
                        if let Some(cmd) = block
                            .get("input")
                            .and_then(|i| i.get("command"))
                            .and_then(Value::as_str)
                        {
                            pending_command = Some(cmd.to_string());
                        }
                    }
                }
            }
        }
    }

    // Find fail -> success correction patterns using RTK-style lookahead.
    let mut corrections = Vec::new();
    const CORRECTION_WINDOW: usize = 3;
    const MIN_PAIR_CONFIDENCE: f64 = 0.6;
    for idx in 0..executions.len() {
        let failed = &executions[idx];
        if !is_command_error(failed.failed, &failed.output) {
            continue;
        }
        let error_type = classify_error(&failed.output);
        if is_tdd_cycle_error(&error_type, &failed.output) {
            continue;
        }
        for candidate in executions.iter().skip(idx + 1).take(CORRECTION_WINDOW) {
            let similarity = command_similarity(&failed.command, &candidate.command);
            if similarity < 0.5 {
                continue;
            }
            if failed.command == candidate.command
                || differs_only_by_path(&failed.command, &candidate.command)
            {
                continue;
            }
            let mut confidence = similarity;
            if !is_command_error(candidate.failed, &candidate.output) {
                confidence = (confidence + 0.2).min(1.0);
            }
            if confidence < MIN_PAIR_CONFIDENCE {
                continue;
            }
            corrections.push(Correction {
                failed_command: truncate_command(&failed.command, 80),
                successful_command: truncate_command(&candidate.command, 80),
                error_type,
                base_command: extract_base_command(&failed.command),
                frequency: 1,
                confidence,
            });
            break;
        }
    }

    Ok(corrections)
}

fn output_looks_like_error(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("failed")
        || lower.contains("invalid")
        || lower.contains("unknown option")
        || lower.contains("unknown flag")
        || lower.contains("command not found")
        || lower.contains("cannot")
        || lower.contains("no such file")
        || lower.contains("missing")
        || lower.contains("permission denied")
}

fn is_command_error(failed: bool, output: &str) -> bool {
    if !failed || output_looks_like_user_rejection(output) {
        return false;
    }
    output_looks_like_error(output)
}

fn output_looks_like_user_rejection(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("user declined")
        || lower.contains("user rejected")
        || lower.contains("user cancelled")
        || lower.contains("user canceled")
        || lower.contains("operation cancelled by user")
        || lower.contains("operation canceled by user")
        || lower.contains("doesn't want")
}

fn classify_error(output: &str) -> String {
    let lower = output.to_ascii_lowercase();
    if lower.contains("unknown option") || lower.contains("unknown flag") {
        "unknown_flag".to_string()
    } else if lower.contains("command not found") {
        "command_not_found".to_string()
    } else if lower.contains("no such file") || lower.contains("not found") {
        "wrong_path".to_string()
    } else if lower.contains("missing") || lower.contains("required") {
        "missing_arg".to_string()
    } else if lower.contains("permission denied") {
        "permission_denied".to_string()
    } else {
        "other".to_string()
    }
}

fn is_tdd_cycle_error(error_type: &str, output: &str) -> bool {
    if output.contains("error[E") || output.contains("aborting due to") {
        return true;
    }
    if output.contains("test result: FAILED") || output.contains("tests failed") {
        return true;
    }
    matches!(error_type, "command_not_found" | "other")
        && (output.contains("error[E") || output.contains("FAILED"))
}

fn extract_base_command(cmd: &str) -> String {
    let stripped = cmd
        .trim()
        .strip_prefix("RUST_BACKTRACE=1 ")
        .or_else(|| cmd.trim().strip_prefix("NODE_ENV=production "))
        .or_else(|| cmd.trim().strip_prefix("DEBUG=* "))
        .unwrap_or_else(|| cmd.trim());
    let parts = stripped.split_whitespace().collect::<Vec<_>>();
    match parts.len() {
        0 => String::new(),
        1 => parts[0].to_string(),
        _ => format!("{} {}", parts[0], parts[1]),
    }
}

fn command_similarity(left: &str, right: &str) -> f64 {
    let left_base = extract_base_command(left);
    let right_base = extract_base_command(right);
    if left_base != right_base {
        return 0.0;
    }
    let left_args = command_args_after_base(left, &left_base);
    let right_args = command_args_after_base(right, &right_base);
    if left_args.is_empty() && right_args.is_empty() {
        return 1.0;
    }
    let intersection = left_args.intersection(&right_args).count();
    let union = left_args.union(&right_args).count();
    if union == 0 {
        0.5
    } else {
        0.5 + (intersection as f64 / union as f64) * 0.5
    }
}

fn command_args_after_base<'a>(cmd: &'a str, base: &str) -> BTreeSet<&'a str> {
    cmd.trim()
        .strip_prefix(base)
        .unwrap_or("")
        .split_whitespace()
        .collect()
}

fn differs_only_by_path(left: &str, right: &str) -> bool {
    let left_base = extract_base_command(left);
    let right_base = extract_base_command(right);
    left_base == right_base && {
        let similarity = command_similarity(left, right);
        similarity > 0.9 && similarity < 1.0
    }
}

fn extract_diff_token(wrong: &str, right: &str) -> String {
    let wrong_parts = wrong.split_whitespace().collect::<BTreeSet<_>>();
    let right_parts = right.split_whitespace().collect::<BTreeSet<_>>();
    let removed = wrong_parts
        .difference(&right_parts)
        .next()
        .copied()
        .unwrap_or_default();
    let added = right_parts
        .difference(&wrong_parts)
        .next()
        .copied()
        .unwrap_or_default();
    match (removed.is_empty(), added.is_empty()) {
        (false, false) => format!("{removed} -> {added}"),
        (false, true) => format!("removed {removed}"),
        (true, false) => format!("added {added}"),
        (true, true) => "unknown".to_string(),
    }
}

fn truncate_command(cmd: &str, max: usize) -> String {
    if cmd.len() <= max {
        cmd.to_string()
    } else {
        format!("{}...", &cmd[..max.saturating_sub(3)])
    }
}

fn write_corrections_rules(corrections: &[Correction]) -> Result<()> {
    let dir = PathBuf::from(".claude/rules");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let path = dir.join("cli-corrections.md");
    let mut content = String::from("# CLI Corrections (auto-generated by `Packet28 learn`)\n\n");
    content.push_str("These patterns were learned from session history.\n\n");

    let mut grouped: BTreeMap<&str, Vec<&Correction>> = BTreeMap::new();
    for correction in corrections.iter().take(50) {
        grouped
            .entry(correction.base_command.as_str())
            .or_default()
            .push(correction);
    }

    for (base_command, corrections) in grouped {
        content.push_str(&format!("## {}\n", capitalize_first(base_command)));
        for correction in corrections {
            let seen = if correction.frequency > 1 {
                format!(" (seen {}x)", correction.frequency)
            } else {
                String::new()
            };
            content.push_str(&format!(
                "- Use `{}` not `{}`{}\n",
                correction.successful_command, correction.failed_command, seen
            ));
        }
        content.push('\n');
    }

    fs::write(&path, &content).with_context(|| format!("failed to write {}", path.display()))?;

    Ok(())
}

fn capitalize_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_command_uses_first_two_tokens_and_strips_common_env() {
        assert_eq!(extract_base_command("git commit --amend"), "git commit");
        assert_eq!(
            extract_base_command("RUST_BACKTRACE=1 cargo test --all"),
            "cargo test"
        );
    }

    #[test]
    fn similarity_matches_rtk_same_base_scoring() {
        assert_eq!(command_similarity("git status", "git status"), 1.0);
        assert_eq!(
            command_similarity("git status --porcelain=v9", "git status --short"),
            0.5
        );
        assert_eq!(command_similarity("git status", "cargo test"), 0.0);
    }

    #[test]
    fn command_error_filters_user_rejections_and_tdd_noise() {
        assert!(!is_command_error(true, "Operation cancelled by user"));
        assert!(is_command_error(true, "error: unknown flag --foo"));
        assert!(is_tdd_cycle_error(
            "other",
            "error[E0308]: mismatched types"
        ));
    }
}
