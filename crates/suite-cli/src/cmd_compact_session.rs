use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use packet28_daemon_core::storage::load_task_registry;
use serde::Serialize;

use super::{load_task_state, resolve_root, SessionArgs};

#[derive(Debug, Serialize)]
struct SessionItem {
    task_id: String,
    running: bool,
    latest_context_version: Option<String>,
    latest_hook_command_kind: Option<String>,
    latest_hook_handoff_reason: Option<String>,
    recent_invocation_count: usize,
    changed_paths_since_checkpoint: usize,
}

#[derive(Debug, Serialize)]
struct SessionAdoptionReport {
    sessions_scanned: usize,
    total_commands: usize,
    packet28_commands: usize,
    adoption_pct: f64,
    sessions: Vec<SessionAdoptionItem>,
}

#[derive(Debug, Serialize)]
struct SessionAdoptionItem {
    session_id: String,
    date: String,
    command_count: usize,
    packet28_command_count: usize,
    adoption_pct: f64,
    estimated_output_tokens: u64,
}

pub fn run_session(args: SessionArgs) -> Result<i32> {
    if args.sessions_dir.is_some() {
        return run_session_adoption(args);
    }
    let root = resolve_root(&args.root)?;
    let registry = load_task_registry(&root)?;
    let mut sessions = Vec::<SessionItem>::new();
    for (task_id, task) in registry.tasks {
        if args
            .task_id
            .as_deref()
            .is_some_and(|wanted| wanted != task_id.as_str())
        {
            continue;
        }
        let state = load_task_state(&root, &task_id).ok();
        sessions.push(SessionItem {
            task_id: task_id.clone(),
            running: task.lifecycle.is_running(),
            latest_context_version: task.latest_context_version,
            latest_hook_command_kind: task.latest_hook_command_kind,
            latest_hook_handoff_reason: task.latest_hook_handoff_reason,
            recent_invocation_count: state
                .as_ref()
                .map(|state| state.recent_tool_invocations.len())
                .unwrap_or(0),
            changed_paths_since_checkpoint: state
                .as_ref()
                .map(|state| state.changed_paths_since_checkpoint.len())
                .unwrap_or(0),
        });
    }
    sessions.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    sessions.truncate(args.limit.max(1));
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(sessions)?, args.pretty)?;
    } else {
        for session in sessions {
            println!(
                "task={} running={} recent_invocations={} changed_paths={} hook_kind={}",
                session.task_id,
                session.running,
                session.recent_invocation_count,
                session.changed_paths_since_checkpoint,
                session
                    .latest_hook_command_kind
                    .unwrap_or_else(|| "n/a".to_string())
            );
        }
    }
    Ok(0)
}

fn run_session_adoption(args: SessionArgs) -> Result<i32> {
    let Some(sessions_dir) = args.sessions_dir.as_deref().map(PathBuf::from) else {
        anyhow::bail!("session adoption requires --sessions-dir");
    };
    let session_files = crate::cmd_discover::collect_session_files_for_scan(
        &sessions_dir,
        args.limit,
        args.all,
        args.since,
    )?;
    let mut report = SessionAdoptionReport {
        sessions_scanned: 0,
        total_commands: 0,
        packet28_commands: 0,
        adoption_pct: 0.0,
        sessions: Vec::new(),
    };

    for path in session_files {
        if crate::cmd_discover::is_subagent_session_path(&path) {
            continue;
        }
        let Ok(commands) = crate::cmd_discover::extract_bash_commands(&path) else {
            continue;
        };
        if commands.is_empty() {
            continue;
        }
        let session_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();
        let mut command_count = 0usize;
        let mut packet28_command_count = 0usize;
        let mut estimated_output_tokens = 0u64;
        for (command, est_tokens) in commands {
            estimated_output_tokens = estimated_output_tokens.saturating_add(est_tokens);
            for part in split_session_command_chain(&command) {
                command_count += 1;
                if command_is_packet28_covered(&part) {
                    packet28_command_count += 1;
                }
            }
        }
        report.sessions_scanned += 1;
        report.total_commands += command_count;
        report.packet28_commands += packet28_command_count;
        report.sessions.push(SessionAdoptionItem {
            session_id,
            date: session_modified_label(&path),
            command_count,
            packet28_command_count,
            adoption_pct: pct_count(packet28_command_count, command_count),
            estimated_output_tokens,
        });
    }

    report.adoption_pct = pct_count(report.packet28_commands, report.total_commands);
    report.sessions.sort_by(|a, b| {
        b.command_count
            .cmp(&a.command_count)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });

    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else if report.sessions.is_empty() {
        println!("No sessions with Bash commands found.");
    } else {
        print_session_adoption_table(&report, args.limit, args.all);
    }
    Ok(0)
}

fn print_session_adoption_table(report: &SessionAdoptionReport, limit: usize, all: bool) {
    let scope = if all {
        "all".to_string()
    } else {
        format!("last {}", limit.max(1))
    };
    println!("Packet28 Session Overview ({scope})");
    println!("{}", "-".repeat(78));
    println!(
        "{:<12} {:<12} {:>5} {:>8} {:>9} {:<7} {:>8}",
        "Session", "Date", "Cmds", "Packet28", "Adoption", "", "Output"
    );
    println!("{}", "-".repeat(78));
    for session in &report.sessions {
        println!(
            "{:<12} {:<12} {:>5} {:>8} {:>8.0}% {:<7} {:>8}",
            short_session_id(&session.session_id),
            session.date,
            session.command_count,
            session.packet28_command_count,
            session.adoption_pct,
            adoption_bar(session.adoption_pct, 5),
            format_compact_tokens(session.estimated_output_tokens),
        );
    }
    println!("{}", "-".repeat(78));
    println!("Average adoption: {:.0}%", report.adoption_pct);
    println!(
        "Tip: Run `Packet28 discover --sessions-dir <dir>` to find missed Packet28 opportunities"
    );
}

fn short_session_id(session_id: &str) -> String {
    session_id.chars().take(12).collect()
}

fn adoption_bar(pct: f64, width: usize) -> String {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "@".repeat(filled), ".".repeat(empty))
}

fn format_compact_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn session_modified_label(path: &Path) -> String {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);
    let elapsed = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    let days = elapsed.as_secs() / 86_400;
    if days == 0 {
        "Today".to_string()
    } else if days == 1 {
        "Yesterday".to_string()
    } else {
        format!("{days}d ago")
    }
}

fn split_session_command_chain(command: &str) -> Vec<String> {
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
        push_session_command_part(command, start, idx, &mut parts);
        start = idx + delimiter_len;
        if delimiter_len == 2 {
            let _ = chars.next();
        }
    }
    push_session_command_part(command, start, command.len(), &mut parts);
    parts
}

fn push_session_command_part(command: &str, start: usize, end: usize, parts: &mut Vec<String>) {
    let part = command[start..end].trim();
    if !part.is_empty() {
        parts.push(part.to_string());
    }
}

fn command_is_packet28_covered(command: &str) -> bool {
    let first = command.split_whitespace().next().unwrap_or_default();
    if matches!(first, "Packet28" | "packet28" | "packet28-mcp" | "p28") {
        return true;
    }
    !matches!(
        crate::route_registry::decide_command_route(command).kind,
        crate::route_registry::RouteKind::RawPassthrough
    )
}

fn pct_count(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64) * 100.0
    }
}
