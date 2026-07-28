use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use packet28_daemon_core::storage::{
    load_task_events, load_task_events_from_offset, task_event_log_len,
};
use packet28_daemon_protocol::broker::{
    BrokerAction, BrokerPrepareHandoffRequest, BrokerResponseMode, BrokerTaskStatusRequest,
    BrokerTaskStatusResponse, BrokerValidatePlanRequest, BrokerWriteOp,
    BrokerWriteStateBatchRequest, BrokerWriteStateBatchResponse, BrokerWriteStateRequest,
};
use packet28_daemon_protocol::message::{DaemonRequest, DaemonResponse};
use packet28_daemon_protocol::paths::{
    task_artifact_dir, task_brief_markdown_path, task_state_json_path, task_version_json_path,
};
use packet28_daemon_protocol::task::TaskRecord;
use serde::Deserialize;
use serde_json::{json, Map, Value};

#[path = "cmd_mcp_config.rs"]
mod config;
#[path = "cmd_mcp_core_tools.rs"]
mod core_tools;
#[path = "cmd_mcp_fff.rs"]
mod fff;
#[path = "cmd_mcp_memory_tools.rs"]
mod memory_tools;
#[path = "cmd_mcp_native.rs"]
mod native_tools;
#[path = "cmd_mcp_prompt_resource.rs"]
mod prompt_resource;
#[path = "cmd_mcp_proxy.rs"]
mod proxy;
#[path = "cmd_mcp_proxy_catalog.rs"]
mod proxy_catalog;
#[path = "cmd_mcp_proxy_upstream.rs"]
mod proxy_upstream;
#[path = "cmd_mcp_response.rs"]
mod response;
#[path = "cmd_mcp_smoke.rs"]
mod smoke;
#[path = "cmd_mcp_support.rs"]
mod support;
#[path = "cmd_mcp_tool_args.rs"]
mod tool_args;
#[path = "cmd_mcp_tool_catalog.rs"]
mod tool_catalog;
#[path = "cmd_mcp_transport.rs"]
mod transport;

use crate::cmd_mcp::config::McpProxyConfig;
use crate::cmd_mcp::core_tools::handle_packet28_agent_status;
use crate::cmd_mcp::fff::FffMcpClient;
use crate::cmd_mcp::prompt_resource::{
    handle_prompt_get, handle_resource_read, handle_resources_list, prompt_descriptors,
    resolve_current_task_id,
};
use crate::cmd_mcp::proxy::{load_proxy_config, serve_proxy_stdio};
use crate::cmd_mcp::response::{shape_tool_response, summarize_tool_payload};
pub(crate) use crate::cmd_mcp::smoke::smoke_test_agent_config;
use crate::cmd_mcp::support::{
    broker_task_status_via_session, classify_error_message, extract_named_string, extract_paths,
    extract_symbols, is_retryable_error, maybe_store_result_artifact, resolve_session_task_id,
    store_tool_artifact, summarize_json_value, track_task,
};
use crate::cmd_mcp::tool_catalog::{canonical_tool_name, tools_list_payload};
use crate::cmd_mcp::transport::{read_message, write_message, McpMessageFraming};

const MCP_PROTOCOL_VERSION_2024_11_05: &str = "2024-11-05";
const MCP_PROTOCOL_VERSION_2025_03_26: &str = "2025-03-26";
const MCP_LATEST_PROTOCOL_VERSION: &str = MCP_PROTOCOL_VERSION_2025_03_26;
const MCP_NOTIFICATION_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Args)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: McpCommands,
}

#[derive(Subcommand)]
pub enum McpCommands {
    /// Serve Packet28 as an MCP stdio server
    Serve(McpServeArgs),
    /// Proxy one or more upstream MCP servers and auto-capture tool activity
    Proxy(McpProxyArgs),
    /// Validate an MCP server entry from an agent config
    SmokeTest(McpSmokeTestArgs),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum McpToolset {
    /// Small default catalog for search/read/fetch/handoff loops.
    #[default]
    Core,
    /// Full compatibility catalog with memory, graph, feedback, diagnostics, and legacy aliases.
    All,
}

#[derive(Args, Clone)]
pub struct McpServeArgs {
    #[arg(long, default_value = ".")]
    pub root: String,

    /// Tool catalog exposed by tools/list. Tools remain callable by name in core mode.
    #[arg(long, value_enum, default_value_t = McpToolset::Core)]
    pub toolset: McpToolset,
}

#[derive(Args, Clone)]
pub struct McpProxyArgs {
    #[arg(long, default_value = ".")]
    pub root: String,

    #[arg(long, default_value = ".mcp.json")]
    pub upstream_config: String,

    #[arg(long)]
    pub task_id: Option<String>,
}

#[derive(Args, Clone)]
pub struct McpSmokeTestArgs {
    /// Agent config to load (currently: windsurf)
    #[arg(long = "from-config")]
    pub from_config: String,
}

#[derive(Default)]
struct McpSessionState {
    initialized: bool,
    shutdown: bool,
    toolset: McpToolset,
    tracked_tasks: BTreeMap<String, u64>,
    tracked_task_offsets: BTreeMap<String, u64>,
    current_task_id: Option<String>,
    framing: Option<McpMessageFraming>,
    tool_owners: BTreeMap<String, String>,
    tool_forward_names: BTreeMap<String, String>,
    upstream_tools_cache: Vec<Value>,
    upstream_tools_loaded: bool,
    resource_owners: BTreeMap<String, String>,
    upstream_resources_cache: Vec<Value>,
    upstream_resources_loaded: bool,
    upstream_resource_templates_cache: Vec<Value>,
    upstream_resource_templates_loaded: bool,
    proxy_task_id: Option<String>,
    next_invocation_seq: u64,
    fff_client: Option<FffMcpClient>,
    #[cfg(unix)]
    daemon_client: Option<crate::cmd_daemon::PersistentDaemonClient>,
}

pub fn run(args: McpArgs) -> Result<i32> {
    match args.command {
        McpCommands::Serve(args) => run_serve(args),
        McpCommands::Proxy(args) => run_proxy(args),
        McpCommands::SmokeTest(args) => run_smoke_test(args),
    }
}

fn run_serve(args: McpServeArgs) -> Result<i32> {
    let root = crate::broker_client::resolve_root(&args.root);
    crate::broker_client::ensure_daemon(&root)?;
    serve_stdio(root, args.toolset)?;
    Ok(0)
}

fn run_proxy(args: McpProxyArgs) -> Result<i32> {
    let root = crate::broker_client::resolve_root(&args.root);
    crate::broker_client::ensure_daemon(&root)?;
    let config_path = crate::cmd_common::resolve_path_from_cwd(
        &args.upstream_config,
        &crate::cmd_common::caller_cwd()?,
    );
    let config = load_proxy_config(Path::new(&config_path))?;
    serve_proxy_stdio(
        root,
        config,
        args.task_id
            .unwrap_or_else(|| crate::broker_client::derive_task_id("packet28-mcp-proxy-session")),
    )?;
    Ok(0)
}

fn run_smoke_test(args: McpSmokeTestArgs) -> Result<i32> {
    let report = smoke_test_agent_config(&args.from_config)?;
    println!(
        "MCP smoke test ok: server={} tools={}",
        report.server_name, report.tool_count
    );
    Ok(0)
}

fn serve_stdio(root: PathBuf, toolset: McpToolset) -> Result<()> {
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let writer = Arc::new(Mutex::new(io::stdout()));
    let session = Arc::new(Mutex::new(McpSessionState {
        toolset,
        ..McpSessionState::default()
    }));
    start_notification_thread(root.clone(), writer.clone(), session.clone());

    loop {
        let Some((request, framing)) = read_message(&mut reader)? else {
            break;
        };
        if let Ok(mut guard) = session.lock() {
            guard.framing = Some(framing);
        }
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let id = request.get("id").cloned();
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            if let Some(id) = id {
                let response = mcp_error_response(id, -32600, "missing method");
                let mut guard = writer
                    .lock()
                    .map_err(|_| anyhow!("failed to lock MCP stdout"))?;
                write_message(&mut *guard, &response, framing)?;
            }
            continue;
        };

        if id.is_none() {
            let _ = handle_notification(&root, &session, method, params);
            continue;
        }

        let response = match handle_method(&root, &session, method, params) {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(err) => mcp_error_response(
                id.unwrap_or(Value::Null),
                mcp_error_code(&err),
                &err.to_string(),
            ),
        };
        let mut guard = writer
            .lock()
            .map_err(|_| anyhow!("failed to lock MCP stdout"))?;
        write_message(&mut *guard, &response, framing)?;
    }

    if let Ok(mut guard) = session.lock() {
        guard.shutdown = true;
    }
    Ok(())
}

fn start_notification_thread(
    root: PathBuf,
    writer: Arc<Mutex<io::Stdout>>,
    session: Arc<Mutex<McpSessionState>>,
) {
    thread::spawn(move || loop {
        let (initialized, shutdown, tracked_tasks, tracked_task_offsets, framing) =
            match session.lock() {
                Ok(guard) => (
                    guard.initialized,
                    guard.shutdown,
                    guard.tracked_tasks.clone(),
                    guard.tracked_task_offsets.clone(),
                    guard.framing,
                ),
                Err(_) => return,
            };
        if shutdown {
            return;
        }
        if !initialized || framing.is_none() {
            thread::sleep(MCP_NOTIFICATION_POLL_INTERVAL);
            continue;
        }
        let framing = framing.unwrap_or(McpMessageFraming::ContentLength);

        for (task_id, last_seen_seq) in tracked_tasks {
            let previous_offset = tracked_task_offsets.get(&task_id).copied().unwrap_or(0);
            let read = match load_task_events_from_offset(&root, &task_id, previous_offset) {
                Ok(read) => read,
                Err(_) => continue,
            };
            let mut newest_delivered_seq = last_seen_seq;
            for frame in read
                .events
                .into_iter()
                .filter(|frame| frame.seq > last_seen_seq)
            {
                if frame.event.kind != "context_updated" {
                    newest_delivered_seq = newest_delivered_seq.max(frame.seq);
                    continue;
                }
                let mut params = match frame.event.data {
                    Value::Object(map) => map,
                    other => {
                        let mut map = Map::new();
                        map.insert("data".to_string(), other);
                        map
                    }
                };
                params.insert("task_id".to_string(), Value::String(task_id.clone()));
                params.insert(
                    "context_version".to_string(),
                    params
                        .get("context_version")
                        .cloned()
                        .unwrap_or(Value::Null),
                );
                params.insert("event_seq".to_string(), Value::Number(frame.seq.into()));
                let notification = json!({
                    "jsonrpc":"2.0",
                    "method":"notifications/packet28.context_updated",
                    "params": Value::Object(params),
                });
                let write_ok = if let Ok(mut guard) = writer.lock() {
                    write_message(&mut *guard, &notification, framing).is_ok()
                } else {
                    false
                };
                if !write_ok {
                    if let Ok(mut guard) = session.lock() {
                        guard.shutdown = true;
                    }
                    return;
                }
                newest_delivered_seq = newest_delivered_seq.max(frame.seq);
            }
            if newest_delivered_seq > last_seen_seq || read.next_offset != previous_offset {
                if let Ok(mut guard) = session.lock() {
                    if let Some(current) = guard.tracked_tasks.get_mut(&task_id) {
                        *current = newest_delivered_seq;
                    }
                    guard
                        .tracked_task_offsets
                        .insert(task_id.clone(), read.next_offset);
                }
            }
        }
        thread::sleep(MCP_NOTIFICATION_POLL_INTERVAL);
    });
}

fn handle_notification(
    _root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    method: &str,
    _params: Value,
) -> Result<()> {
    if method == "notifications/initialized" {
        let mut guard = session
            .lock()
            .map_err(|_| anyhow!("failed to lock MCP session"))?;
        guard.initialized = true;
        return Ok(());
    }
    Ok(())
}

fn handle_method(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    method: &str,
    params: Value,
) -> Result<Value> {
    match method {
        "initialize" => {
            if let Ok(mut guard) = session.lock() {
                guard.initialized = true;
                for (task_id, last_seen_seq) in guard.tracked_tasks.clone() {
                    let latest_seq = load_task_events(root, &task_id)
                        .ok()
                        .and_then(|frames| frames.last().map(|frame| frame.seq))
                        .unwrap_or(last_seen_seq);
                    let offset = task_event_log_len(root, &task_id).unwrap_or(0);
                    guard.tracked_tasks.insert(task_id.clone(), latest_seq);
                    guard.tracked_task_offsets.insert(task_id, offset);
                }
            }
            Ok(json!({
                "protocolVersion": negotiated_protocol_version(&params),
                "capabilities": {
                    "tools": {},
                    "resources": {},
                    "prompts": {}
                },
                "serverInfo": {
                    "name": "Packet28",
                    "version": env!("PACKET28_VERSION")
                }
            }))
        }
        "tools/list" => {
            let toolset = session
                .lock()
                .map(|guard| guard.toolset)
                .unwrap_or_default();
            Ok(tools_list_payload(toolset))
        }
        "prompts/list" => Ok(json!({
            "prompts": prompt_descriptors(),
        })),
        "prompts/get" => handle_prompt_get(root, session, params),
        "tools/call" => handle_tool_call(root, session, params),
        "resources/list" => handle_resources_list(root, session),
        "resources/templates/list" => Ok(json!({
            "resourceTemplates": [
                {
                    "uriTemplate": "packet28://task/{task_id}/brief",
                    "name": "Packet28 task brief",
                    "description": "Latest brokered brief for a task."
                },
                {
                    "uriTemplate": "packet28://task/{task_id}/events",
                    "name": "Packet28 task events",
                    "description": "Event stream replay for a task."
                },
                {
                    "uriTemplate": "packet28://task/{task_id}/state",
                    "name": "Packet28 task state",
                    "description": "Current task state metadata for a task."
                },
                {
                    "uriTemplate": "packet28://current/{artifact}",
                    "name": "Packet28 current task artifact",
                    "description": "Current task aliases for task, brief, events, and state."
                }
            ]
        })),
        "resources/read" => handle_resource_read(root, session, params),
        _ => Err(anyhow!("unsupported MCP method '{method}'")),
    }
}

fn negotiated_protocol_version(params: &Value) -> &'static str {
    match params.get("protocolVersion").and_then(Value::as_str) {
        Some(MCP_PROTOCOL_VERSION_2024_11_05) => MCP_PROTOCOL_VERSION_2024_11_05,
        Some(MCP_PROTOCOL_VERSION_2025_03_26) => MCP_PROTOCOL_VERSION_2025_03_26,
        _ => MCP_LATEST_PROTOCOL_VERSION,
    }
}

fn mcp_error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc":"2.0",
        "id":id,
        "error":{
            "code":code,
            "message":message
        }
    })
}

fn mcp_error_code(err: &anyhow::Error) -> i64 {
    let message = err.to_string();
    if message.starts_with("unsupported MCP method") {
        -32601
    } else if message.contains("missing ")
        || message.contains("invalid ")
        || message.contains("expected ")
        || message.contains("failed to parse")
    {
        -32602
    } else {
        -32603
    }
}

fn handle_tool_call(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    params: Value,
) -> Result<Value> {
    let requested_name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing tool name"))?;
    let canonical_name = canonical_tool_name(requested_name);
    let name = canonical_name.as_str();
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    if let Some(native_response) = native_tools::handle_tool_call(root, session, name, &arguments)?
    {
        return Ok(native_response);
    }
    let payload = if let Some(memory_payload) =
        memory_tools::handle_memory_tool_call(root, name, &arguments)?
    {
        memory_payload
    } else if let Some(core_payload) =
        core_tools::handle_core_tool_call(root, session, name, &arguments)?
    {
        core_payload
    } else {
        return Err(anyhow!("unsupported tool '{name}'"));
    };
    let summary = summarize_tool_payload(name, &payload);
    Ok(shape_tool_response(payload, summary))
}

#[cfg(test)]
mod tests;
