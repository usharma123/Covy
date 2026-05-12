//! Learn module: detect error → correction patterns in session JSONL.

use std::collections::BTreeMap;
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
    #[arg(long, default_value_t = 2)]
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
    frequency: usize,
    confidence: f64,
}

#[derive(Debug, Clone)]
struct CommandExecution {
    command: String,
    failed: bool,
    output: String,
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

    // Collect all corrections across sessions
    let mut correction_counts: BTreeMap<(String, String, String), usize> = BTreeMap::new();

    for file in &session_files {
        if let Ok(corrections) = extract_corrections(file) {
            for correction in corrections {
                *correction_counts
                    .entry((
                        correction.failed_command,
                        correction.successful_command,
                        correction.error_type,
                    ))
                    .or_insert(0) += 1;
            }
        }
    }

    // Filter by minimum frequency and compute confidence
    let total_corrections: usize = correction_counts.values().sum();
    for ((failed, success, error_type), count) in &correction_counts {
        let confidence = (*count as f64) / (total_corrections.max(1) as f64);
        if *count >= args.min_frequency && confidence >= args.min_confidence {
            report.corrections.push(Correction {
                failed_command: failed.clone(),
                successful_command: success.clone(),
                error_type: error_type.clone(),
                frequency: *count,
                confidence,
            });
        }
    }

    report
        .corrections
        .sort_by(|a, b| b.frequency.cmp(&a.frequency));
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

    // Find fail → success correction patterns
    let mut corrections = Vec::new();
    for window in executions.windows(2) {
        let first = &window[0];
        let second = &window[1];
        if first.failed && !second.failed {
            // Extract the base command to check if they're related
            let base1 = first.command.split_whitespace().next().unwrap_or("");
            let base2 = second.command.split_whitespace().next().unwrap_or("");
            if base1 == base2 {
                let short1 = truncate_command(&first.command, 80);
                let short2 = truncate_command(&second.command, 80);
                corrections.push(Correction {
                    failed_command: short1,
                    successful_command: short2,
                    error_type: classify_error(&first.output),
                    frequency: 1,
                    confidence: 1.0,
                });
            }
        }
    }

    Ok(corrections)
}

fn output_looks_like_error(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("unknown option")
        || lower.contains("unknown flag")
        || lower.contains("command not found")
        || lower.contains("no such file")
        || lower.contains("missing")
        || lower.contains("permission denied")
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

    for correction in corrections.iter().take(50) {
        content.push_str(&format!(
            "- Instead of `{}`, prefer `{}` (seen {}x)\n",
            correction.failed_command, correction.successful_command, correction.frequency
        ));
    }

    fs::write(&path, &content).with_context(|| format!("failed to write {}", path.display()))?;

    Ok(())
}
