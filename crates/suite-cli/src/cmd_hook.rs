use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Args, Subcommand};
use packet28_daemon_core::{
    hook_runtime_config_path, now_unix, ActiveTaskRecord, BrokerAction, BrokerGetContextRequest,
    HookBoundaryKind, HookEventKind, HookIngestRequest, HookRuntimeConfig,
};
#[cfg(test)]
use packet28_reducer_core::classify_command;
use serde_json::{json, Value};

use crate::cmd_hook_http::{ensure_hook_http_server, run_hook_http_server};
use crate::cmd_hook_packets::build_reducer_packet;
#[cfg(test)]
use crate::cmd_hook_packets::{build_bash_packet, build_grep_packet, build_read_packet};
use crate::cmd_hook_runner::{run_reduce_fixture, run_reducer_runner};
#[path = "cmd_hook_runtime.rs"]
mod hook_runtime;
pub(crate) use crate::cmd_hook_support::{
    compact_text, estimate_text_tokens, now_unix_millis, payload_text_len, reduction_pct,
    shell_join,
};
use crate::cmd_hook_support::{hook_output_text, json_nested_string, json_string};
use crate::cmd_wakeup::build_wakeup_pack_for_injection;
use crate::memory_store::{
    append_transcript_message, enqueue_pending_extraction, hook_event_stats, list_hook_events,
    record_hook_event, HookEventInput, PendingExtractionInput, TranscriptAppendInput,
};
use hook_runtime::{
    build_runtime_pretool_rewrite, build_runtime_reducer_packet, external_runtime_name,
    parse_runtime_event_kind, render_runtime_hook_output, runtime_command, runtime_matcher,
    runtime_session_id, runtime_source, ExternalHookRuntime,
};

#[cfg(test)]
use crate::cmd_hook_runner::workspace_cache_fingerprint;

#[derive(Args)]
pub struct HookArgs {
    #[command(subcommand)]
    pub command: HookCommands,
}

#[derive(Subcommand)]
pub enum HookCommands {
    Claude(ClaudeHookArgs),
    Codex(ClaudeHookArgs),
    Copilot(RuntimeHookArgs),
    Cursor(RuntimeHookArgs),
    Gemini(RuntimeHookArgs),
    Windsurf(RuntimeHookArgs),
    Log(HookLogArgs),
    Stats(HookStatsArgs),
    ServeHttp(HookHttpServerArgs),
    ReducerRunner(ReducerRunnerArgs),
    ReduceFixture(ReduceFixtureArgs),
}

#[derive(Args, Clone)]
pub struct ClaudeHookArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub event: Option<String>,
}

#[derive(Args, Clone)]
pub struct RuntimeHookArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub event: Option<String>,
}

#[derive(Args, Clone)]
pub struct HookLogArgs {
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args, Clone)]
pub struct HookStatsArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args, Clone)]
pub struct HookHttpServerArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub port: u16,
    #[arg(long)]
    pub token: String,
}

#[derive(Args, Clone)]
pub struct ReducerRunnerArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: Option<String>,
    #[arg(long)]
    pub session_id: Option<String>,
    #[arg(long)]
    pub family: String,
    #[arg(long)]
    pub kind: String,
    #[arg(long)]
    pub fingerprint: String,
    #[arg(long)]
    pub cwd: Option<String>,
    #[arg(long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    #[arg(trailing_var_arg = true)]
    pub argv: Vec<String>,
}

#[derive(Args, Clone)]
pub struct ReduceFixtureArgs {
    #[arg(long)]
    pub command: String,
    #[arg(long)]
    pub stdout_path: String,
    #[arg(long)]
    pub stderr_path: Option<String>,
    #[arg(long, default_value_t = 0)]
    pub exit_code: i32,
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: HookArgs) -> Result<i32> {
    match args.command {
        HookCommands::Claude(args) => run_claude(args),
        HookCommands::Codex(args) => run_claude(args),
        HookCommands::Copilot(args) => run_runtime_hook(args, ExternalHookRuntime::Copilot),
        HookCommands::Cursor(args) => run_runtime_hook(args, ExternalHookRuntime::Cursor),
        HookCommands::Gemini(args) => run_runtime_hook(args, ExternalHookRuntime::Gemini),
        HookCommands::Windsurf(args) => run_runtime_hook(args, ExternalHookRuntime::Windsurf),
        HookCommands::Log(args) => run_hook_log(args),
        HookCommands::Stats(args) => run_hook_stats(args),
        HookCommands::ServeHttp(args) => run_hook_http_server(args),
        HookCommands::ReducerRunner(args) => run_reducer_runner(args),
        HookCommands::ReduceFixture(args) => run_reduce_fixture(args),
    }
}

pub(crate) struct ClaudeHookOutcome {
    pub(crate) exit_code: i32,
    pub(crate) body: Option<String>,
}

struct RuntimeHookOutcome {
    exit_code: i32,
    body: Option<String>,
}

fn run_claude(args: ClaudeHookArgs) -> Result<i32> {
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;
    let payload = if buffer.trim().is_empty() {
        Value::Object(Default::default())
    } else {
        match serde_json::from_str(&buffer) {
            Ok(payload) => payload,
            Err(err) => {
                eprintln!("packet28 hook: ignoring malformed JSON payload: {err}");
                return Ok(0);
            }
        }
    };
    let root = resolve_hook_root(&args, &payload);
    match process_claude_hook_payload(&root, args.event.as_deref(), &payload, true) {
        Ok(outcome) => {
            if let Some(body) = outcome.body {
                println!("{body}");
            }
            Ok(outcome.exit_code)
        }
        Err(err) => {
            eprintln!("packet28 hook: allowing runtime action after processing error: {err:#}");
            Ok(0)
        }
    }
}

pub(crate) fn process_claude_hook_payload(
    root: &Path,
    event_override: Option<&str>,
    payload: &Value,
    bootstrap_http_server: bool,
) -> Result<ClaudeHookOutcome> {
    let runtime_config = load_hook_runtime_config(root);
    let event_kind = event_override
        .map(|value| parse_event_kind(Some(value)))
        .unwrap_or_else(|| parse_event_kind(json_string(payload, "hook_event_name").as_deref()));
    if bootstrap_http_server
        && matches!(
            event_kind,
            HookEventKind::SessionStart | HookEventKind::UserPromptSubmit
        )
    {
        ensure_hook_http_server(root, &runtime_config)?;
    }
    crate::broker_client::ensure_daemon(root)?;

    let session_id = json_string(payload, "session_id");
    let task_id = resolve_task_id(root, payload, session_id.as_deref())?;
    let matcher = json_string(payload, "matcher");
    let source = json_string(payload, "source");
    let rewrite = build_pretool_rewrite(
        &runtime_config,
        root,
        payload,
        event_kind,
        &task_id,
        session_id.as_deref(),
    )?;
    let reducer_packet = build_reducer_packet(&runtime_config, payload, event_kind);
    let response = crate::broker_client::hook_ingest(
        root,
        HookIngestRequest {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            event_kind,
            matcher: matcher.clone(),
            source,
            boundary_kind: boundary_for_event(event_kind),
            lifecycle_event: None,
            reducer_packet,
            host_context_budget_tokens: std::env::var("PACKET28_HOST_CONTEXT_BUDGET_TOKENS")
                .ok()
                .and_then(|v| v.parse().ok()),
        },
    )?;
    record_hook_event(HookEventInput {
        runtime: "claude",
        event_kind: hook_event_name(event_kind),
        session_id: session_id.as_deref(),
        task_id: Some(&task_id),
        matcher: matcher.as_deref(),
        payload_json: &serde_json::to_string(payload)?,
    })?;
    let _ = capture_hook_output("claude", root, event_kind, payload, session_id.as_deref());
    let additional_context = build_session_start_additional_context(
        root,
        event_kind,
        payload,
        response.additional_context.as_deref(),
    )?;
    let action_critic = build_pretool_action_critic(root, event_kind, payload, &task_id)
        .unwrap_or_else(|_| Vec::new());
    Ok(ClaudeHookOutcome {
        exit_code: if response.block_stop { 2 } else { 0 },
        body: render_hook_output(
            event_kind,
            rewrite,
            &response,
            additional_context,
            &action_critic,
        )?,
    })
}

fn run_runtime_hook(args: RuntimeHookArgs, runtime: ExternalHookRuntime) -> Result<i32> {
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;
    let payload = if buffer.trim().is_empty() {
        Value::Object(Default::default())
    } else {
        match serde_json::from_str(&buffer) {
            Ok(payload) => payload,
            Err(err) => {
                eprintln!(
                    "packet28 {} hook: ignoring malformed JSON payload: {err}",
                    external_runtime_name(runtime)
                );
                return Ok(0);
            }
        }
    };
    match process_runtime_hook_payload(args, runtime, payload) {
        Ok(outcome) => {
            if let Some(body) = outcome.body {
                println!("{body}");
            }
            Ok(outcome.exit_code)
        }
        Err(err) => {
            eprintln!(
                "packet28 {} hook: allowing runtime action after processing error: {err:#}",
                external_runtime_name(runtime)
            );
            Ok(0)
        }
    }
}

fn process_runtime_hook_payload(
    args: RuntimeHookArgs,
    runtime: ExternalHookRuntime,
    payload: Value,
) -> Result<RuntimeHookOutcome> {
    let root = resolve_runtime_hook_root(&args, &payload);
    let runtime_config = load_hook_runtime_config(&root);
    crate::broker_client::ensure_daemon(&root)?;

    let event_kind = args
        .event
        .as_deref()
        .map(|value| parse_runtime_event_kind(runtime, Some(value)))
        .unwrap_or_else(|| {
            parse_runtime_event_kind(runtime, json_string(&payload, "hook_event_name").as_deref())
        });
    let session_id = runtime_session_id(runtime, &payload);
    let task_id = resolve_runtime_task_id(&root, &payload, session_id.as_deref(), runtime)?;
    let matcher = runtime_matcher(runtime, &payload, event_kind);
    let rewrite = build_runtime_pretool_rewrite(
        runtime,
        &runtime_config,
        &root,
        &payload,
        event_kind,
        &task_id,
        session_id.as_deref(),
    )?;
    let reducer_packet = build_runtime_reducer_packet(runtime, &payload, event_kind);

    let _response = crate::broker_client::hook_ingest(
        &root,
        HookIngestRequest {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            event_kind,
            matcher: matcher.clone(),
            source: Some(runtime_source(runtime).to_string()),
            boundary_kind: boundary_for_event(event_kind),
            lifecycle_event: None,
            reducer_packet,
            host_context_budget_tokens: None,
        },
    )?;
    record_hook_event(HookEventInput {
        runtime: external_runtime_name(runtime),
        event_kind: hook_event_name(event_kind),
        session_id: session_id.as_deref(),
        task_id: Some(&task_id),
        matcher: matcher.as_deref(),
        payload_json: &serde_json::to_string(&payload)?,
    })?;
    let _ = capture_hook_output(
        external_runtime_name(runtime),
        &root,
        event_kind,
        &payload,
        session_id.as_deref(),
    );
    Ok(RuntimeHookOutcome {
        exit_code: 0,
        body: render_runtime_hook_output(runtime, event_kind, &payload, rewrite)?,
    })
}

fn run_hook_log(args: HookLogArgs) -> Result<i32> {
    let events = list_hook_events(args.limit)?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(events)?, args.pretty)?;
    } else {
        for event in events {
            println!(
                "id={} runtime={} event={} session={} task={} matcher={}",
                event.id,
                event.runtime,
                event.event_kind,
                event.session_id.unwrap_or_else(|| "n/a".to_string()),
                event.task_id.unwrap_or_else(|| "n/a".to_string()),
                event.matcher.unwrap_or_else(|| "n/a".to_string())
            );
        }
    }
    Ok(0)
}

fn run_hook_stats(args: HookStatsArgs) -> Result<i32> {
    let stats = hook_event_stats()?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(stats)?, args.pretty)?;
    } else {
        for stat in stats {
            println!(
                "runtime={} event={} count={}",
                stat.runtime, stat.event_kind, stat.event_count
            );
        }
    }
    Ok(0)
}

fn capture_hook_output(
    runtime: &str,
    root: &Path,
    event_kind: HookEventKind,
    payload: &Value,
    session_id: Option<&str>,
) -> Result<()> {
    if !matches!(
        event_kind,
        HookEventKind::PostToolUse | HookEventKind::PostToolUseFailure
    ) {
        return Ok(());
    }
    let raw_output = hook_pending_raw_output(payload, event_kind);
    let raw_output = raw_output.trim();
    if raw_output.is_empty() {
        return Ok(());
    }
    let raw_output = compact_text(raw_output, 8 * 1024);
    enqueue_pending_extraction(PendingExtractionInput {
        project: Some(&hook_project_name(root, payload)),
        tool_name: hook_tool_name(runtime, payload).as_deref(),
        raw_output: &raw_output,
    })?;
    let fallback_session = json_string(payload, "task_id");
    let transcript_session = session_id.or(fallback_session.as_deref());
    let project = hook_project_name(root, payload);
    append_transcript_message(TranscriptAppendInput {
        session: transcript_session,
        agent: Some(runtime),
        role: Some("tool"),
        content: &raw_output,
        source: Some("packet28-hook"),
        project: Some(&project),
    })?;
    Ok(())
}

fn hook_pending_raw_output(payload: &Value, event_kind: HookEventKind) -> String {
    match event_kind {
        HookEventKind::PostToolUse => payload
            .get("tool_response")
            .map(hook_output_text)
            .or_else(|| json_string(payload, "output"))
            .or_else(|| json_string(payload, "stdout"))
            .or_else(|| json_string(payload, "result"))
            .unwrap_or_default(),
        HookEventKind::PostToolUseFailure => {
            json_string(payload, "error").unwrap_or_else(|| hook_output_text(payload))
        }
        _ => String::new(),
    }
}

fn hook_project_name(root: &Path, payload: &Value) -> String {
    json_string(payload, "project")
        .or_else(|| {
            json_string(payload, "cwd").and_then(|cwd| {
                Path::new(&cwd)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
        })
        .or_else(|| {
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "project".to_string())
}

fn hook_tool_name(runtime: &str, payload: &Value) -> Option<String> {
    json_string(payload, "tool_name")
        .or_else(|| json_string(payload, "tool"))
        .or_else(|| json_string(payload, "name"))
        .or_else(|| json_string(payload, "command").map(|_| "Bash".to_string()))
        .or_else(|| Some(runtime.to_string()))
}

fn resolve_hook_root(args: &ClaudeHookArgs, payload: &Value) -> PathBuf {
    if args.root.trim() != "." {
        return crate::broker_client::resolve_root(&args.root);
    }
    json_string(payload, "cwd")
        .map(|cwd| crate::broker_client::resolve_root(&cwd))
        .unwrap_or_else(|| crate::broker_client::resolve_root("."))
}

fn resolve_task_id(root: &Path, payload: &Value, session_id: Option<&str>) -> Result<String> {
    if let Some(task_id) = json_string(payload, "task_id").filter(|value| !value.trim().is_empty())
    {
        crate::task_runtime::store_active_task(
            root,
            &ActiveTaskRecord {
                task_id: task_id.clone(),
                session_id: session_id.map(ToOwned::to_owned),
                updated_at_unix: now_unix(),
            },
        )?;
        return Ok(task_id);
    }
    if let Some(active) = crate::task_runtime::load_active_task(root) {
        if session_id.is_none() || active.session_id.as_deref() == session_id {
            return Ok(active.task_id);
        }
    }
    let task_id = session_id
        .map(crate::task_runtime::derive_claude_task_id)
        .unwrap_or_else(|| crate::broker_client::derive_task_id("claude-project"));
    crate::task_runtime::store_active_task(
        root,
        &ActiveTaskRecord {
            task_id: task_id.clone(),
            session_id: session_id.map(ToOwned::to_owned),
            updated_at_unix: now_unix(),
        },
    )?;
    Ok(task_id)
}

fn resolve_runtime_task_id(
    root: &Path,
    payload: &Value,
    session_id: Option<&str>,
    runtime: ExternalHookRuntime,
) -> Result<String> {
    if let Some(task_id) = json_string(payload, "task_id").filter(|value| !value.trim().is_empty())
    {
        crate::task_runtime::store_active_task(
            root,
            &ActiveTaskRecord {
                task_id: task_id.clone(),
                session_id: session_id.map(ToOwned::to_owned),
                updated_at_unix: now_unix(),
            },
        )?;
        return Ok(task_id);
    }
    if let Some(active) = crate::task_runtime::load_active_task(root) {
        return Ok(active.task_id);
    }
    let seed = match runtime {
        ExternalHookRuntime::Copilot => "copilot-project",
        ExternalHookRuntime::Cursor => "cursor-project",
        ExternalHookRuntime::Gemini => "gemini-project",
        ExternalHookRuntime::Windsurf => "windsurf-project",
    };
    let task_id = session_id
        .map(crate::task_runtime::derive_claude_task_id)
        .unwrap_or_else(|| crate::broker_client::derive_task_id(seed));
    crate::task_runtime::store_active_task(
        root,
        &ActiveTaskRecord {
            task_id: task_id.clone(),
            session_id: session_id.map(ToOwned::to_owned),
            updated_at_unix: now_unix(),
        },
    )?;
    Ok(task_id)
}

fn resolve_runtime_hook_root(args: &RuntimeHookArgs, payload: &Value) -> PathBuf {
    if args.root.trim() != "." {
        return crate::broker_client::resolve_root(&args.root);
    }
    if let Some(root) = json_string(payload, "cwd")
        .or_else(|| json_nested_string(payload, &["tool_info", "cwd"]))
        .or_else(|| json_nested_string(payload, &["workspace_root"]))
    {
        return crate::broker_client::resolve_root(&root);
    }
    if let Some(root) = payload
        .get("workspace_roots")
        .and_then(Value::as_array)
        .and_then(|roots| roots.first())
        .and_then(Value::as_str)
    {
        return crate::broker_client::resolve_root(root);
    }
    crate::broker_client::resolve_root(".")
}

fn parse_event_kind(value: Option<&str>) -> HookEventKind {
    match value.unwrap_or_default().trim() {
        "SessionStart" => HookEventKind::SessionStart,
        "UserPromptSubmit" => HookEventKind::UserPromptSubmit,
        "PreToolUse" => HookEventKind::PreToolUse,
        "PostToolUse" => HookEventKind::PostToolUse,
        "PostToolUseFailure" => HookEventKind::PostToolUseFailure,
        "CommandStarted" => HookEventKind::CommandStarted,
        "CommandProgress" => HookEventKind::CommandProgress,
        "CommandFinished" => HookEventKind::CommandFinished,
        "Stop" => HookEventKind::Stop,
        "SubagentStop" => HookEventKind::SubagentStop,
        "PreCompact" => HookEventKind::PreCompact,
        "SessionEnd" => HookEventKind::SessionEnd,
        _ => HookEventKind::Unknown,
    }
}

fn boundary_for_event(kind: HookEventKind) -> HookBoundaryKind {
    match kind {
        HookEventKind::Stop => HookBoundaryKind::Stop,
        HookEventKind::SubagentStop => HookBoundaryKind::SubagentStop,
        HookEventKind::PreCompact => HookBoundaryKind::PreCompact,
        HookEventKind::SessionEnd => HookBoundaryKind::SessionEnd,
        _ => HookBoundaryKind::None,
    }
}

fn hook_event_name(kind: HookEventKind) -> &'static str {
    match kind {
        HookEventKind::SessionStart => "session_start",
        HookEventKind::UserPromptSubmit => "user_prompt_submit",
        HookEventKind::PreToolUse => "pre_tool_use",
        HookEventKind::PostToolUse => "post_tool_use",
        HookEventKind::PostToolUseFailure => "post_tool_use_failure",
        HookEventKind::CommandStarted => "command_started",
        HookEventKind::CommandProgress => "command_progress",
        HookEventKind::CommandFinished => "command_finished",
        HookEventKind::Stop => "stop",
        HookEventKind::SubagentStop => "subagent_stop",
        HookEventKind::PreCompact => "pre_compact",
        HookEventKind::SessionEnd => "session_end",
        HookEventKind::Unknown => "unknown",
    }
}

fn render_hook_output(
    event_kind: HookEventKind,
    rewrite: Option<Value>,
    response: &packet28_daemon_core::HookIngestResponse,
    session_start_context: Option<String>,
    action_critic: &[String],
) -> Result<Option<String>> {
    match event_kind {
        HookEventKind::SessionStart => {
            if let Some(additional_context) = session_start_context {
                return Ok(Some(serde_json::to_string(&json!({
                    "hookSpecificOutput": {
                        "hookEventName": "SessionStart",
                        "additionalContext": additional_context,
                    }
                }))?));
            }
        }
        HookEventKind::PreToolUse => {
            let critic_reason = render_action_critic_reason(action_critic);
            if let Some(updated_input) = rewrite {
                let mut hook_output = json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                        "updatedInput": updated_input,
                    }
                });
                if let Some(reason) = critic_reason {
                    hook_output["hookSpecificOutput"]["permissionDecisionReason"] = json!(reason);
                }
                return Ok(Some(serde_json::to_string(&hook_output)?));
            }
            if let Some(reason) = critic_reason {
                return Ok(Some(serde_json::to_string(&json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                        "permissionDecisionReason": reason,
                    }
                }))?));
            }
        }
        HookEventKind::Stop | HookEventKind::SubagentStop => {
            if response.relaunch_requested {
                // Daemon is handling relaunch — allow the stop to proceed.
                // The next session will bootstrap from the handoff artifact.
                eprintln!(
                    "packet28: context threshold reached, daemon relaunch queued (artifact={})",
                    response
                        .latest_handoff_artifact_id
                        .as_deref()
                        .unwrap_or("pending")
                );
            } else if response.block_stop {
                return Ok(Some(serde_json::to_string(&json!({
                    "decision": "block",
                    "reason": response.stop_reason.clone().unwrap_or_else(|| "Packet28 requires an intention before stop".to_string()),
                }))?));
            } else if let Some(stop_reason) = response.stop_reason.as_ref() {
                return Ok(Some(serde_json::to_string(&json!({
                    "systemMessage": stop_reason,
                }))?));
            }
        }
        _ => {}
    }
    Ok(None)
}

fn build_pretool_action_critic(
    root: &Path,
    event_kind: HookEventKind,
    payload: &Value,
    task_id: &str,
) -> Result<Vec<String>> {
    if !matches!(event_kind, HookEventKind::PreToolUse) {
        return Ok(Vec::new());
    }
    let Some(command) = runtime_command(payload).filter(|command| !command.trim().is_empty())
    else {
        return Ok(Vec::new());
    };
    let response = crate::broker_client::get_context(
        root,
        BrokerGetContextRequest {
            task_id: task_id.to_string(),
            action: Some(BrokerAction::ChooseTool),
            query: Some(command),
            tool_name: json_string(payload, "tool_name"),
            include_sections: vec!["action_critic".to_string()],
            max_sections: Some(1),
            default_max_items_per_section: Some(4),
            persist_artifacts: Some(false),
            ..BrokerGetContextRequest::default()
        },
    )?;
    Ok(response
        .sections
        .iter()
        .find(|section| section.id == "action_critic")
        .map(|section| {
            section
                .body
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| line.trim_start_matches("- ").to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default())
}

fn render_action_critic_reason(lines: &[String]) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    let joined = lines
        .iter()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!("Packet28 action critic: {joined}"))
}

fn build_session_start_additional_context(
    root: &Path,
    event_kind: HookEventKind,
    payload: &Value,
    daemon_context: Option<&str>,
) -> Result<Option<String>> {
    if !matches!(event_kind, HookEventKind::SessionStart) {
        return Ok(daemon_context.map(ToOwned::to_owned));
    }
    let project = hook_project_name(root, payload);
    let max_tokens = std::env::var("PACKET28_HOOK_WAKEUP_TOKENS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(500);
    let wakeup = build_wakeup_pack_for_injection(None, Some(&project), 8, max_tokens)?;
    let context = match (daemon_context, wakeup) {
        (Some(daemon), Some(wakeup)) if !daemon.trim().is_empty() => {
            format!("{daemon}\n\n{wakeup}")
        }
        (Some(daemon), _) if !daemon.trim().is_empty() => daemon.to_string(),
        (_, Some(wakeup)) => wakeup,
        _ => return Ok(None),
    };
    Ok(Some(context))
}

fn build_pretool_rewrite(
    runtime_config: &HookRuntimeConfig,
    root: &Path,
    payload: &Value,
    event_kind: HookEventKind,
    task_id: &str,
    session_id: Option<&str>,
) -> Result<Option<Value>> {
    if !matches!(event_kind, HookEventKind::PreToolUse) || !runtime_config.rewrite_enabled {
        return Ok(None);
    }
    if json_string(payload, "tool_name").as_deref() != Some("Bash") {
        return Ok(None);
    }
    let Some(tool_input) = payload.get("tool_input") else {
        return Ok(None);
    };
    let Some(command) = json_string(tool_input, "command") else {
        return Ok(None);
    };
    let hook_cwd = json_string(payload, "cwd").unwrap_or_else(|| root.display().to_string());
    let hook_cwd_path = std::path::Path::new(&hook_cwd);
    let mut decision = crate::route_registry::decide_command_route_with_cwd_and_root(
        &command,
        hook_cwd_path,
        root,
    );

    // In hook context, promote NativeTool → ReducerRewrite when the reducer-core
    // also classifies the command (e.g. head/cat/sed → fs family).
    if matches!(decision.kind, crate::route_registry::RouteKind::NativeTool)
        && !decision
            .native_tool
            .as_ref()
            .is_some_and(|tool| matches!(tool.kind, crate::route_registry::NativeToolKind::Grep))
    {
        if let Some(spec) = packet28_reducer_core::classify_command_argv(&command, &decision.argv) {
            decision = crate::route_registry::RouteDecision {
                kind: crate::route_registry::RouteKind::ReducerRewrite,
                reason: None,
                argv: decision.argv,
                env_assignments: decision.env_assignments,
                reducer_spec: Some(spec),
                native_tool: None,
                original_argv: decision.original_argv,
                wrapper_prefix: decision.wrapper_prefix,
                original_command: decision.original_command,
            };
        }
    }

    // Only allow compact local rewrites through hooks. ProxyPassthrough and
    // RawPassthrough are not rewritten in the hook path.
    let proceed = match &decision.kind {
        crate::route_registry::RouteKind::ReducerRewrite => {
            decision.reducer_spec.as_ref().is_some_and(|spec| {
                runtime_config
                    .reducer_allowlist
                    .iter()
                    .any(|entry| entry == &spec.family)
            })
        }
        crate::route_registry::RouteKind::NativeTool => true,
        crate::route_registry::RouteKind::TomlFilterRewrite => true,
        crate::route_registry::RouteKind::CompoundRewrite => true,
        _ => false,
    };
    if !proceed {
        return Ok(None);
    }

    let mut updated_input = tool_input.clone();
    let Some(rewritten) =
        crate::route_registry::build_route_rewrite(root, task_id, session_id, &hook_cwd, &decision)
    else {
        return Ok(None);
    };
    if let Some(object) = updated_input.as_object_mut() {
        object.insert("command".to_string(), Value::String(rewritten));
    } else {
        updated_input = json!({ "command": rewritten });
    }
    Ok(Some(updated_input))
}

fn load_hook_runtime_config(root: &Path) -> HookRuntimeConfig {
    fs::read_to_string(hook_runtime_config_path(root))
        .ok()
        .and_then(|raw| serde_json::from_str::<HookRuntimeConfig>(&raw).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretool_rewrites_strict_git_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"git status --short src/lib.rs"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            Some("session-1"),
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();
        assert!(command.contains("hook reducer-runner"));
        assert!(command.contains("--family git"));
        assert!(command.contains("--kind git_status"));
    }

    #[test]
    fn pretool_keeps_grep_on_native_compact_path_with_basic_alternation() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{
                "command": r"grep 'fn classify\|Mutation' crates/packet28-reducer-core/src/command.rs"
            }
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-grep",
            Some("session-1"),
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();

        assert!(command.contains(" compact grep "));
        assert!(!command.contains("hook reducer-runner"));
        assert!(command.contains("fn classify\\|Mutation"));
        assert!(command.contains("crates/packet28-reducer-core/src/command.rs"));
    }

    #[test]
    fn pretool_hook_output_surfaces_action_critic_without_rewrite() {
        let body = render_hook_output(
            HookEventKind::PreToolUse,
            None,
            &packet28_daemon_core::HookIngestResponse::default(),
            None,
            &["destructive_command: inspect scope first".to_string()],
        )
        .unwrap()
        .unwrap();
        let payload: Value = serde_json::from_str(&body).unwrap();
        let output = &payload["hookSpecificOutput"];
        assert_eq!(output["hookEventName"], "PreToolUse");
        assert_eq!(output["permissionDecision"], "allow");
        assert!(output["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("Packet28 action critic"));
    }

    #[test]
    fn grep_hook_packet_preserves_actionable_regions_and_preview() {
        let input = json!({
            "pattern": r"fn classify\|Mutation",
            "include": ["crates/packet28-reducer-core/src/command.rs"]
        });
        let response = json!({
            "output": "crates/packet28-reducer-core/src/command.rs:16:pub fn classify_command(command: &str) {}\ncrates/packet28-reducer-core/src/command.rs:34:pub fn classify_command_argv(command: &str) {}\n"
        });

        let packet = build_grep_packet(&input, &response).unwrap();

        assert_eq!(
            packet.search_query.as_deref(),
            Some(r"fn classify\|Mutation")
        );
        assert!(packet
            .regions
            .contains(&"crates/packet28-reducer-core/src/command.rs:16-16".to_string()));
        assert!(packet
            .regions
            .contains(&"crates/packet28-reducer-core/src/command.rs:34-34".to_string()));
        let preview = packet.compact_preview.unwrap();
        assert!(preview.contains("Grep found 2 matches"));
        assert!(preview.contains("crates/packet28-reducer-core/src/command.rs:16:"));
    }

    #[test]
    fn bash_grep_post_capture_preserves_actionable_regions_without_pretool_rewrite() {
        let input = json!({
            "command": r"grep -n 'fn classify\|Mutation\|fn classify_command' crates/packet28-reducer-core/src/command.rs"
        });
        let response = json!({
            "stdout": "16:pub fn classify_command(command: &str) -> Option<CommandReducerSpec> {\n34:pub fn classify_command_argv(command: &str, argv: &[String]) -> Option<CommandReducerSpec> {\n"
        });

        let packet = build_bash_packet(&input, &response).unwrap();

        assert_eq!(packet.tool_name, "Bash");
        assert_eq!(packet.packet_type, "packet28.hook.bash.grep.v1");
        assert_eq!(
            packet.search_query.as_deref(),
            Some(r"fn classify\|Mutation\|fn classify_command")
        );
        assert!(packet
            .regions
            .contains(&"crates/packet28-reducer-core/src/command.rs:16-16".to_string()));
        assert!(packet
            .regions
            .contains(&"crates/packet28-reducer-core/src/command.rs:34-34".to_string()));
        let preview = packet.compact_preview.unwrap();
        assert!(preview.contains("Grep found 2 matches"));
        assert!(preview.contains("crates/packet28-reducer-core/src/command.rs:16:"));
    }

    #[test]
    fn pretool_hook_output_preserves_rewrite_with_action_critic() {
        let body = render_hook_output(
            HookEventKind::PreToolUse,
            Some(json!({"command": "Packet28 hook reducer-runner -- git status"})),
            &packet28_daemon_core::HookIngestResponse::default(),
            None,
            &["broad_search: add focus_paths".to_string()],
        )
        .unwrap()
        .unwrap();
        let payload: Value = serde_json::from_str(&body).unwrap();
        let output = &payload["hookSpecificOutput"];
        assert_eq!(output["permissionDecision"], "allow");
        assert_eq!(
            output["updatedInput"]["command"],
            "Packet28 hook reducer-runner -- git status"
        );
        assert!(output["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("broad_search"));
    }

    #[test]
    fn runtime_pretool_rewrites_use_shared_route_planner() {
        let root = PathBuf::from("/tmp/demo");
        let config = HookRuntimeConfig::default();

        let claude = build_pretool_rewrite(
            &config,
            &root,
            &json!({
                "tool_name": "Bash",
                "tool_input": {"command": "sudo git status --short"}
            }),
            HookEventKind::PreToolUse,
            "task-rtk",
            Some("session-rtk"),
        )
        .unwrap()
        .unwrap();
        let claude_command = claude["command"].as_str().unwrap();
        assert!(claude_command.contains("hook reducer-runner"));
        assert!(claude_command.contains("--kind git_status"));
        assert!(claude_command.ends_with(" -- git status --short"));

        let cursor = build_runtime_pretool_rewrite(
            ExternalHookRuntime::Cursor,
            &config,
            &root,
            &json!({
                "command": "env RUST_BACKTRACE=1 cargo test",
                "cwd": "/tmp/demo"
            }),
            HookEventKind::PreToolUse,
            "task-rtk",
            Some("session-rtk"),
        )
        .unwrap()
        .unwrap();
        let cursor_command = cursor["command"].as_str().unwrap();
        assert!(cursor_command.contains("--kind rust_test"));
        assert!(cursor_command.contains("--env RUST_BACKTRACE=1"));
        assert!(cursor_command.ends_with(" -- cargo test"));

        let copilot = build_runtime_pretool_rewrite(
            ExternalHookRuntime::Copilot,
            &config,
            &root,
            &json!({
                "tool_name": "runTerminalCommand",
                "tool_input": {"command": "/usr/bin/git status --short"},
                "workspace_root": "/tmp/demo"
            }),
            HookEventKind::PreToolUse,
            "task-rtk",
            Some("session-rtk"),
        )
        .unwrap()
        .unwrap();
        let copilot_command = copilot["command"].as_str().unwrap();
        assert!(copilot_command.contains("--kind git_status"));
        assert!(copilot_command.ends_with(" -- git status --short"));

        let gemini = build_runtime_pretool_rewrite(
            ExternalHookRuntime::Gemini,
            &config,
            &root,
            &json!({
                "tool_name": "run_shell_command",
                "tool_input": {"command": "sudo git status --short"},
                "cwd": "/tmp/demo"
            }),
            HookEventKind::PreToolUse,
            "task-rtk",
            Some("session-rtk"),
        )
        .unwrap()
        .unwrap();
        let gemini_command = gemini["command"].as_str().unwrap();
        assert!(gemini_command.contains("--kind git_status"));
        assert!(gemini_command.ends_with(" -- git status --short"));
    }

    #[test]
    fn rust_workspace_fingerprint_changes_for_out_of_band_source_edit() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"demo\"\n").unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn value() -> i32 { 1 }\n",
        )
        .unwrap();
        let spec = classify_command("cargo test --lib").unwrap();

        let before = workspace_cache_fingerprint(dir.path(), dir.path(), &spec);
        fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn value() -> i32 { 2 }\n",
        )
        .unwrap();
        let after = workspace_cache_fingerprint(dir.path(), dir.path(), &spec);

        assert_ne!(before, after);
    }

    #[test]
    fn pretool_declines_composed_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"cargo test 2>&1 | grep FAILED"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            None,
        )
        .unwrap();
        assert!(rewrite.is_none());
    }

    #[test]
    fn pretool_rewrites_supported_compound_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"cargo test | grep FAIL && git status --short"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            None,
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();
        assert!(command.contains("| grep FAIL &&"));
        assert_eq!(command.matches("hook reducer-runner").count(), 2);
    }

    #[test]
    fn pretool_rewrites_strict_fs_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"head -n 5 README.md"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            Some("session-1"),
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();
        assert!(command.contains("hook reducer-runner"));
        assert!(command.contains("--family fs"));
        assert!(command.contains("--kind fs_head"));
    }

    #[test]
    fn pretool_rewrites_strict_rust_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"cargo test -p packet28-reducer-core"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            Some("session-1"),
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();
        assert!(command.contains("hook reducer-runner"));
        assert!(command.contains("--family rust"));
        assert!(command.contains("--kind rust_test"));
    }

    #[test]
    fn pretool_declines_ambiguous_fs_sed_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"sed -i 1,4p README.md"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            None,
        )
        .unwrap();
        assert!(rewrite.is_none());
    }

    #[test]
    fn pretool_rewrites_strict_github_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"gh pr list --limit 5"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            Some("session-1"),
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();
        assert!(command.contains("hook reducer-runner"));
        assert!(command.contains("--family github"));
        assert!(command.contains("--kind gh_pr_list"));
    }

    #[test]
    fn pretool_declines_ambiguous_github_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"gh pr list --json title"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            None,
        )
        .unwrap();
        assert!(rewrite.is_none());
    }

    #[test]
    fn pretool_rewrites_strict_python_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"python3 -m pytest tests"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            Some("session-1"),
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();
        assert!(command.contains("hook reducer-runner"));
        assert!(command.contains("--family python"));
        assert!(command.contains("--kind python_pytest"));
    }

    #[test]
    fn pretool_declines_ambiguous_python_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"ruff check --output-format json src"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            None,
        )
        .unwrap();
        assert!(rewrite.is_none());
    }

    #[test]
    fn pretool_rewrites_strict_javascript_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"npx tsc --noEmit"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            Some("session-1"),
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();
        assert!(command.contains("hook reducer-runner"));
        assert!(command.contains("--family javascript"));
        assert!(command.contains("--kind javascript_tsc"));
    }

    #[test]
    fn pretool_declines_ambiguous_javascript_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"eslint --format json src"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            None,
        )
        .unwrap();
        assert!(rewrite.is_none());
    }

    #[test]
    fn pretool_rewrites_strict_go_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"go test ./..."}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            Some("session-1"),
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();
        assert!(command.contains("hook reducer-runner"));
        assert!(command.contains("--family go"));
        assert!(command.contains("--kind go_test"));
    }

    #[test]
    fn pretool_declines_ambiguous_go_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"go test -json ./..."}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            None,
        )
        .unwrap();
        assert!(rewrite.is_none());
    }

    #[test]
    fn pretool_rewrites_strict_infra_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"kubectl get pods"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            Some("session-1"),
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();
        assert!(command.contains("hook reducer-runner"));
        assert!(command.contains("--family infra"));
        assert!(command.contains("--kind kubectl_get"));
    }

    #[test]
    fn pretool_rewrites_strict_ruby_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"bundle exec rspec spec/models/user_spec.rb"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            Some("session-1"),
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();
        assert!(command.contains("hook reducer-runner"));
        assert!(command.contains("--family ruby"));
        assert!(command.contains("--kind ruby_rspec"));
    }

    #[test]
    fn pretool_rewrites_strict_dotnet_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"dotnet test Packet28.Tests.csproj"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            Some("session-1"),
        )
        .unwrap()
        .unwrap();
        let command = rewrite["command"].as_str().unwrap();
        assert!(command.contains("hook reducer-runner"));
        assert!(command.contains("--family dotnet"));
        assert!(command.contains("--kind dotnet_test"));
    }

    #[test]
    fn pretool_declines_ambiguous_infra_command() {
        let root = PathBuf::from("/tmp/demo");
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"curl -o out.txt https://example.com"}
        });
        let rewrite = build_pretool_rewrite(
            &HookRuntimeConfig::default(),
            &root,
            &payload,
            HookEventKind::PreToolUse,
            "task-123",
            None,
        )
        .unwrap();
        assert!(rewrite.is_none());
    }

    #[test]
    fn post_tool_skips_reducer_runner_command() {
        let packet = build_reducer_packet(
            &HookRuntimeConfig::default(),
            &json!({
                "tool_name":"Bash",
                "tool_input":{"command":"Packet28 hook reducer-runner --root . -- task"},
                "tool_response":{"stdout":"done"}
            }),
            HookEventKind::PostToolUse,
        );
        assert!(packet.is_none());
    }

    #[test]
    fn post_tool_failure_captures_failed_bash_packet() {
        let packet = build_reducer_packet(
            &HookRuntimeConfig::default(),
            &json!({
                "tool_name":"Bash",
                "tool_input":{"command":"git status --short src/lib.rs"},
                "error":"fatal: not a git repository"
            }),
            HookEventKind::PostToolUseFailure,
        )
        .unwrap();
        assert!(packet.failed);
        assert_eq!(packet.reducer_family.as_deref(), Some("git"));
        assert_eq!(packet.canonical_command_kind.as_deref(), Some("git_status"));
        assert!(packet.summary.contains("fatal: not a git repository"));
    }

    #[test]
    fn read_reducer_marks_read_operation() {
        let packet = build_read_packet(
            &json!({"file_path":"src/lib.rs","offset":10,"limit":5}),
            &json!({"content":"demo"}),
        )
        .unwrap();
        assert_eq!(
            packet.operation_kind,
            suite_packet_core::ToolOperationKind::Read
        );
        assert_eq!(packet.paths, vec!["src/lib.rs".to_string()]);
        assert_eq!(
            packet.cache_fingerprint.as_deref(),
            Some("read:src/lib.rs:10:14")
        );
    }
}
