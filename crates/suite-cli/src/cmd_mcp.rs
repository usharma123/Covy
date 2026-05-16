use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use packet28_daemon_core::{
    load_task_events, task_artifact_dir, task_brief_markdown_path, task_state_json_path,
    task_version_json_path, BrokerAction, BrokerPlanStep, BrokerPrepareHandoffRequest,
    BrokerResponseMode, BrokerTaskStatusRequest, BrokerTaskStatusResponse,
    BrokerValidatePlanRequest, BrokerWriteOp, BrokerWriteStateBatchRequest,
    BrokerWriteStateBatchResponse, BrokerWriteStateRequest, BrokerWriteStateResponse,
    DaemonRequest, DaemonResponse, TaskRecord,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};

#[allow(dead_code)]
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
#[allow(dead_code)]
#[path = "cmd_mcp_support.rs"]
mod support;
#[path = "cmd_mcp_transport.rs"]
mod transport;

use crate::cmd_mcp::native_tools::{
    handle_packet28_fetch_context, handle_packet28_fetch_raw_output,
    handle_packet28_fetch_tool_result, handle_packet28_glob, handle_packet28_prepare_handoff,
    handle_packet28_read_regions, handle_packet28_search, handle_packet28_search_fast,
    handle_packet28_validate_plan, handle_packet28_write_intention, Packet28ActionCriticArgs,
    Packet28FetchContextArgs, Packet28FetchRawOutputArgs, Packet28FetchToolResultArgs,
    Packet28GlobArgs, Packet28HandoffCompressionArgs, Packet28HandoffDiffArgs,
    Packet28PatchRiskArgs, Packet28PrepareHandoffArgs, Packet28PromptPressureArgs,
    Packet28ReadRegionsArgs, Packet28RecommendNextToolArgs, Packet28SearchArgs,
    Packet28SearchFastArgs, Packet28ValidatePlanArgs, Packet28ValidateToolOutcomeArgs,
    Packet28VerifyHandoffArgs, Packet28WriteIntentionArgs,
};
use crate::cmd_mcp::prompt_resource::{
    handle_prompt_get, handle_resource_read, handle_resources_list, prompt_descriptors,
    resolve_current_task_id,
};
use crate::cmd_mcp::proxy::{load_proxy_config, serve_proxy_stdio};
use crate::cmd_mcp::support::{
    broker_task_status_via_session, classify_error_message, extract_named_string, extract_paths,
    extract_symbols, is_retryable_error, maybe_store_result_artifact, resolve_session_task_id,
    store_tool_artifact, summarize_json_value, track_task,
};
use crate::cmd_mcp::transport::{
    read_message, render_command_preview, write_message, McpMessageFraming,
};
use crate::cmd_transcript::{export_transcripts, import_transcripts_from_str};
use crate::cmd_wakeup::{build_wakeup_report_scoped, WakeupScope};
use crate::memory_store::{
    add_concept_with_metadata, append_transcript_message, apply_feedback, consolidate_memories,
    create_graph_memoir, decay_memories, delete_concept, delete_feedback,
    delete_pending_extractions, distill_memories_to_graph, embed_memories,
    enqueue_pending_extraction, export_graph, extract_memory_patterns, feedback_stats,
    forget_memories_by_topic, forget_memory, graph_stats, inspect_graph, inspect_graph_concept,
    learn_project_graph, link_concepts, list_feedback, list_graph_memoirs, list_memories_filtered,
    list_pending_extractions, list_transcript_sessions, local_store_stats, memory_health,
    memory_topics, process_pending_extractions, prune_memories, recall_memories_filtered,
    record_feedback_with_metadata, refine_concept, search_concepts_filtered,
    search_feedback_filtered, search_transcripts_filtered, show_graph_memoir,
    show_transcript_session, store_memory_with_metadata, transcript_stats, update_memory,
    FeedbackInput, MemoryListQuery, MemoryRecallQuery, MemoryStoreInput, MemoryUpdateInput,
    PendingExtractionInput, TranscriptAppendInput,
};
use crate::route_registry::{
    build_route_rewrite, decide_command_route_with_cwd_and_root, NativeToolKind, RouteKind,
};
use crate::runtime_integrations::windsurf;

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

#[derive(Args, Clone)]
pub struct McpServeArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
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
    tracked_tasks: BTreeMap<String, u64>,
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
    #[cfg(unix)]
    daemon_client: Option<crate::cmd_daemon::PersistentDaemonClient>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
#[derive(Default)]
struct McpProxyConfig {
    #[serde(rename = "mcpServers")]
    mcp_servers: BTreeMap<String, McpProxyServerConfig>,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
#[serde(default)]
struct McpProxyServerConfig {
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: Option<String>,
    timeout_ms: Option<u64>,
    compact_tools: Vec<String>,
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
    serve_stdio(root)?;
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

pub(crate) struct McpSmokeReport {
    pub(crate) server_name: String,
    pub(crate) tool_count: usize,
}

pub(crate) fn smoke_test_agent_config(agent: &str) -> Result<McpSmokeReport> {
    let server = load_agent_mcp_server(agent)?;
    smoke_test_mcp_server(&server)
}

fn load_agent_mcp_server(agent: &str) -> Result<McpProxyServerConfig> {
    match agent {
        "windsurf" => load_named_mcp_server(&windsurf_mcp_config_path(), "packet28"),
        other => Err(anyhow!(
            "unsupported MCP config '{other}'; supported values: windsurf"
        )),
    }
}

fn windsurf_mcp_config_path() -> PathBuf {
    windsurf::mcp_config_path(&dirs_home())
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn load_named_mcp_server(path: &Path, name: &str) -> Result<McpProxyServerConfig> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read '{}'", path.display()))?;
    let config: McpProxyConfig = serde_json::from_str(&content)
        .with_context(|| format!("invalid MCP config '{}'", path.display()))?;
    config
        .mcp_servers
        .get(name)
        .cloned()
        .with_context(|| format!("MCP server '{name}' missing from '{}'", path.display()))
}

fn smoke_test_mcp_server(server: &McpProxyServerConfig) -> Result<McpSmokeReport> {
    if server.command.trim().is_empty() {
        return Err(anyhow!("MCP server command is empty"));
    }
    let mut harness = ConfiguredMcpHarness::start(server)?;
    harness.send(&json!({
        "jsonrpc":"2.0",
        "id":1,
        "method":"initialize",
        "params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"packet28-smoke-test","version":"1"}}
    }))?;
    let initialize = harness.read_response(1)?;
    let server_name = initialize["result"]["serverInfo"]["name"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    harness.send(&json!({
        "jsonrpc":"2.0",
        "id":2,
        "method":"tools/list"
    }))?;
    let tools = harness.read_response(2)?;
    let tool_names = tools["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>();
    if !tool_names.contains(&"packet28_search") {
        return Err(anyhow!("packet28_search missing from tools/list"));
    }
    Ok(McpSmokeReport {
        server_name,
        tool_count: tool_names.len(),
    })
}

struct ConfiguredMcpHarness {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ConfiguredMcpHarness {
    fn start(server: &McpProxyServerConfig) -> Result<Self> {
        let mut command = Command::new(&server.command);
        command.args(&server.args);
        if let Some(cwd) = server.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
            command.current_dir(cwd);
        }
        for (key, value) in &server.env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to start configured MCP server `{}`",
                    render_command_preview(&server.command, &server.args)
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to capture MCP stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture MCP stdout"))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    fn send(&mut self, value: &Value) -> Result<()> {
        write_message(&mut self.stdin, value, McpMessageFraming::ContentLength)
    }

    fn read_response(&mut self, expected_id: u64) -> Result<Value> {
        let started = Instant::now();
        loop {
            if started.elapsed() > Duration::from_secs(10) {
                return Err(anyhow!(
                    "timed out waiting for MCP response id={expected_id}"
                ));
            }
            let Some((value, _)) = read_message(&mut self.stdout)? else {
                return Err(anyhow!(
                    "MCP stream closed before response id={expected_id}"
                ));
            };
            if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
                if let Some(error) = value.get("error") {
                    return Err(anyhow!(
                        "MCP response id={expected_id} returned error: {error}"
                    ));
                }
                return Ok(value);
            }
        }
    }
}

impl Drop for ConfiguredMcpHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn serve_stdio(root: PathBuf) -> Result<()> {
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let writer = Arc::new(Mutex::new(io::stdout()));
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    start_notification_thread(root.clone(), writer.clone(), session.clone());

    loop {
        let Some((request, framing)) = read_message(&mut reader)? else {
            break;
        };
        if let Ok(mut guard) = session.lock() {
            guard.framing = Some(framing);
        }
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing method"))?;
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let id = request.get("id").cloned();

        if id.is_none() {
            let _ = handle_notification(&root, &session, method, params);
            continue;
        }

        let response = match handle_method(&root, &session, method, params) {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(err) => json!({
                "jsonrpc":"2.0",
                "id":id,
                "error":{
                    "code":-32000,
                    "message":err.to_string()
                }
            }),
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
        let (initialized, shutdown, tracked_tasks, framing) = match session.lock() {
            Ok(guard) => (
                guard.initialized,
                guard.shutdown,
                guard.tracked_tasks.clone(),
                guard.framing,
            ),
            Err(_) => return,
        };
        if shutdown {
            return;
        }
        if !initialized || framing.is_none() {
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        let framing = framing.unwrap_or(McpMessageFraming::ContentLength);

        for (task_id, last_seen_seq) in tracked_tasks {
            let frames = match load_task_events(&root, &task_id) {
                Ok(frames) => frames,
                Err(_) => continue,
            };
            let mut newest_delivered_seq = last_seen_seq;
            for frame in frames.into_iter().filter(|frame| frame.seq > last_seen_seq) {
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
            if newest_delivered_seq > last_seen_seq {
                if let Ok(mut guard) = session.lock() {
                    if let Some(current) = guard.tracked_tasks.get_mut(&task_id) {
                        *current = newest_delivered_seq;
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(250));
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

fn cursor_safe_tool_name(name: &str) -> String {
    name.strip_prefix("packet28.")
        .map(|suffix| format!("packet28_{suffix}"))
        .unwrap_or_else(|| name.to_string())
}

fn canonical_tool_name(name: &str) -> String {
    name.strip_prefix("packet28_")
        .map(|suffix| format!("packet28.{suffix}"))
        .unwrap_or_else(|| name.to_string())
}

fn rewrite_tool_names_for_cursor(payload: &mut Value) {
    let Some(tools) = payload.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };

    for tool in tools {
        if let Some(name) = tool.get("name").and_then(Value::as_str) {
            let safe_name = cursor_safe_tool_name(name);
            tool["name"] = Value::String(safe_name);
        }
    }
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
                    guard.tracked_tasks.insert(task_id, latest_seq);
                }
            }
            Ok(json!({
                "protocolVersion": "2024-11-05",
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
            let mut payload = json!({
            "tools": [
                {
                    "name": "packet28.search",
                    "description": "Run compact code/text search and return a slim preview plus a fetchable full artifact.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["query"],
                        "properties": {
                            "task_id": {"type":"string"},
                            "query": {"type":"string"},
                            "paths": {"type":"array","items":{"type":"string"}},
                            "fixed_string": {"type":"boolean"},
                            "case_sensitive": {"type":"boolean"},
                            "whole_word": {"type":"boolean"},
                            "context_lines": {"type":"integer","minimum":0},
                            "max_matches_per_file": {"type":"integer","minimum":1},
                            "max_total_matches": {"type":"integer","minimum":1},
                            "search_strategy": {"type":"string","enum":["hybrid","recall","indexed","native"]},
                            "response_mode": {"type":"string","enum":["slim","full"]}
                        }
                    }
                },
                {
                    "name": "packet28.search_fast",
                    "description": "Run compact code/text search over the persistent daemon socket without storing artifacts or broker state.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["query"],
                        "properties": {
                            "query": {"type":"string"},
                            "paths": {"type":"array","items":{"type":"string"}},
                            "fixed_string": {"type":"boolean"},
                            "case_sensitive": {"type":"boolean"},
                            "whole_word": {"type":"boolean"},
                            "context_lines": {"type":"integer","minimum":0},
                            "max_matches_per_file": {"type":"integer","minimum":1},
                            "max_total_matches": {"type":"integer","minimum":1},
                            "search_strategy": {"type":"string","enum":["hybrid","recall","indexed","native"]},
                            "response_mode": {"type":"string","enum":["slim","full"]}
                        }
                    }
                },
                {
                    "name": "packet28.read_regions",
                    "description": "Read targeted file regions and return a slim preview plus a fetchable full artifact.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["path"],
                        "properties": {
                            "task_id": {"type":"string"},
                            "path": {"type":"string"},
                            "regions": {"type":"array","items":{"type":"string"}},
                            "line_start": {"type":"integer","minimum":1},
                            "line_end": {"type":"integer","minimum":1},
                            "response_mode": {"type":"string","enum":["slim","full"]}
                        }
                    }
                },
                {
                    "name": "packet28.glob",
                    "description": "Resolve a glob pattern into compact path matches with a fetchable full artifact.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["pattern"],
                        "properties": {
                            "task_id": {"type":"string"},
                            "pattern": {"type":"string"},
                            "paths": {"type":"array","items":{"type":"string"}},
                            "max_results": {"type":"integer","minimum":1},
                            "response_mode": {"type":"string","enum":["slim","full"]}
                        }
                    }
                },
                {
                    "name": "packet28.fetch_tool_result",
                    "description": "Fetch a previously stored full artifact for packet28.search, packet28.read_regions, packet28.glob, or hook-captured tool output.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "artifact_id": {"type":"string"},
                            "invocation_id": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.fetch_raw_output",
                    "description": "Fetch raw output from a hook spool file or other Packet28 raw artifact handle.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "handle": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.fetch_context",
                    "description": "Fetch a stored Packet28 broker context by context_version or artifact_id. Use response_mode='slim' to omit heavy sections.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "artifact_id": {"type":"string"},
                            "context_version": {"type":"string"},
                            "response_mode": {"type":"string","enum":["slim","full"]}
                        }
                    }
                },
                {
                    "name": "packet28.verify_handoff",
                    "description": "Verify a stored Packet28 handoff context artifact has enough objective, next-action, debt, and evidence signal for replay.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "artifact_id": {"type":"string"},
                            "context_version": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.prompt_pressure",
                    "description": "Estimate whether a stored handoff context plus the next worker prompt will fit a target token budget and identify the largest removable sections.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "artifact_id": {"type":"string"},
                            "context_version": {"type":"string"},
                            "next_prompt": {"type":"string"},
                            "budget_tokens": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.handoff_diff",
                    "description": "Compare two stored Packet28 handoff context artifacts and report compact objective, next-action, evidence, and debt-signal deltas.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "left_artifact_id": {"type":"string"},
                            "left_context_version": {"type":"string"},
                            "right_artifact_id": {"type":"string"},
                            "right_context_version": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.handoff_compress",
                    "description": "Recommend non-critical handoff sections to remove so an over-budget replay packet can fit while preserving objective, next-action, debt, and evidence signals.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "artifact_id": {"type":"string"},
                            "context_version": {"type":"string"},
                            "next_prompt": {"type":"string"},
                            "budget_tokens": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.prepare_handoff",
                    "description": "Prepare a compact Packet28 handoff packet for bootstrapping a fresh worker after a checkpoint.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "query": {"type":"string"},
                            "response_mode": {"type":"string","enum":["slim","full"]}
                        }
                    }
                },
                {
                    "name": "packet28.handoff",
                    "description": "Compatibility alias for packet28.prepare_handoff.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "query": {"type":"string"},
                            "response_mode": {"type":"string","enum":["slim","full"]}
                        }
                    }
                },
                {
                    "name": "packet28.validate_plan",
                    "description": "Validate an agent implementation plan against Packet28 broker state, coverage, dependency order, read-before-edit, and mapped test-gate evidence.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["task_id", "steps"],
                        "properties": {
                            "task_id": {"type":"string"},
                            "steps": {
                                "type":"array",
                                "items": {
                                    "type":"object",
                                    "properties": {
                                        "id": {"type":"string"},
                                        "action": {"type":"string"},
                                        "description": {"type":"string"},
                                        "paths": {"type":"array","items":{"type":"string"}},
                                        "symbols": {"type":"array","items":{"type":"string"}},
                                        "depends_on": {"type":"array","items":{"type":"string"}}
                                    }
                                }
                            },
                            "require_read_before_edit": {"type":"boolean"},
                            "require_test_gate": {"type":"boolean"},
                            "budget_tokens": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.action_critic",
                    "description": "Return focused Packet28 action-critic warnings before choosing a tool or editing focused files.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["task_id", "action"],
                        "properties": {
                            "task_id": {"type":"string"},
                            "action": {"type":"string","enum":["choose_tool","edit"]},
                            "query": {"type":"string"},
                            "tool_name": {"type":"string"},
                            "focus_paths": {"type":"array","items":{"type":"string"}},
                            "focus_symbols": {"type":"array","items":{"type":"string"}},
                            "budget_tokens": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.recommend_next_tool",
                    "description": "Recommend one or two next Packet28 commands using local ROI, failure-advice, and focused freshness signals.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "query": {"type":"string"},
                            "focus_paths": {"type":"array","items":{"type":"string"}},
                            "focus_symbols": {"type":"array","items":{"type":"string"}},
                            "max_recommendations": {"type":"integer","minimum":1,"maximum":4}
                        }
                    }
                },
                {
                    "name": "packet28.validate_tool_outcome",
                    "description": "Classify the latest matching Packet28 tool outcome as success, fallback, missing artifact, stale artifact, or failure.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "command": {"type":"string"},
                            "focus_paths": {"type":"array","items":{"type":"string"}}
                        }
                    }
                },
                {
                    "name": "packet28.patch_risk",
                    "description": "Score pre-edit patch risk from path scope, cached testmap mappings, and recent failure/fallback records.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"},
                            "paths": {"type":"array","items":{"type":"string"}}
                        }
                    }
                },
                {
                    "name": "packet28.verify_experiments",
                    "description": "Verify Packet28 experiment manifest evidence, artifacts, metric gates, runtime versions, and required workflow coverage.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "manifest": {"type":"string"},
                            "require_workflows": {"type":"array","items":{"type":"string"}}
                        }
                    }
                },
                {
                    "name": "packet28.hypothesis_add",
                    "description": "Record an active task hypothesis so it flows into Packet28 broker context and handoff state.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["text"],
                        "properties": {
                            "task_id": {"type":"string"},
                            "id": {"type":"string"},
                            "text": {"type":"string"},
                            "paths": {"type":"array","items":{"type":"string"}},
                            "symbols": {"type":"array","items":{"type":"string"}},
                            "artifact_id": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.hypothesis_list",
                    "description": "List active task hypotheses from the Packet28 broker snapshot.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.hypothesis_resolve",
                    "description": "Confirm or reject an active Packet28 task hypothesis.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["id", "status"],
                        "properties": {
                            "task_id": {"type":"string"},
                            "id": {"type":"string"},
                            "status": {"type":"string","enum":["confirmed","rejected"]},
                            "note": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.reduce",
                    "description": "Reduce command stdout/stderr into a compact Packet28 packet without executing the command.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["command"],
                        "properties": {
                            "command": {"type":"string"},
                            "stdout": {"type":"string"},
                            "stderr": {"type":"string"},
                            "exit_code": {"type":"integer"}
                        }
                    }
                },
                {
                    "name": "packet28.rewrite",
                    "description": "Plan the Packet28 reducer/native-tool/proxy rewrite for a shell command.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["command"],
                        "properties": {
                            "command": {"type":"string"},
                            "task_id": {"type":"string"},
                            "session_id": {"type":"string"},
                            "cwd": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.doctor",
                    "description": "Run Packet28 doctor and return its JSON health report.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "agent": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_store",
                    "description": "Store a local Packet28 memory in ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["content"],
                        "properties": {
                            "content": {"type":"string"},
                            "tags": {"type":"string"},
                            "topic": {"type":"string"},
                            "importance": {"type":"string"},
                            "keywords": {"type":"string"},
                            "project": {"type":"string"},
                            "source": {"type":"string"},
                            "raw_excerpt": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_recall",
                    "description": "Recall local Packet28 memories from ~/.packet28/packet28.db using keyword search.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["query"],
                        "properties": {
                            "query": {"type":"string"},
                            "limit": {"type":"integer","minimum":1},
                            "topic": {"type":"string"},
                            "project": {"type":"string"},
                            "tag": {"type":"string"},
                            "keyword": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_list",
                    "description": "List recent local Packet28 memories from ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1},
                            "topic": {"type":"string"},
                            "project": {"type":"string"},
                            "all": {"type":"boolean"},
                            "sort": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_update",
                    "description": "Update a local Packet28 memory by id.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["id"],
                        "properties": {
                            "id": {"type":"integer"},
                            "content": {"type":"string"},
                            "tags": {"type":"string"},
                            "topic": {"type":"string"},
                            "importance": {"type":"string"},
                            "keywords": {"type":"string"},
                            "project": {"type":"string"},
                            "source": {"type":"string"},
                            "raw_excerpt": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_forget",
                    "description": "Delete a local Packet28 memory by id, or delete memories in a topic.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {"type":"integer"},
                            "topic": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_topics",
                    "description": "List local Packet28 memory topics and counts.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.memory_stats",
                    "description": "Return local Packet28 memory and store statistics.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.memory_health",
                    "description": "Return local Packet28 memory topic health, staleness, and consolidation signals.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "topic": {"type":"string"},
                            "stale_after_days": {"type":"integer","minimum":0},
                            "consolidation_threshold": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.memory_consolidate",
                    "description": "Consolidate local Packet28 memories for a topic into one deterministic summary memory.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "topic": {"type":"string"},
                            "keep_originals": {"type":"boolean"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_decay",
                    "description": "Apply local weight decay to non-critical Packet28 memories.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "factor": {"type":"number","minimum":0,"maximum":1}
                        }
                    }
                },
                {
                    "name": "packet28.memory_prune",
                    "description": "Delete low-weight non-critical Packet28 memories, or preview candidates with dry_run.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "threshold": {"type":"number","minimum":0,"maximum":1},
                            "dry_run": {"type":"boolean"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_embed",
                    "description": "Create local deterministic memory embeddings in ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {"type":"integer"},
                            "all": {"type":"boolean"},
                            "dimensions": {"type":"integer","minimum":8}
                        }
                    }
                },
                {
                    "name": "packet28.memory_extract_patterns",
                    "description": "Detect recurring local memory patterns in a topic and optionally create graph concepts.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["topic"],
                        "properties": {
                            "topic": {"type":"string"},
                            "memoir": {"type":"string"},
                            "min_cluster_size": {"type":"integer","minimum":2}
                        }
                    }
                },
                {
                    "name": "packet28.memory_pending_enqueue",
                    "description": "Queue raw local tool/session text for later Packet28 memory extraction.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["raw_output"],
                        "properties": {
                            "raw_output": {"type":"string"},
                            "project": {"type":"string"},
                            "tool_name": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_pending_list",
                    "description": "List queued Packet28 pending memory extractions.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.memory_pending_process",
                    "description": "Process queued Packet28 pending extractions into durable local memories.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1},
                            "dry_run": {"type":"boolean"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_pending_delete",
                    "description": "Delete queued Packet28 pending extraction rows by id.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["ids"],
                        "properties": {
                            "ids": {"type":"array","items":{"type":"integer"}}
                        }
                    }
                },
                {
                    "name": "packet28.memory_pending_stats",
                    "description": "Return queued Packet28 pending extraction counts.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.feedback_record",
                    "description": "Record a local feedback correction in ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["subject", "correction"],
                        "properties": {
                            "subject": {"type":"string"},
                            "correction": {"type":"string"},
                            "topic": {"type":"string"},
                            "context": {"type":"string"},
                            "predicted": {"type":"string"},
                            "reason": {"type":"string"},
                            "source": {"type":"string"},
                            "project": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.feedback_search",
                    "description": "Search local feedback corrections in ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["query"],
                        "properties": {
                            "query": {"type":"string"},
                            "memoir": {"type":"string"},
                            "label": {"type":"string"},
                            "project": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.feedback_list",
                    "description": "List local feedback corrections from ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "topic": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.feedback_apply",
                    "description": "Increment the applied count for a feedback correction.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["id"],
                        "properties": {
                            "id": {"type":"integer"}
                        }
                    }
                },
                {
                    "name": "packet28.feedback_delete",
                    "description": "Delete a local feedback correction by id.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["id"],
                        "properties": {
                            "id": {"type":"integer"}
                        }
                    }
                },
                {
                    "name": "packet28.feedback_stats",
                    "description": "Return local feedback correction statistics.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.wakeup",
                    "description": "Build a compact local wake-up pack from memory, feedback, transcripts, graph, and stats.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {"type":"string"},
                            "project": {"type":"string"},
                            "paths": {"type":"array","items":{"type":"string"}},
                            "symbols": {"type":"array","items":{"type":"string"}},
                            "intent": {"type":"string"},
                            "limit": {"type":"integer","minimum":1},
                            "max_tokens": {"type":"integer","minimum":1},
                            "format": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.learn_project",
                    "description": "Scan a local project into Packet28 graph concepts and relations.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "directory": {"type":"string"},
                            "name": {"type":"string"},
                            "memoir": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_append",
                    "description": "Append a local transcript message to ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["content"],
                        "properties": {
                            "content": {"type":"string"},
                            "session": {"type":"string"},
                            "agent": {"type":"string"},
                            "role": {"type":"string"},
                            "source": {"type":"string"},
                            "project": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_list",
                    "description": "List local transcript sessions.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_show",
                    "description": "Show local transcript messages for a session.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["session"],
                        "properties": {
                            "session": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_search",
                    "description": "Search local transcript messages with FTS and LIKE fallback.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["query"],
                        "properties": {
                            "query": {"type":"string"},
                            "memoir": {"type":"string"},
                            "label": {"type":"string"},
                            "project": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_stats",
                    "description": "Return local transcript session and message statistics.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.transcript_export",
                    "description": "Export local transcript messages as Packet28 transcript JSON.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "session": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_import",
                    "description": "Import Packet28 transcript JSON into the local transcript store.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["content"],
                        "properties": {
                            "content": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.graph_create",
                    "description": "Create or update a Packet28 memoir-style graph container.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": {"type":"string"},
                            "description": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.graph_list",
                    "description": "List Packet28 memoir-style graph containers.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.graph_show",
                    "description": "Show one Packet28 memoir-style graph container with concepts and relations.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.graph_add_concept",
                    "description": "Add or update a local Packet28 graph concept.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {
                            "name": {"type":"string"},
                            "description": {"type":"string"},
                            "memoir": {"type":"string"},
                            "labels": {"type":"array","items":{"type":"string"}},
                            "confidence": {"type":"number","minimum":0,"maximum":1},
                            "source_ids": {"type":"array","items":{"type":"string"}}
                        }
                    }
                },
                {
                    "name": "packet28.graph_refine",
                    "description": "Refine a local Packet28 graph concept definition.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["name", "description"],
                        "properties": {
                            "name": {"type":"string"},
                            "description": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.graph_link",
                    "description": "Create a typed relation between two local Packet28 graph concepts.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["source", "target"],
                        "properties": {
                            "source": {"type":"string"},
                            "target": {"type":"string"},
                            "relation": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.graph_search",
                    "description": "Search local Packet28 graph concepts.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["query"],
                        "properties": {
                            "query": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.graph_export",
                    "description": "Export the local Packet28 graph as json, dot, or ascii.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "format": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.graph_stats",
                    "description": "Return local Packet28 graph counts and relation type statistics.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.graph_delete",
                    "description": "Delete a local Packet28 graph concept and attached relations.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {
                            "name": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.graph_inspect",
                    "description": "Inspect local Packet28 graph concepts and relations.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.graph_inspect_concept",
                    "description": "Inspect one Packet28 graph concept and its relation neighborhood.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {
                            "name": {"type":"string"},
                            "memoir": {"type":"string"},
                            "depth": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.graph_distill",
                    "description": "Distill memories from a Packet28 topic into memoir-style graph concepts.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["from_topic"],
                        "properties": {
                            "from_topic": {"type":"string"},
                            "into": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.write_intention",
                    "description": "Persist the current task objective and worker intent into Packet28.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["text"],
                        "properties": {
                            "task_id": {"type":"string"},
                            "text": {"type":"string"},
                            "note": {"type":"string"},
                            "step_id": {"type":"string"},
                            "question_id": {"type":"string"},
                            "paths": {"type":"array","items":{"type":"string"}},
                            "symbols": {"type":"array","items":{"type":"string"}}
                        }
                    }
                },
                {
                    "name": "packet28.task_status",
                    "description": "Return current Packet28 task status and handoff state.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.capabilities",
                    "description": "Describe the active Packet28 hooks-first runtime contract.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                }
            ]
            });
            rewrite_tool_names_for_cursor(&mut payload);
            Ok(payload)
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
    let payload = match name {
        "packet28.search" => {
            let mut request: Packet28SearchArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                Some(request.query.as_str()),
                "packet28.search",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_search(root, session, request)?
        }
        "packet28.search_fast" => {
            let request: Packet28SearchFastArgs = serde_json::from_value(arguments)?;
            handle_packet28_search_fast(root, session, request)?
        }
        "packet28.read_regions" => {
            let mut request: Packet28ReadRegionsArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                Some(request.path.as_str()),
                "packet28.read_regions",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_read_regions(root, session, request)?
        }
        "packet28.glob" => {
            let mut request: Packet28GlobArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                Some(request.pattern.as_str()),
                "packet28.glob",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_glob(root, session, request)?
        }
        "packet28.fetch_tool_result" => {
            let mut request: Packet28FetchToolResultArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                None,
                "packet28.fetch_tool_result",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_fetch_tool_result(root, request)?
        }
        "packet28.fetch_raw_output" => {
            let mut request: Packet28FetchRawOutputArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                Some(request.handle.as_str()),
                "packet28.fetch_raw_output",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_fetch_raw_output(root, request)?
        }
        "packet28.fetch_context" => {
            let mut request: Packet28FetchContextArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                None,
                "packet28.fetch_context",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_fetch_context(root, request)?
        }
        "packet28.verify_handoff" => {
            let mut request: Packet28VerifyHandoffArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_verify_handoff(root, request)?
        }
        "packet28.prompt_pressure" => {
            let mut request: Packet28PromptPressureArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_prompt_pressure(root, request)?
        }
        "packet28.handoff_diff" => {
            let mut request: Packet28HandoffDiffArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .left_artifact_id
                    .as_deref()
                    .or(request.left_context_version.as_deref())
                    .or(request.right_artifact_id.as_deref())
                    .or(request.right_context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_diff(root, request)?
        }
        "packet28.handoff_compress" => {
            let mut request: Packet28HandoffCompressionArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_compress(root, request)?
        }
        "packet28.prepare_handoff" | "packet28.handoff" => {
            let mut request: Packet28PrepareHandoffArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(session, root, &request.task_id, None, name)?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_prepare_handoff(root, request)?
        }
        "packet28.validate_plan" => {
            let mut request: Packet28ValidatePlanArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(session, root, &request.task_id, None, name)?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_validate_plan(root, request)?
        }
        "packet28.action_critic" => {
            let mut request: Packet28ActionCriticArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request.query.as_deref().or(request.tool_name.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_action_critic(root, request)?
        }
        "packet28.recommend_next_tool" => {
            let mut request: Packet28RecommendNextToolArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request.query.as_deref(),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_recommend_next_tool(root, request)?
        }
        "packet28.validate_tool_outcome" => {
            let mut request: Packet28ValidateToolOutcomeArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request.command.as_deref(),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_validate_tool_outcome(root, request)?
        }
        "packet28.patch_risk" => {
            let mut request: Packet28PatchRiskArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request.paths.first().map(String::as_str),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_patch_risk(root, request)?
        }
        "packet28.verify_experiments" => {
            let request: VerifyExperimentsToolArgs = serde_json::from_value(arguments)?;
            let manifest = root.join(
                request
                    .manifest
                    .as_deref()
                    .unwrap_or("docs/experiments/manifest.json"),
            );
            crate::cmd_verify::verify_experiments_payload(
                root,
                &manifest,
                &request.require_workflows.unwrap_or_default(),
                false,
            )?
        }
        "packet28.hypothesis_add" => {
            let mut request: HypothesisAddToolArgs = serde_json::from_value(arguments)?;
            request.task_id = Some(resolve_session_task_id(
                session,
                root,
                request.task_id.as_deref().unwrap_or_default(),
                Some(request.text.as_str()),
                name,
            )?);
            let task_id = request.task_id.as_deref().unwrap_or_default();
            track_task(session, root, task_id)?;
            serde_json::to_value(crate::cmd_hypothesis::add_hypothesis_record(
                root,
                task_id,
                request.id,
                &request.text,
                request.paths.unwrap_or_default(),
                request.symbols.unwrap_or_default(),
                request.artifact_id,
            )?)?
        }
        "packet28.hypothesis_list" => {
            let mut request: HypothesisListToolArgs = serde_json::from_value(arguments)?;
            request.task_id = Some(resolve_session_task_id(
                session,
                root,
                request.task_id.as_deref().unwrap_or_default(),
                None,
                name,
            )?);
            let task_id = request.task_id.as_deref().unwrap_or_default();
            track_task(session, root, task_id)?;
            serde_json::to_value(crate::cmd_hypothesis::active_hypotheses(root, task_id)?)?
        }
        "packet28.hypothesis_resolve" => {
            let mut request: HypothesisResolveToolArgs = serde_json::from_value(arguments)?;
            request.task_id = Some(resolve_session_task_id(
                session,
                root,
                request.task_id.as_deref().unwrap_or_default(),
                Some(request.id.as_str()),
                name,
            )?);
            let status = match request.status.trim() {
                "confirmed" | "confirm" => "confirmed",
                "rejected" | "reject" => "rejected",
                other => {
                    return Err(anyhow!(
                        "packet28.hypothesis_resolve status must be confirmed or rejected, got '{other}'"
                    ))
                }
            };
            let task_id = request.task_id.as_deref().unwrap_or_default();
            track_task(session, root, task_id)?;
            serde_json::to_value(crate::cmd_hypothesis::resolve_hypothesis_record(
                root,
                task_id,
                &request.id,
                status,
                request.note,
            )?)?
        }
        "packet28.reduce" => {
            let request: ReduceToolArgs = serde_json::from_value(arguments)?;
            handle_packet28_reduce(request)?
        }
        "packet28.rewrite" => {
            let request: RewriteToolArgs = serde_json::from_value(arguments)?;
            handle_packet28_rewrite(root, request)
        }
        "packet28.doctor" => {
            let request: DoctorToolArgs = serde_json::from_value(arguments)?;
            handle_packet28_doctor(root, request)?
        }
        "packet28.write_intention" => {
            let mut request: Packet28WriteIntentionArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                Some(request.text.as_str()),
                "packet28.write_intention",
            )?;
            track_task(session, root, &request.task_id)?;
            crate::task_runtime::store_active_task(
                root,
                &packet28_daemon_core::ActiveTaskRecord {
                    task_id: request.task_id.clone(),
                    session_id: None,
                    updated_at_unix: packet28_daemon_core::now_unix(),
                },
            )?;
            handle_packet28_write_intention(root, session, request)?
        }
        "packet28.memory_store" => {
            let request: MemoryStoreToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(store_memory_with_metadata(MemoryStoreInput {
                content: &request.content,
                tags: request.tags.as_deref(),
                topic: request.topic.as_deref(),
                importance: request.importance.as_deref(),
                keywords: request.keywords.as_deref(),
                project: request.project.as_deref(),
                source: request.source.as_deref(),
                raw_excerpt: request.raw_excerpt.as_deref(),
            })?)?
        }
        "packet28.memory_recall" => {
            let request: MemoryRecallToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(recall_memories_filtered(MemoryRecallQuery {
                query: &request.query,
                limit: request.limit.unwrap_or(10),
                topic: request.topic.as_deref(),
                project: request.project.as_deref(),
                tag: request.tag.as_deref(),
                keyword: request.keyword.as_deref(),
            })?)?
        }
        "packet28.memory_list" => {
            let request: MemoryListToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(list_memories_filtered(MemoryListQuery {
                limit: request.limit.unwrap_or(20),
                topic: request.topic.as_deref(),
                project: request.project.as_deref(),
                all: request.all.unwrap_or(false),
                sort: request.sort.as_deref().unwrap_or("recent"),
            })?)?
        }
        "packet28.memory_update" => {
            let request: MemoryUpdateToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(update_memory(MemoryUpdateInput {
                id: request.id,
                content: request.content.as_deref(),
                tags: request.tags.as_deref(),
                topic: request.topic.as_deref(),
                importance: request.importance.as_deref(),
                keywords: request.keywords.as_deref(),
                project: request.project.as_deref(),
                source: request.source.as_deref(),
                raw_excerpt: request.raw_excerpt.as_deref(),
            })?)?
        }
        "packet28.memory_forget" => {
            let request: MemoryForgetToolArgs = serde_json::from_value(arguments)?;
            let deleted = match (request.id, request.topic.as_deref()) {
                (Some(id), None) => forget_memory(id)?,
                (None, Some(topic)) => forget_memories_by_topic(topic)?,
                _ => return Err(anyhow!("pass exactly one of id or topic")),
            };
            json!({ "deleted": deleted })
        }
        "packet28.memory_topics" => serde_json::to_value(memory_topics()?)?,
        "packet28.memory_stats" => serde_json::to_value(local_store_stats()?)?,
        "packet28.memory_health" => {
            let request: MemoryHealthToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(memory_health(
                request.topic.as_deref(),
                request.stale_after_days.unwrap_or(30),
                request.consolidation_threshold.unwrap_or(10),
            )?)?
        }
        "packet28.memory_consolidate" => {
            let request: MemoryConsolidateToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(consolidate_memories(
                request.topic.as_deref(),
                request.keep_originals.unwrap_or(false),
            )?)?
        }
        "packet28.memory_decay" => {
            let request: MemoryDecayToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(decay_memories(request.factor.unwrap_or(0.95))?)?
        }
        "packet28.memory_prune" => {
            let request: MemoryPruneToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(prune_memories(
                request.threshold.unwrap_or(0.1),
                request.dry_run.unwrap_or(false),
            )?)?
        }
        "packet28.memory_embed" => {
            let request: MemoryEmbedToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(embed_memories(
                request.id,
                request.all.unwrap_or(false),
                request.dimensions.unwrap_or(384),
            )?)?
        }
        "packet28.memory_extract_patterns" => {
            let request: MemoryExtractPatternsToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(extract_memory_patterns(
                &request.topic,
                request.memoir.as_deref(),
                request.min_cluster_size.unwrap_or(3),
            )?)?
        }
        "packet28.memory_pending_enqueue" => {
            let request: MemoryPendingEnqueueToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(enqueue_pending_extraction(PendingExtractionInput {
                project: request.project.as_deref(),
                tool_name: request.tool_name.as_deref(),
                raw_output: &request.raw_output,
            })?)?
        }
        "packet28.memory_pending_list" => {
            let request: MemoryPendingListToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(list_pending_extractions(request.limit.unwrap_or(20))?)?
        }
        "packet28.memory_pending_process" => {
            let request: MemoryPendingProcessToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(process_pending_extractions(
                request.limit.unwrap_or(20),
                request.dry_run.unwrap_or(false),
            )?)?
        }
        "packet28.memory_pending_delete" => {
            let request: MemoryPendingDeleteToolArgs = serde_json::from_value(arguments)?;
            serde_json::json!({ "deleted": delete_pending_extractions(&request.ids)? })
        }
        "packet28.memory_pending_stats" => {
            let stats = local_store_stats()?;
            serde_json::json!({ "pending_extraction_count": stats.pending_extraction_count })
        }
        "packet28.feedback_record" => {
            let request: FeedbackRecordToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(record_feedback_with_metadata(FeedbackInput {
                subject: &request.subject,
                correction: &request.correction,
                topic: request.topic.as_deref(),
                context: request.context.as_deref(),
                predicted: request.predicted.as_deref(),
                reason: request.reason.as_deref(),
                source: request.source.as_deref(),
                project: request.project.as_deref(),
            })?)?
        }
        "packet28.feedback_search" => {
            let request: FeedbackSearchToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(search_feedback_filtered(
                &request.query,
                request.project.as_deref(),
                request.limit.unwrap_or(10),
            )?)?
        }
        "packet28.feedback_list" => {
            let request: FeedbackListToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(list_feedback(
                request.topic.as_deref(),
                request.limit.unwrap_or(20),
            )?)?
        }
        "packet28.feedback_apply" => {
            let request: FeedbackIdToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(apply_feedback(request.id)?)?
        }
        "packet28.feedback_delete" => {
            let request: FeedbackIdToolArgs = serde_json::from_value(arguments)?;
            serde_json::json!({ "deleted": delete_feedback(request.id)? })
        }
        "packet28.feedback_stats" => serde_json::to_value(feedback_stats()?)?,
        "packet28.wakeup" => {
            let request: WakeupToolArgs = serde_json::from_value(arguments)?;
            let paths = request
                .paths
                .as_ref()
                .or(request.path.as_ref())
                .cloned()
                .unwrap_or_default();
            let symbols = request
                .symbols
                .as_ref()
                .or(request.symbol.as_ref())
                .cloned()
                .unwrap_or_default();
            serde_json::to_value(build_wakeup_report_scoped(
                request.query.as_deref(),
                request.project.as_deref(),
                WakeupScope {
                    paths: paths.iter().map(String::as_str).collect(),
                    symbols: symbols.iter().map(String::as_str).collect(),
                    intent: request.intent.as_deref(),
                },
                request.limit.unwrap_or(5),
                request.max_tokens.unwrap_or(500),
                request.format.as_deref().unwrap_or("markdown"),
            )?)?
        }
        "packet28.learn_project" => {
            let request: LearnProjectToolArgs = serde_json::from_value(arguments)?;
            let dir = request
                .directory
                .unwrap_or_else(|| root.display().to_string());
            serde_json::to_value(learn_project_graph(
                Path::new(&dir),
                request.name.as_deref(),
                request.memoir.as_deref(),
                request.limit.unwrap_or(20),
            )?)?
        }
        "packet28.transcript_append" => {
            let request: TranscriptAppendToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(append_transcript_message(TranscriptAppendInput {
                session: request.session.as_deref(),
                agent: request.agent.as_deref(),
                role: request.role.as_deref(),
                content: &request.content,
                source: request.source.as_deref(),
                project: request.project.as_deref(),
            })?)?
        }
        "packet28.transcript_list" => {
            let request: TranscriptListToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(list_transcript_sessions(request.limit.unwrap_or(20))?)?
        }
        "packet28.transcript_show" => {
            let request: TranscriptShowToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(show_transcript_session(
                &request.session,
                request.limit.unwrap_or(100),
            )?)?
        }
        "packet28.transcript_search" => {
            let request: TranscriptSearchToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(search_transcripts_filtered(
                &request.query,
                request.project.as_deref(),
                request.limit.unwrap_or(10),
            )?)?
        }
        "packet28.transcript_stats" => serde_json::to_value(transcript_stats()?)?,
        "packet28.transcript_export" => {
            let request: TranscriptExportToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(export_transcripts(
                request.session.as_deref(),
                request.limit.unwrap_or(10_000),
            )?)?
        }
        "packet28.transcript_import" => {
            let request: TranscriptImportToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(import_transcripts_from_str(&request.content)?)?
        }
        "packet28.graph_create" => {
            let request: GraphCreateToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(create_graph_memoir(
                request.name.as_deref(),
                request.description.as_deref(),
            )?)?
        }
        "packet28.graph_list" => serde_json::to_value(list_graph_memoirs()?)?,
        "packet28.graph_show" => {
            let request: GraphShowToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(show_graph_memoir(
                request.name.as_deref(),
                request.limit.unwrap_or(50),
            )?)?
        }
        "packet28.graph_add_concept" => {
            let request: GraphConceptToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(add_concept_with_metadata(
                &request.name,
                request.description.as_deref(),
                request.memoir.as_deref(),
                &request.labels.unwrap_or_default(),
                request.confidence,
                &request.source_ids.unwrap_or_default(),
            )?)?
        }
        "packet28.graph_refine" => {
            let request: GraphRefineToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(refine_concept(&request.name, &request.description)?)?
        }
        "packet28.graph_link" => {
            let request: GraphLinkToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(link_concepts(
                &request.source,
                &request.target,
                request.relation.as_deref().unwrap_or("related_to"),
            )?)?
        }
        "packet28.graph_search" => {
            let request: GraphSearchToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(search_concepts_filtered(
                &request.query,
                request.memoir.as_deref(),
                request.label.as_deref(),
                request.limit.unwrap_or(10),
            )?)?
        }
        "packet28.graph_export" => {
            let request: GraphExportToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(export_graph(
                request.format.as_deref().unwrap_or("json"),
                request.limit.unwrap_or(100),
            )?)?
        }
        "packet28.graph_stats" => serde_json::to_value(graph_stats()?)?,
        "packet28.graph_delete" => {
            let request: GraphDeleteToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(delete_concept(&request.name)?)?
        }
        "packet28.graph_inspect" => {
            let request: GraphInspectToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(inspect_graph(request.limit.unwrap_or(50))?)?
        }
        "packet28.graph_inspect_concept" => {
            let request: GraphInspectConceptToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(inspect_graph_concept(
                &request.name,
                request.memoir.as_deref(),
                request.depth.unwrap_or(1),
            )?)?
        }
        "packet28.graph_distill" => {
            let request: GraphDistillToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(distill_memories_to_graph(
                &request.from_topic,
                request.into.as_deref(),
                request.limit.unwrap_or(100),
            )?)?
        }
        "packet28.task_status" => {
            let task_id = resolve_session_task_id(
                session,
                root,
                arguments
                    .get("task_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                None,
                "packet28.task_status",
            )?;
            track_task(session, root, &task_id)?;
            serde_json::to_value(broker_task_status_via_session(root, session, &task_id)?)?
        }
        "packet28.capabilities" => capabilities_payload(),
        _ => return Err(anyhow!("unsupported tool '{name}'")),
    };
    Ok(json!({
        "content": [
            {
                "type": "text",
                "text": summarize_tool_payload(name, &payload)
            }
        ],
        "structuredContent": payload
    }))
}

#[derive(Debug, Deserialize)]
struct MemoryStoreToolArgs {
    content: String,
    tags: Option<String>,
    topic: Option<String>,
    importance: Option<String>,
    keywords: Option<String>,
    project: Option<String>,
    source: Option<String>,
    raw_excerpt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryRecallToolArgs {
    query: String,
    limit: Option<usize>,
    topic: Option<String>,
    project: Option<String>,
    tag: Option<String>,
    keyword: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryListToolArgs {
    limit: Option<usize>,
    topic: Option<String>,
    project: Option<String>,
    all: Option<bool>,
    sort: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryUpdateToolArgs {
    id: i64,
    content: Option<String>,
    tags: Option<String>,
    topic: Option<String>,
    importance: Option<String>,
    keywords: Option<String>,
    project: Option<String>,
    source: Option<String>,
    raw_excerpt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryForgetToolArgs {
    id: Option<i64>,
    topic: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryHealthToolArgs {
    topic: Option<String>,
    stale_after_days: Option<i64>,
    consolidation_threshold: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MemoryConsolidateToolArgs {
    topic: Option<String>,
    keep_originals: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct MemoryDecayToolArgs {
    factor: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct MemoryPruneToolArgs {
    threshold: Option<f64>,
    dry_run: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct MemoryEmbedToolArgs {
    id: Option<i64>,
    all: Option<bool>,
    dimensions: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct MemoryExtractPatternsToolArgs {
    topic: String,
    memoir: Option<String>,
    min_cluster_size: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct MemoryPendingEnqueueToolArgs {
    raw_output: String,
    project: Option<String>,
    tool_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryPendingListToolArgs {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct MemoryPendingProcessToolArgs {
    limit: Option<usize>,
    dry_run: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct MemoryPendingDeleteToolArgs {
    ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
struct FeedbackRecordToolArgs {
    subject: String,
    correction: String,
    topic: Option<String>,
    context: Option<String>,
    predicted: Option<String>,
    reason: Option<String>,
    source: Option<String>,
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeedbackSearchToolArgs {
    query: String,
    limit: Option<usize>,
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeedbackListToolArgs {
    topic: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct FeedbackIdToolArgs {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct WakeupToolArgs {
    query: Option<String>,
    project: Option<String>,
    path: Option<Vec<String>>,
    paths: Option<Vec<String>>,
    symbol: Option<Vec<String>>,
    symbols: Option<Vec<String>>,
    intent: Option<String>,
    limit: Option<usize>,
    max_tokens: Option<usize>,
    format: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LearnProjectToolArgs {
    directory: Option<String>,
    name: Option<String>,
    memoir: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TranscriptAppendToolArgs {
    content: String,
    session: Option<String>,
    agent: Option<String>,
    role: Option<String>,
    source: Option<String>,
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TranscriptListToolArgs {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TranscriptShowToolArgs {
    session: String,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TranscriptSearchToolArgs {
    query: String,
    limit: Option<usize>,
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TranscriptExportToolArgs {
    session: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TranscriptImportToolArgs {
    content: String,
}

#[derive(Debug, Deserialize)]
struct GraphCreateToolArgs {
    name: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphShowToolArgs {
    name: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct GraphConceptToolArgs {
    name: String,
    description: Option<String>,
    memoir: Option<String>,
    labels: Option<Vec<String>>,
    confidence: Option<f64>,
    source_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct GraphRefineToolArgs {
    name: String,
    description: String,
}

#[derive(Debug, Deserialize)]
struct GraphLinkToolArgs {
    source: String,
    target: String,
    relation: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphSearchToolArgs {
    query: String,
    memoir: Option<String>,
    label: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct GraphExportToolArgs {
    format: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct GraphDeleteToolArgs {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GraphInspectToolArgs {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct GraphInspectConceptToolArgs {
    name: String,
    memoir: Option<String>,
    depth: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct GraphDistillToolArgs {
    from_topic: String,
    into: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ReduceToolArgs {
    command: String,
    stdout: Option<String>,
    stderr: Option<String>,
    exit_code: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct RewriteToolArgs {
    command: String,
    task_id: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DoctorToolArgs {
    agent: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VerifyExperimentsToolArgs {
    manifest: Option<String>,
    require_workflows: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct HypothesisAddToolArgs {
    task_id: Option<String>,
    id: Option<String>,
    text: String,
    paths: Option<Vec<String>>,
    symbols: Option<Vec<String>>,
    artifact_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HypothesisListToolArgs {
    task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HypothesisResolveToolArgs {
    task_id: Option<String>,
    id: String,
    status: String,
    note: Option<String>,
}

fn handle_packet28_reduce(request: ReduceToolArgs) -> Result<Value> {
    let spec = packet28_reducer_core::classify_command(&request.command)
        .ok_or_else(|| anyhow!("unsupported command for packet28.reduce"))?;
    let reduction = packet28_reducer_core::reduce_command_output(
        &spec,
        request.stdout.as_deref().unwrap_or_default(),
        request.stderr.as_deref().unwrap_or_default(),
        request.exit_code.unwrap_or(0),
    )?;
    Ok(json!({
        "command": request.command,
        "reduction": reduction,
        "reducer_family": spec.family,
        "reducer_kind": spec.canonical_kind,
    }))
}

fn handle_packet28_rewrite(root: &Path, request: RewriteToolArgs) -> Value {
    let cwd = request
        .cwd
        .clone()
        .unwrap_or_else(|| root.display().to_string());
    let decision = decide_command_route_with_cwd_and_root(&request.command, Path::new(&cwd), root);
    let task_id = request.task_id.as_deref().unwrap_or("packet28-mcp-rewrite");
    let rewritten = build_route_rewrite(
        root,
        task_id,
        request.session_id.as_deref(),
        &cwd,
        &decision,
    );
    let native_tool = decision.native_tool.as_ref().map(|tool| match tool.kind {
        NativeToolKind::Tree => "tree",
        NativeToolKind::Read => "read",
        NativeToolKind::Grep => "grep",
        NativeToolKind::Env => "env",
    });
    json!({
        "command": request.command,
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
        "native_tool": native_tool,
        "rewritten_command": rewritten,
        "reducer_family": decision.reducer_spec.as_ref().map(|spec| spec.family.clone()),
        "reducer_kind": decision
            .reducer_spec
            .as_ref()
            .map(|spec| spec.canonical_kind.clone()),
    })
}

fn handle_packet28_doctor(root: &Path, request: DoctorToolArgs) -> Result<Value> {
    let mut command = Command::new(std::env::current_exe()?);
    command.arg("doctor").arg("--root").arg(root).arg("--json");
    if let Some(agent) = request.agent {
        command.arg("--agent").arg(agent);
    }
    let output = command.output().context("failed to run Packet28 doctor")?;
    if !output.status.success() {
        return Err(anyhow!(
            "Packet28 doctor failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).context("Packet28 doctor did not return JSON")
}

fn capabilities_payload() -> Value {
    // Keep this payload minimal — it is injected into every MCP init and
    // counts against the agent's context budget.  Only include fields the
    // agent needs to *decide what to call*; omit anything derivable from
    // tool schemas or MCP protocol defaults.
    json!({
        "response_modes": ["slim", "full"],
        "hooks_first": true,
        "push_notification": "notifications/packet28.context_updated",
        "task_id_optional_after_first": true,
        "relaunch": "daemon_managed",
        "supersession": "replace"
    })
}

fn summarize_tool_payload(name: &str, payload: &Value) -> String {
    match name {
        "packet28.search" | "packet28.search_fast" | "packet28.read_regions" | "packet28.glob" => {
            payload
                .get("compact_preview")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "Packet28 compact tool result.".to_string())
        }
        "packet28.fetch_tool_result" => {
            let artifact_id = payload
                .get("artifact_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 fetched tool artifact {artifact_id}.")
        }
        "packet28.fetch_raw_output" => {
            let path = payload
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 fetched raw output from {path}.")
        }
        "packet28.fetch_context" => {
            let artifact_id = payload
                .get("artifact_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 fetched broker context artifact {artifact_id}.")
        }
        "packet28.verify_handoff" => {
            let ready = payload
                .get("ready")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let score = payload
                .get("score")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 handoff replay ready={ready} score={score}.")
        }
        "packet28.prompt_pressure" => {
            let pressure = payload
                .get("pressure")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let remaining = payload
                .get("remaining_tokens")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 prompt pressure={pressure} remaining_tokens={remaining}.")
        }
        "packet28.handoff_diff" => {
            let delta_count = payload
                .get("delta_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let top_delta = payload
                .get("top_delta")
                .and_then(Value::as_str)
                .unwrap_or("none");
            format!("Packet28 handoff diff delta_count={delta_count} top_delta={top_delta}.")
        }
        "packet28.handoff_compress" => {
            let recommendation_count = payload
                .get("recommendations")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            let projected_over_budget = payload
                .get("projected_over_budget")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            format!("Packet28 handoff compression recommendations={recommendation_count} projected_over_budget={projected_over_budget}.")
        }
        "packet28.prepare_handoff" | "packet28.handoff" => {
            let ready = payload
                .get("handoff_ready")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let reason = payload
                .get("handoff_reason")
                .and_then(Value::as_str)
                .unwrap_or("handoff prepared");
            if ready {
                format!("Packet28 prepared a handoff: {reason}")
            } else {
                format!("Packet28 did not prepare a handoff: {reason}")
            }
        }
        "packet28.validate_plan" => {
            let valid = payload
                .get("valid")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let violations = payload
                .get("violations")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            let warnings = payload
                .get("warnings")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            format!("Packet28 plan validation valid={valid} violations={violations} warnings={warnings}.")
        }
        "packet28.action_critic" => {
            let warning_count = payload
                .get("warnings")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            format!("Packet28 action critic returned {warning_count} warning(s).")
        }
        "packet28.recommend_next_tool" => {
            let recommendation_count = payload
                .get("recommendations")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            let token_estimate = payload
                .get("token_estimate")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!(
                "Packet28 recommended {recommendation_count} next tool(s), estimated {token_estimate} tokens."
            )
        }
        "packet28.validate_tool_outcome" => {
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let valid = payload
                .get("valid_success")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            format!("Packet28 tool outcome status={status} valid_success={valid}.")
        }
        "packet28.patch_risk" => {
            let risk = payload
                .get("risk")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let score = payload
                .get("score")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 patch risk={risk} score={score}.")
        }
        "packet28.verify_experiments" => {
            let ok = payload.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let experiments = payload
                .get("experiment_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let issues = payload
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!(
                "Packet28 experiment manifest ok={ok} experiments={experiments} issues={issues}."
            )
        }
        "packet28.hypothesis_add" => {
            let id = payload
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 recorded active hypothesis {id}.")
        }
        "packet28.hypothesis_list" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 listed {count} active hypothesis/hypotheses.")
        }
        "packet28.hypothesis_resolve" => {
            let id = payload
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("resolved");
            format!("Packet28 marked hypothesis {id} {status}.")
        }
        "packet28.reduce" => "Packet28 command reduction.".to_string(),
        "packet28.rewrite" => {
            let route = payload
                .get("route")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 rewrite route: {route}.")
        }
        "packet28.doctor" => "Packet28 doctor report.".to_string(),
        "packet28.memory_store" => {
            let id = payload
                .get("id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 stored memory {id}.")
        }
        "packet28.memory_recall" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 recalled {count} memor(y/ies).")
        }
        "packet28.memory_list" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 listed {count} memor(y/ies).")
        }
        "packet28.memory_update" => {
            let id = payload
                .get("id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 updated memory {id}.")
        }
        "packet28.memory_forget" => {
            let deleted = payload
                .get("deleted")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 deleted {deleted} memor(y/ies).")
        }
        "packet28.memory_topics" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 listed {count} memory topic(s).")
        }
        "packet28.memory_stats" => "Packet28 memory statistics.".to_string(),
        "packet28.memory_health" => {
            let total = payload
                .get("total_memories")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let needs = payload
                .get("topics_needing_consolidation")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!(
                "Packet28 memory health: {total} memories, {needs} topic(s) need consolidation."
            )
        }
        "packet28.memory_consolidate" => {
            let count = payload
                .get("source_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 memory consolidation {status} from {count} source memor(y/ies).")
        }
        "packet28.memory_decay" => {
            let count = payload
                .get("decayed_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 decayed {count} memor(y/ies).")
        }
        "packet28.memory_prune" => {
            let deleted = payload
                .get("deleted_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let candidates = payload
                .get("candidate_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 pruned {deleted} of {candidates} candidate memor(y/ies).")
        }
        "packet28.memory_embed" => {
            let count = payload
                .get("embedded_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 embedded {count} memor(y/ies).")
        }
        "packet28.memory_extract_patterns" => {
            let count = payload
                .get("pattern_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 extracted {count} memory pattern(s).")
        }
        "packet28.feedback_record" => {
            let id = payload
                .get("id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 recorded feedback {id}.")
        }
        "packet28.feedback_search" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 found {count} feedback correction(s).")
        }
        "packet28.feedback_list" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 listed {count} feedback correction(s).")
        }
        "packet28.feedback_apply" => {
            let count = payload
                .get("applied_count")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 feedback applied count is now {count}.")
        }
        "packet28.feedback_delete" => {
            let deleted = payload
                .get("deleted")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 deleted {deleted} feedback correction(s).")
        }
        "packet28.feedback_stats" => "Packet28 feedback statistics.".to_string(),
        "packet28.wakeup" => "Packet28 wake-up pack.".to_string(),
        "packet28.learn_project" => {
            let concepts = payload
                .get("total_concepts")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let links = payload
                .get("link_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 learned project graph: {concepts} concepts, {links} links.")
        }
        "packet28.transcript_append" => {
            let id = payload
                .get("id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 appended transcript message {id}.")
        }
        "packet28.transcript_list" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 listed {count} transcript session(s).")
        }
        "packet28.transcript_show" | "packet28.transcript_search" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 returned {count} transcript message(s).")
        }
        "packet28.transcript_stats" => "Packet28 transcript statistics.".to_string(),
        "packet28.transcript_export" => {
            let count = payload
                .get("messages")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            format!("Packet28 exported {count} transcript message(s).")
        }
        "packet28.transcript_import" => {
            let count = payload
                .get("imported_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 imported {count} transcript message(s).")
        }
        "packet28.graph_create" => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("default");
            format!("Packet28 graph memoir: {name}.")
        }
        "packet28.graph_list" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 returned {count} graph memoir(s).")
        }
        "packet28.graph_show" => {
            let name = payload
                .get("memoir")
                .and_then(|memoir| memoir.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("default");
            format!("Packet28 graph memoir detail: {name}.")
        }
        "packet28.graph_add_concept" | "packet28.graph_refine" => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("concept");
            format!("Packet28 graph concept: {name}.")
        }
        "packet28.graph_link" => "Packet28 graph relation recorded.".to_string(),
        "packet28.graph_search" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 found {count} graph concept(s).")
        }
        "packet28.graph_export" => {
            let format = payload
                .get("format")
                .and_then(Value::as_str)
                .unwrap_or("json");
            format!("Packet28 graph exported as {format}.")
        }
        "packet28.graph_stats" => "Packet28 graph statistics.".to_string(),
        "packet28.graph_delete" => {
            let deleted = payload
                .get("deleted_concepts")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 deleted {deleted} graph concept(s).")
        }
        "packet28.graph_inspect" => "Packet28 graph inspection.".to_string(),
        "packet28.graph_inspect_concept" => {
            let name = payload
                .get("concept")
                .and_then(|concept| concept.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("concept");
            format!("Packet28 graph concept inspection: {name}.")
        }
        "packet28.graph_distill" => {
            let created = payload
                .get("created_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let refined = payload
                .get("refined_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 distilled graph concepts: {created} created, {refined} refined.")
        }
        "packet28.task_status" => "Packet28 task status.".to_string(),
        "packet28.capabilities" => "Packet28 broker capabilities.".to_string(),
        _ => "Packet28 response.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_list_exposes_search_fast_without_task_id() {
        let root = tempfile::tempdir().unwrap();
        let session = Arc::new(Mutex::new(McpSessionState::default()));
        let payload = handle_method(root.path(), &session, "tools/list", Value::Null).unwrap();
        let tools = payload["tools"].as_array().unwrap();
        let search_fast = tools
            .iter()
            .find(|tool| tool["name"] == "packet28_search_fast")
            .unwrap();
        let props = search_fast["inputSchema"]["properties"]
            .as_object()
            .unwrap();

        assert!(props.contains_key("query"));
        assert!(!props.contains_key("task_id"));
    }

    #[test]
    fn tools_list_exposes_product_compatibility_aliases() {
        let root = tempfile::tempdir().unwrap();
        let session = Arc::new(Mutex::new(McpSessionState::default()));
        let payload = handle_method(root.path(), &session, "tools/list", Value::Null).unwrap();
        let tool_names = payload["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();

        for required in [
            "packet28_reduce",
            "packet28_rewrite",
            "packet28_handoff",
            "packet28_verify_handoff",
            "packet28_prompt_pressure",
            "packet28_handoff_diff",
            "packet28_handoff_compress",
            "packet28_validate_plan",
            "packet28_action_critic",
            "packet28_recommend_next_tool",
            "packet28_validate_tool_outcome",
            "packet28_patch_risk",
            "packet28_verify_experiments",
            "packet28_hypothesis_add",
            "packet28_hypothesis_list",
            "packet28_hypothesis_resolve",
            "packet28_doctor",
            "packet28_memory_list",
            "packet28_memory_embed",
            "packet28_memory_extract_patterns",
            "packet28_feedback_search",
            "packet28_feedback_list",
            "packet28_feedback_apply",
            "packet28_feedback_delete",
            "packet28_feedback_stats",
            "packet28_wakeup",
            "packet28_learn_project",
            "packet28_transcript_append",
            "packet28_transcript_search",
            "packet28_transcript_stats",
            "packet28_graph_create",
            "packet28_graph_list",
            "packet28_graph_show",
            "packet28_graph_search",
            "packet28_graph_export",
            "packet28_graph_stats",
            "packet28_graph_inspect_concept",
            "packet28_graph_distill",
        ] {
            assert!(
                tool_names.contains(&required),
                "{required} missing from tools/list"
            );
        }
    }

    #[test]
    fn reduce_and_rewrite_tools_return_structured_results() {
        let root = tempfile::tempdir().unwrap();
        let session = Arc::new(Mutex::new(McpSessionState::default()));
        let reduce = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.reduce",
                "arguments": {
                    "command": "git status --short",
                    "stdout": " M src/lib.rs\n",
                    "exit_code": 0
                }
            }),
        )
        .unwrap();
        assert_eq!(
            reduce["structuredContent"]["reducer_family"],
            Value::String("git".to_string())
        );

        let rewrite = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.rewrite",
                "arguments": {
                    "command": "git status --short"
                }
            }),
        )
        .unwrap();
        assert_eq!(
            rewrite["structuredContent"]["route"],
            Value::String("reducer_rewrite".to_string())
        );
    }

    #[test]
    fn verify_experiments_tool_returns_manifest_status() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("docs/experiments")).unwrap();
        std::fs::write(
            root.path().join("docs/experiments/evidence.md"),
            "saved_tokens: 12\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("docs/experiments/manifest.json"),
            r#"{
              "experiments": [{
                "id": "mcp-verify",
                "workflow": "MCP experiment audit",
                "commands": ["Packet28 verify experiments --json"],
                "artifacts": ["docs/experiments/evidence.md"],
                "metrics": [{"name":"saved_tokens","value":12,"min":10,"evidence":["saved_tokens: 12"]}],
                "runtime_versions": [{"name":"packet28","version":"0.2.59"}]
              }]
            }"#,
        )
        .unwrap();
        let session = Arc::new(Mutex::new(McpSessionState::default()));
        let response = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.verify_experiments",
                "arguments": {
                    "require_workflows": ["MCP experiment audit"]
                }
            }),
        )
        .unwrap();

        assert_eq!(response["structuredContent"]["ok"], true);
        assert_eq!(response["structuredContent"]["experiment_count"], 1);
        assert_eq!(response["structuredContent"]["issue_count"], 0);
    }

    #[test]
    fn verify_handoff_fails_when_next_action_is_missing() {
        let root = tempfile::tempdir().unwrap();
        let task_id = "task-handoff-replay";
        let context_version = "ctx-missing-next";
        let path = task_version_json_path(root.path(), task_id, context_version);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "context_version": context_version,
                "artifact_id": context_version,
                "brief": "## Task Objective\nFinish the replay verifier.",
                "sections": [{"id": "context_debt", "title": "Context Debt", "body": "- debt_summary: stale_paths=0 open_questions=0 unverified_edits=0 contradictions=0"}],
                "evidence_artifact_ids": ["artifact-1"],
                "next_action_summary": null,
                "latest_intention": null
            }))
            .unwrap(),
        )
        .unwrap();
        let session = Arc::new(Mutex::new(McpSessionState::default()));
        let response = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.verify_handoff",
                "arguments": {
                    "task_id": task_id,
                    "context_version": context_version
                }
            }),
        )
        .unwrap();

        assert_eq!(response["structuredContent"]["ready"], false);
        assert_eq!(response["structuredContent"]["score"], 75);
        assert!(response["structuredContent"]["missing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|missing| missing == "next_action"));
        assert!(
            serde_json::to_string(&response["structuredContent"])
                .unwrap()
                .len()
                < 512
        );
    }

    #[test]
    fn prompt_pressure_identifies_largest_removable_section() {
        let root = tempfile::tempdir().unwrap();
        let task_id = "task-prompt-pressure";
        let context_version = "ctx-pressure";
        let path = task_version_json_path(root.path(), task_id, context_version);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "context_version": context_version,
                "artifact_id": context_version,
                "brief": "## Task Objective\nKeep only the decisive replay context.",
                "sections": [
                    {"id": "objective", "title": "Objective", "body": "finish the prompt pressure verifier"},
                    {"id": "search_evidence", "title": "Search Evidence", "body": "line with redundant evidence ".repeat(90)},
                    {"id": "next_action", "title": "Next Action", "body": "run focused verifier"}
                ],
                "next_action_summary": "run focused verifier"
            }))
            .unwrap(),
        )
        .unwrap();
        let session = Arc::new(Mutex::new(McpSessionState::default()));
        let response = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.prompt_pressure",
                "arguments": {
                    "task_id": task_id,
                    "context_version": context_version,
                    "next_prompt": "Continue the handoff and implement the focused verifier.",
                    "budget_tokens": 220
                }
            }),
        )
        .unwrap();

        assert_eq!(response["structuredContent"]["pressure"], "over_budget");
        assert_eq!(response["structuredContent"]["over_budget"], true);
        assert_eq!(
            response["structuredContent"]["largest_removable_sections"][0]["id"],
            "search_evidence"
        );
        assert!(
            serde_json::to_string(&response["structuredContent"])
                .unwrap()
                .len()
                < 768
        );
    }

    #[test]
    fn handoff_diff_reports_changed_next_action_as_top_delta() {
        let root = tempfile::tempdir().unwrap();
        let task_id = "task-handoff-diff";
        for (context_version, next_action) in [
            ("ctx-before", "run cargo check before editing"),
            ("ctx-after", "edit cmd_mcp_native.rs before cargo check"),
        ] {
            let path = task_version_json_path(root.path(), task_id, context_version);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                serde_json::to_vec_pretty(&json!({
                    "context_version": context_version,
                    "artifact_id": context_version,
                    "brief": "## Task Objective\nFinish the handoff diff verifier.",
                    "sections": [{"id": "context_debt", "title": "Context Debt", "body": "none"}],
                    "evidence_artifact_ids": ["artifact-1"],
                    "next_action_summary": next_action
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let session = Arc::new(Mutex::new(McpSessionState::default()));
        let response = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.handoff_diff",
                "arguments": {
                    "task_id": task_id,
                    "left_context_version": "ctx-before",
                    "right_context_version": "ctx-after"
                }
            }),
        )
        .unwrap();

        assert_eq!(response["structuredContent"]["top_delta"], "next_action");
        assert_eq!(
            response["structuredContent"]["deltas"][0]["field"],
            "next_action"
        );
        assert!(
            serde_json::to_string(&response["structuredContent"])
                .unwrap()
                .len()
                < 1024
        );
    }

    #[test]
    fn handoff_compress_preserves_objective_and_next_action_sections() {
        let root = tempfile::tempdir().unwrap();
        let task_id = "task-handoff-compress";
        let context_version = "ctx-compress";
        let path = task_version_json_path(root.path(), task_id, context_version);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "context_version": context_version,
                "artifact_id": context_version,
                "brief": "## Task Objective\nCompress the handoff without losing replay anchors.",
                "sections": [
                    {"id": "objective", "title": "Objective", "body": "preserve this objective anchor ".repeat(40)},
                    {"id": "next_action", "title": "Next Action", "body": "preserve this next action anchor ".repeat(40)},
                    {"id": "search_evidence", "title": "Search Evidence", "body": "drop redundant search result ".repeat(120)}
                ],
                "evidence_artifact_ids": ["artifact-1"],
                "next_action_summary": "continue with focused verification"
            }))
            .unwrap(),
        )
        .unwrap();
        let session = Arc::new(Mutex::new(McpSessionState::default()));
        let response = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.handoff_compress",
                "arguments": {
                    "task_id": task_id,
                    "context_version": context_version,
                    "next_prompt": "Continue with focused verification.",
                    "budget_tokens": 350
                }
            }),
        )
        .unwrap();

        let recommendations = response["structuredContent"]["recommendations"]
            .as_array()
            .unwrap();
        assert!(recommendations
            .iter()
            .any(|recommendation| recommendation["id"] == "search_evidence"));
        assert!(!recommendations
            .iter()
            .any(|recommendation| recommendation["id"] == "objective"));
        assert!(!recommendations
            .iter()
            .any(|recommendation| recommendation["id"] == "next_action"));
        assert!(
            serde_json::to_string(&response["structuredContent"])
                .unwrap()
                .len()
                < 1024
        );
    }

    #[test]
    fn recommend_next_tool_changes_with_focus_freshness_and_roi() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".packet28")).unwrap();
        std::fs::write(
            root.path().join(".packet28/run-savings.jsonl"),
            [
                json!({
                    "command": "Packet28 run -- cargo test",
                    "cwd": root.path().display().to_string(),
                    "family": "rust",
                    "canonical_kind": "cargo_test",
                    "exit_code": 0,
                    "raw_est_tokens": 1200,
                    "reduced_est_tokens": 100,
                    "savings_percent": 91.7,
                    "fallback_reason": null,
                    "failure_fingerprint": null,
                    "changed_paths": ["src/lib.rs"],
                    "timestamp_unix_ms": 20
                })
                .to_string(),
                json!({
                    "command": "Packet28 run -- npm test",
                    "cwd": root.path().display().to_string(),
                    "family": "node",
                    "canonical_kind": "npm_test",
                    "exit_code": 0,
                    "raw_est_tokens": 300,
                    "reduced_est_tokens": 100,
                    "savings_percent": 66.7,
                    "fallback_reason": null,
                    "failure_fingerprint": null,
                    "changed_paths": [],
                    "timestamp_unix_ms": 10
                })
                .to_string(),
            ]
            .join("\n"),
        )
        .unwrap();
        let session = Arc::new(Mutex::new(McpSessionState::default()));

        let roi = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.recommend_next_tool",
                "arguments": {
                    "task_id": "task-route",
                    "query": "what should I run next",
                    "max_recommendations": 1
                }
            }),
        )
        .unwrap();
        assert_eq!(
            roi["structuredContent"]["recommendations"][0]["command"],
            "Packet28 run -- cargo test"
        );
        assert!(roi["structuredContent"]["token_estimate"].as_u64().unwrap() < 256);

        let focused = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.recommend_next_tool",
                "arguments": {
                    "task_id": "task-route",
                    "focus_paths": ["src/lib.rs"],
                    "max_recommendations": 1
                }
            }),
        )
        .unwrap();
        assert_eq!(
            focused["structuredContent"]["recommendations"][0]["risk"],
            "stale_focus_evidence"
        );
        assert!(
            focused["structuredContent"]["recommendations"][0]["command"]
                .as_str()
                .unwrap()
                .contains("packet28.read_regions")
        );
    }

    #[test]
    fn validate_tool_outcome_does_not_treat_fallback_as_success() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".packet28")).unwrap();
        std::fs::write(
            root.path().join(".packet28/run-savings.jsonl"),
            json!({
                "command": "Packet28 run -- rg auth",
                "cwd": root.path().display().to_string(),
                "family": "search",
                "canonical_kind": "rg",
                "exit_code": 0,
                "raw_est_tokens": 900,
                "reduced_est_tokens": 200,
                "savings_percent": 77.8,
                "fallback_reason": "fff auto preferred backend failed: launch error",
                "failure_fingerprint": null,
                "changed_paths": [],
                "timestamp_unix_ms": 20
            })
            .to_string(),
        )
        .unwrap();
        let session = Arc::new(Mutex::new(McpSessionState::default()));

        let response = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.validate_tool_outcome",
                "arguments": {
                    "task_id": "task-outcome",
                    "command": "rg auth"
                }
            }),
        )
        .unwrap();

        assert_eq!(response["structuredContent"]["status"], "fallback");
        assert_eq!(response["structuredContent"]["valid_success"], false);
        assert!(response["structuredContent"]["evidence"]
            .as_str()
            .unwrap()
            .contains("fallback_reason="));
    }

    #[test]
    fn patch_risk_requires_broader_checks_for_shared_unmapped_paths() {
        let root = tempfile::tempdir().unwrap();
        let state_dir = root.path().join(".covy/state");
        std::fs::create_dir_all(&state_dir).unwrap();
        let mut index = suite_packet_core::TestMapIndex::default();
        index.file_to_tests.insert(
            "src/leaf.rs".to_string(),
            ["tests/leaf_test.rs".to_string()].into_iter().collect(),
        );
        testy_core::pipeline_testmap::write_testmap(&state_dir.join("testmap.bin"), &index)
            .unwrap();
        let session = Arc::new(Mutex::new(McpSessionState::default()));

        let leaf = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.patch_risk",
                "arguments": {
                    "task_id": "task-risk",
                    "paths": ["src/leaf.rs"]
                }
            }),
        )
        .unwrap();
        let shared = handle_tool_call(
            root.path(),
            &session,
            json!({
                "name": "packet28.patch_risk",
                "arguments": {
                    "task_id": "task-risk",
                    "paths": ["src/lib.rs"]
                }
            }),
        )
        .unwrap();

        assert!(
            shared["structuredContent"]["score"].as_u64().unwrap()
                > leaf["structuredContent"]["score"].as_u64().unwrap()
        );
        assert_eq!(shared["structuredContent"]["risk"], "medium");
        assert!(shared["structuredContent"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason == "missing_testmap_mappings=1"));
        assert!(leaf["structuredContent"]["required_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "run tests/leaf_test.rs"));
    }
}
