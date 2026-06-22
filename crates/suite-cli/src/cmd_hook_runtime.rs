use std::path::Path;

use anyhow::Result;
use packet28_daemon_core::{HookEventKind, HookReducerPacket, HookRuntimeConfig};
use serde_json::{json, Value};

use crate::cmd_hook_packets::packet_from_parts;
use crate::cmd_hook_support::{
    compact_text, extract_command_paths, first_nonempty_line, json_nested_string, json_string,
};

#[derive(Clone, Copy)]
pub(super) enum ExternalHookRuntime {
    Copilot,
    Cursor,
    Gemini,
    Windsurf,
}

pub(super) fn parse_runtime_event_kind(
    runtime: ExternalHookRuntime,
    value: Option<&str>,
) -> HookEventKind {
    match runtime {
        ExternalHookRuntime::Copilot => match value.unwrap_or_default().trim() {
            "" | "PreToolUse" | "preToolUse" | "beforeToolUse" => HookEventKind::PreToolUse,
            "PostToolUse" | "postToolUse" | "afterToolUse" => HookEventKind::PostToolUse,
            _ => HookEventKind::Unknown,
        },
        ExternalHookRuntime::Cursor => match value.unwrap_or_default().trim() {
            "beforeSubmitPrompt" => HookEventKind::UserPromptSubmit,
            "beforeShellExecution" => HookEventKind::PreToolUse,
            "afterShellExecution" => HookEventKind::PostToolUse,
            "stop" | "afterAgentResponse" => HookEventKind::Stop,
            _ => HookEventKind::Unknown,
        },
        ExternalHookRuntime::Gemini => match value.unwrap_or_default().trim() {
            "" | "BeforeTool" | "before_tool" => HookEventKind::PreToolUse,
            "AfterTool" | "after_tool" => HookEventKind::PostToolUse,
            _ => HookEventKind::Unknown,
        },
        ExternalHookRuntime::Windsurf => match value.unwrap_or_default().trim() {
            "pre_user_prompt" => HookEventKind::UserPromptSubmit,
            "pre_run_command" => HookEventKind::PreToolUse,
            "post_run_command" => HookEventKind::PostToolUse,
            "post_cascade_response" | "post_cascade_response_with_transcript" => {
                HookEventKind::Stop
            }
            _ => HookEventKind::Unknown,
        },
    }
}

pub(super) fn runtime_session_id(runtime: ExternalHookRuntime, payload: &Value) -> Option<String> {
    match runtime {
        ExternalHookRuntime::Copilot => {
            json_string(payload, "conversation_id").or_else(|| json_string(payload, "session_id"))
        }
        ExternalHookRuntime::Cursor => {
            json_string(payload, "conversation_id").or_else(|| json_string(payload, "session_id"))
        }
        ExternalHookRuntime::Gemini => {
            json_string(payload, "session_id").or_else(|| json_string(payload, "conversation_id"))
        }
        ExternalHookRuntime::Windsurf => {
            json_string(payload, "trajectory_id").or_else(|| json_string(payload, "execution_id"))
        }
    }
}

pub(super) fn runtime_matcher(
    runtime: ExternalHookRuntime,
    payload: &Value,
    event_kind: HookEventKind,
) -> Option<String> {
    match runtime {
        ExternalHookRuntime::Copilot => match event_kind {
            HookEventKind::PreToolUse | HookEventKind::PostToolUse => {
                json_string(payload, "tool_name")
                    .or_else(|| json_string(payload, "toolName"))
                    .or_else(|| Some("Bash".to_string()))
            }
            _ => json_string(payload, "hook_event_name"),
        },
        ExternalHookRuntime::Cursor => match event_kind {
            HookEventKind::PreToolUse | HookEventKind::PostToolUse => Some("Bash".to_string()),
            _ => json_string(payload, "hook_event_name"),
        },
        ExternalHookRuntime::Gemini => match event_kind {
            HookEventKind::PreToolUse | HookEventKind::PostToolUse => {
                json_string(payload, "tool_name").or_else(|| Some("run_shell_command".to_string()))
            }
            _ => json_string(payload, "hook_event_name"),
        },
        ExternalHookRuntime::Windsurf => match event_kind {
            HookEventKind::PreToolUse | HookEventKind::PostToolUse => Some("Bash".to_string()),
            _ => json_string(payload, "agent_action_name"),
        },
    }
}

pub(super) fn runtime_command(payload: &Value) -> Option<String> {
    json_nested_string(payload, &["tool_input", "command"])
        .or_else(|| json_string(payload, "command"))
        .or_else(|| json_string(payload, "command_line"))
        .or_else(|| json_string(payload, "shell_command"))
}

fn copilot_cli_command(payload: &Value) -> Option<String> {
    if json_string(payload, "toolName").as_deref() != Some("bash") {
        return None;
    }
    let args = json_string(payload, "toolArgs")?;
    let parsed = serde_json::from_str::<Value>(&args).ok()?;
    json_string(&parsed, "command")
}

fn is_copilot_cli_payload(payload: &Value) -> bool {
    payload.get("toolName").is_some() || payload.get("toolArgs").is_some()
}

pub(super) fn build_runtime_reducer_packet(
    runtime: ExternalHookRuntime,
    payload: &Value,
    event_kind: HookEventKind,
) -> Option<HookReducerPacket> {
    match runtime {
        ExternalHookRuntime::Copilot => build_copilot_reducer_packet(payload, event_kind),
        ExternalHookRuntime::Cursor => build_cursor_reducer_packet(payload, event_kind),
        ExternalHookRuntime::Gemini => build_gemini_reducer_packet(payload, event_kind),
        ExternalHookRuntime::Windsurf => build_windsurf_reducer_packet(payload, event_kind),
    }
}

pub(super) fn build_runtime_pretool_rewrite(
    runtime: ExternalHookRuntime,
    runtime_config: &HookRuntimeConfig,
    root: &Path,
    payload: &Value,
    event_kind: HookEventKind,
    task_id: &str,
    session_id: Option<&str>,
) -> Result<Option<Value>> {
    if !matches!(
        runtime,
        ExternalHookRuntime::Copilot | ExternalHookRuntime::Cursor | ExternalHookRuntime::Gemini
    ) {
        return Ok(None);
    }
    if !matches!(event_kind, HookEventKind::PreToolUse) {
        return Ok(None);
    }
    let command = match runtime {
        ExternalHookRuntime::Copilot if is_copilot_cli_payload(payload) => {
            let Some(command) = copilot_cli_command(payload) else {
                return Ok(None);
            };
            command
        }
        ExternalHookRuntime::Copilot => {
            let tool_name = json_string(payload, "tool_name");
            if !matches!(
                tool_name.as_deref(),
                Some("runTerminalCommand") | Some("Bash") | Some("bash")
            ) {
                return Ok(None);
            }
            let Some(command) = runtime_command(payload) else {
                return Ok(None);
            };
            command
        }
        ExternalHookRuntime::Gemini => {
            if json_string(payload, "tool_name").as_deref() != Some("run_shell_command") {
                return Ok(None);
            }
            let Some(command) = runtime_command(payload) else {
                return Ok(None);
            };
            command
        }
        _ => {
            let Some(command) = runtime_command(payload) else {
                return Ok(None);
            };
            command
        }
    };
    let normalized = json!({
        "tool_name": "Bash",
        "cwd": json_string(payload, "cwd")
            .or_else(|| json_nested_string(payload, &["tool_info", "cwd"]))
            .or_else(|| json_string(payload, "workspace_root"))
            .unwrap_or_else(|| root.display().to_string()),
        "tool_input": {
            "command": command
        }
    });
    super::build_pretool_rewrite(
        runtime_config,
        root,
        &normalized,
        event_kind,
        task_id,
        session_id,
    )
}

pub(super) fn render_runtime_hook_output(
    runtime: ExternalHookRuntime,
    event_kind: HookEventKind,
    payload: &Value,
    rewrite: Option<Value>,
) -> Result<Option<String>> {
    match (runtime, event_kind, rewrite) {
        (ExternalHookRuntime::Copilot, HookEventKind::PreToolUse, Some(updated_input))
            if is_copilot_cli_payload(payload) =>
        {
            let command = json_string(&updated_input, "command").unwrap_or_default();
            Ok(Some(serde_json::to_string(&json!({
                "permissionDecision": "deny",
                "permissionDecisionReason": format!(
                    "Token savings: use `{}` instead (Packet28 reduces command output tokens)",
                    command
                ),
            }))?))
        }
        (ExternalHookRuntime::Copilot, HookEventKind::PreToolUse, Some(updated_input)) => {
            Ok(Some(serde_json::to_string(&json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "permissionDecisionReason": "Packet28 auto-rewrite",
                    "updatedInput": updated_input,
                },
            }))?))
        }
        (ExternalHookRuntime::Copilot, HookEventKind::PreToolUse, None)
            if is_copilot_cli_payload(payload) =>
        {
            Ok(None)
        }
        (ExternalHookRuntime::Copilot, HookEventKind::PreToolUse, None) => Ok(None),
        (ExternalHookRuntime::Cursor, HookEventKind::PreToolUse, Some(updated_input)) => {
            Ok(Some(serde_json::to_string(&json!({
                "permission": "allow",
                "updated_input": updated_input,
            }))?))
        }
        (ExternalHookRuntime::Cursor, HookEventKind::PreToolUse, None) => {
            Ok(Some("{}".to_string()))
        }
        (ExternalHookRuntime::Gemini, HookEventKind::PreToolUse, Some(updated_input)) => {
            Ok(Some(serde_json::to_string(&json!({
                "decision": "allow",
                "hookSpecificOutput": {
                    "tool_input": updated_input,
                },
            }))?))
        }
        (ExternalHookRuntime::Gemini, HookEventKind::PreToolUse, None) => {
            Ok(Some(serde_json::to_string(&json!({
                "decision": "allow",
            }))?))
        }
        _ => Ok(None),
    }
}

fn build_cursor_reducer_packet(
    payload: &Value,
    event_kind: HookEventKind,
) -> Option<HookReducerPacket> {
    if !matches!(event_kind, HookEventKind::PostToolUse) {
        return None;
    }
    let command = runtime_command(payload)?;
    let output = json_string(payload, "output")
        .or_else(|| json_string(payload, "result"))
        .or_else(|| json_string(payload, "stderr"))
        .or_else(|| json_string(payload, "stdout"))
        .unwrap_or_default();
    let summary = first_nonempty_line(&output)
        .unwrap_or_else(|| format!("command completed: {}", compact_text(&command, 100)));
    let paths = extract_command_paths(&command);
    Some(packet_from_parts(
        "packet28.hook.cursor.command.v1",
        "Bash",
        suite_packet_core::ToolOperationKind::Generic,
        Some("cursor_native".to_string()),
        Some("shell".to_string()),
        summary,
        Some(command),
        None,
        paths,
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        payload.clone(),
        false,
    ))
}

fn build_copilot_reducer_packet(
    payload: &Value,
    event_kind: HookEventKind,
) -> Option<HookReducerPacket> {
    if !matches!(event_kind, HookEventKind::PostToolUse) {
        return None;
    }
    let command = copilot_cli_command(payload).or_else(|| runtime_command(payload))?;
    let output = json_string(payload, "output")
        .or_else(|| json_string(payload, "result"))
        .or_else(|| json_string(payload, "stderr"))
        .or_else(|| json_string(payload, "stdout"))
        .unwrap_or_default();
    let summary = first_nonempty_line(&output)
        .unwrap_or_else(|| format!("command completed: {}", compact_text(&command, 100)));
    let paths = extract_command_paths(&command);
    Some(packet_from_parts(
        "packet28.hook.copilot.command.v1",
        "Bash",
        suite_packet_core::ToolOperationKind::Generic,
        Some("copilot_native".to_string()),
        Some("shell".to_string()),
        summary,
        Some(command),
        None,
        paths,
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        payload.clone(),
        false,
    ))
}

fn build_gemini_reducer_packet(
    payload: &Value,
    event_kind: HookEventKind,
) -> Option<HookReducerPacket> {
    if !matches!(event_kind, HookEventKind::PostToolUse) {
        return None;
    }
    if json_string(payload, "tool_name").as_deref() != Some("run_shell_command") {
        return None;
    }
    let command = runtime_command(payload)?;
    let output = json_string(payload, "output")
        .or_else(|| json_string(payload, "result"))
        .or_else(|| json_string(payload, "stderr"))
        .or_else(|| json_string(payload, "stdout"))
        .unwrap_or_default();
    let summary = first_nonempty_line(&output)
        .unwrap_or_else(|| format!("command completed: {}", compact_text(&command, 100)));
    let paths = extract_command_paths(&command);
    Some(packet_from_parts(
        "packet28.hook.gemini.command.v1",
        "run_shell_command",
        suite_packet_core::ToolOperationKind::Generic,
        Some("gemini_native".to_string()),
        Some("shell".to_string()),
        summary,
        Some(command),
        None,
        paths,
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        payload.clone(),
        false,
    ))
}

fn build_windsurf_reducer_packet(
    payload: &Value,
    event_kind: HookEventKind,
) -> Option<HookReducerPacket> {
    if !matches!(event_kind, HookEventKind::PostToolUse) {
        return None;
    }
    let command = json_nested_string(payload, &["tool_info", "command_line"])?;
    let summary = format!("command completed: {}", compact_text(&command, 100));
    let paths = extract_command_paths(&command);
    Some(packet_from_parts(
        "packet28.hook.windsurf.command.v1",
        "Bash",
        suite_packet_core::ToolOperationKind::Generic,
        Some("windsurf_native".to_string()),
        Some("shell".to_string()),
        summary,
        Some(command),
        None,
        paths,
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        payload.clone(),
        false,
    ))
}

pub(super) fn external_runtime_name(runtime: ExternalHookRuntime) -> &'static str {
    match runtime {
        ExternalHookRuntime::Copilot => "copilot",
        ExternalHookRuntime::Cursor => "cursor",
        ExternalHookRuntime::Gemini => "gemini",
        ExternalHookRuntime::Windsurf => "windsurf",
    }
}

pub(super) fn runtime_source(runtime: ExternalHookRuntime) -> &'static str {
    match runtime {
        ExternalHookRuntime::Copilot => "packet28.copilot.hook",
        ExternalHookRuntime::Cursor => "packet28.cursor.hook",
        ExternalHookRuntime::Gemini => "packet28.gemini.hook",
        ExternalHookRuntime::Windsurf => "packet28.windsurf.hook",
    }
}
