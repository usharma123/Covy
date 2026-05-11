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
    task_version_json_path, BrokerPrepareHandoffRequest, BrokerResponseMode,
    BrokerTaskStatusRequest, BrokerTaskStatusResponse, BrokerWriteOp, BrokerWriteStateBatchRequest,
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
    handle_packet28_write_intention, Packet28FetchContextArgs, Packet28FetchRawOutputArgs,
    Packet28FetchToolResultArgs, Packet28GlobArgs, Packet28PrepareHandoffArgs,
    Packet28ReadRegionsArgs, Packet28SearchArgs, Packet28SearchFastArgs,
    Packet28WriteIntentionArgs,
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
use crate::memory_store::{
    inspect_graph, list_memories, local_store_stats, recall_memories, record_feedback,
    search_feedback, store_memory,
};
use crate::route_registry::{
    build_route_rewrite, decide_command_route_with_cwd, NativeToolKind, RouteKind,
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
    if !tool_names.contains(&"packet28.search") {
        return Err(anyhow!("packet28.search missing from tools/list"));
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
        "tools/list" => Ok(json!({
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
                            "tags": {"type":"string"}
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
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.memory_list",
                    "description": "List recent local Packet28 memories from ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1}
                        }
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
                            "correction": {"type":"string"}
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
                            "limit": {"type":"integer","minimum":1}
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
        })),
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
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing tool name"))?;
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
        "packet28.prepare_handoff" | "packet28.handoff" => {
            let mut request: Packet28PrepareHandoffArgs = serde_json::from_value(arguments)?;
            request.task_id = resolve_session_task_id(session, root, &request.task_id, None, name)?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_prepare_handoff(root, request)?
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
            serde_json::to_value(store_memory(&request.content, request.tags.as_deref())?)?
        }
        "packet28.memory_recall" => {
            let request: MemoryRecallToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(recall_memories(
                &request.query,
                request.limit.unwrap_or(10),
            )?)?
        }
        "packet28.memory_list" => {
            let request: MemoryListToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(list_memories(request.limit.unwrap_or(20))?)?
        }
        "packet28.feedback_record" => {
            let request: FeedbackRecordToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(record_feedback(&request.subject, &request.correction)?)?
        }
        "packet28.feedback_search" => {
            let request: FeedbackSearchToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(search_feedback(
                &request.query,
                request.limit.unwrap_or(10),
            )?)?
        }
        "packet28.feedback_stats" => serde_json::to_value(local_store_stats()?)?,
        "packet28.graph_inspect" => {
            let request: GraphInspectToolArgs = serde_json::from_value(arguments)?;
            serde_json::to_value(inspect_graph(request.limit.unwrap_or(50))?)?
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
}

#[derive(Debug, Deserialize)]
struct MemoryRecallToolArgs {
    query: String,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct MemoryListToolArgs {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct FeedbackRecordToolArgs {
    subject: String,
    correction: String,
}

#[derive(Debug, Deserialize)]
struct FeedbackSearchToolArgs {
    query: String,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct GraphInspectToolArgs {
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
    let decision = decide_command_route_with_cwd(&request.command, Path::new(&cwd));
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
        "packet28.feedback_stats" => "Packet28 feedback statistics.".to_string(),
        "packet28.graph_inspect" => "Packet28 graph inspection.".to_string(),
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
            .find(|tool| tool["name"] == "packet28.search_fast")
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
            "packet28.reduce",
            "packet28.rewrite",
            "packet28.handoff",
            "packet28.doctor",
            "packet28.memory_list",
            "packet28.feedback_search",
            "packet28.feedback_stats",
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
}
