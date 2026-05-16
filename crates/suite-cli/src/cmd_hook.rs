use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use packet28_daemon_core::{
    daemon_dir, hook_runtime_config_path, load_task_registry, now_unix, task_artifact_dir,
    ActiveTaskRecord, BrokerAction, BrokerGetContextRequest, HookBoundaryKind, HookEventKind,
    HookIngestRequest, HookLifecycleEvent, HookLifecycleKind, HookReducerCacheEntry,
    HookReducerPacket, HookRuntimeConfig, TaskRecord,
};
use packet28_reducer_core::{
    classify_command, classify_command_argv, reduce_command_output, CommandReducerSpec,
};
use serde_json::{json, Value};

use crate::cmd_wakeup::build_wakeup_pack_for_injection;
use crate::memory_store::{
    append_transcript_message, enqueue_pending_extraction, hook_event_stats, list_hook_events,
    record_hook_event, HookEventInput, PendingExtractionInput, TranscriptAppendInput,
};

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

#[derive(Clone, Copy)]
enum ExternalHookRuntime {
    Copilot,
    Cursor,
    Gemini,
    Windsurf,
}

const CLAUDE_HTTP_HOOK_PATH: &str = "/packet28/claude-hook";
const CLAUDE_HTTP_HEALTH_PATH: &str = "/packet28/health";
const CLAUDE_HTTP_TOKEN_HEADER: &str = "x-packet28-hook-token";
const HOOK_HTTP_START_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HTTP_HOOK_BODY_BYTES: usize = 8 * 1024 * 1024;

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

struct ClaudeHookOutcome {
    exit_code: i32,
    body: Option<String>,
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

fn process_claude_hook_payload(
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

#[derive(Clone)]
struct HookHttpSettings {
    port: u16,
    token: String,
}

#[derive(Debug)]
struct HttpHookRequest {
    method: String,
    path: String,
    headers: std::collections::HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Debug)]
struct HttpClientResponse {
    status_code: u16,
    body: String,
}

fn run_hook_http_server(args: HookHttpServerArgs) -> Result<i32> {
    let root = crate::broker_client::resolve_root(&args.root);
    let listener = TcpListener::bind(("127.0.0.1", args.port))
        .with_context(|| format!("failed to bind Packet28 hook server on port {}", args.port))?;
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let root = root.clone();
                let token = args.token.clone();
                thread::spawn(move || {
                    if let Err(err) = handle_hook_http_connection(stream, &root, &token) {
                        eprintln!("packet28 hook http request failed: {err:#}");
                    }
                });
            }
            Err(err) => return Err(anyhow!(err)).context("Packet28 hook HTTP listener failed"),
        }
    }
    Ok(0)
}

fn ensure_hook_http_server(root: &Path, runtime_config: &HookRuntimeConfig) -> Result<()> {
    let Some(settings) = hook_http_settings(runtime_config) else {
        return Ok(());
    };
    if hook_http_server_healthy(root, &settings).unwrap_or(false) {
        return Ok(());
    }
    start_hook_http_server(root, &settings)?;
    wait_for_hook_http_server(root, &settings, HOOK_HTTP_START_TIMEOUT)
}

fn hook_http_settings(runtime_config: &HookRuntimeConfig) -> Option<HookHttpSettings> {
    let token = runtime_config
        .http_hook_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    Some(HookHttpSettings {
        port: runtime_config.http_hook_port?,
        token,
    })
}

fn start_hook_http_server(root: &Path, settings: &HookHttpSettings) -> Result<()> {
    let exe = std::env::current_exe().context("failed to resolve current Packet28 binary")?;
    let log_path = daemon_dir(root).join("packet28-hook-http.log");
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create hook log dir '{}'", parent.display()))?;
    }
    let stdout = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open hook log '{}'", log_path.display()))?;
    let stderr = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open hook log '{}'", log_path.display()))?;
    Command::new(exe)
        .arg("hook")
        .arg("serve-http")
        .arg("--root")
        .arg(root.display().to_string())
        .arg("--port")
        .arg(settings.port.to_string())
        .arg("--token")
        .arg(&settings.token)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .context("failed to spawn Packet28 hook HTTP server")?;
    Ok(())
}

fn wait_for_hook_http_server(
    root: &Path,
    settings: &HookHttpSettings,
    timeout: Duration,
) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if hook_http_server_healthy(root, settings).unwrap_or(false) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(anyhow!(
        "Packet28 hook HTTP server did not become ready at {}",
        hook_http_health_url(settings)
    ))
}

fn hook_http_server_healthy(root: &Path, settings: &HookHttpSettings) -> Result<bool> {
    let response = send_http_request(
        settings.port,
        "GET",
        CLAUDE_HTTP_HEALTH_PATH,
        &[(CLAUDE_HTTP_TOKEN_HEADER, settings.token.as_str())],
        None,
    )?;
    if response.status_code != 200 {
        return Ok(false);
    }
    let value: Value = serde_json::from_str(&response.body)?;
    Ok(value["service"] == json!("packet28-hook-http")
        && value["workspace_root"] == json!(root.display().to_string()))
}

fn hook_http_health_url(settings: &HookHttpSettings) -> String {
    format!(
        "http://127.0.0.1:{}{}",
        settings.port, CLAUDE_HTTP_HEALTH_PATH
    )
}

fn handle_hook_http_connection(stream: TcpStream, root: &Path, token: &str) -> Result<()> {
    let request = read_http_request(&stream)?;
    let mut stream = stream;
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", CLAUDE_HTTP_HEALTH_PATH) => {
            let supplied_token = request
                .headers
                .get(CLAUDE_HTTP_TOKEN_HEADER)
                .map(String::as_str)
                .unwrap_or_default();
            if supplied_token != token {
                write_http_response(&mut stream, 401, "text/plain", "unauthorized")?;
                return Ok(());
            }
            write_http_response(
                &mut stream,
                200,
                "application/json",
                &serde_json::to_string(&json!({
                    "service": "packet28-hook-http",
                    "workspace_root": root.display().to_string(),
                }))?,
            )?;
        }
        ("POST", CLAUDE_HTTP_HOOK_PATH) => {
            let supplied_token = request
                .headers
                .get(CLAUDE_HTTP_TOKEN_HEADER)
                .map(String::as_str)
                .unwrap_or_default();
            if supplied_token != token {
                write_http_response(&mut stream, 401, "text/plain", "unauthorized")?;
                return Ok(());
            }
            let payload: Value = if request.body.is_empty() {
                Value::Object(Default::default())
            } else {
                match serde_json::from_slice(&request.body) {
                    Ok(value) => value,
                    Err(err) => {
                        write_http_response(
                            &mut stream,
                            500,
                            "text/plain",
                            &format!("invalid JSON payload: {err}"),
                        )?;
                        return Ok(());
                    }
                }
            };
            let outcome = match process_claude_hook_payload(root, None, &payload, false) {
                Ok(outcome) => outcome,
                Err(err) => {
                    write_http_response(
                        &mut stream,
                        500,
                        "text/plain",
                        &format!("Packet28 hook processing failed: {err}"),
                    )?;
                    return Ok(());
                }
            };
            let body = outcome.body.unwrap_or_else(|| "{}".to_string());
            write_http_response(&mut stream, 200, "application/json", &body)?;
        }
        _ => {
            write_http_response(&mut stream, 404, "text/plain", "not found")?;
        }
    }
    Ok(())
}

fn read_http_request(stream: &TcpStream) -> Result<HttpHookRequest> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .context("failed to read HTTP request line")?;
    let request_line = request_line.trim_end_matches(['\r', '\n']);
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    if method.is_empty() || path.is_empty() {
        return Err(anyhow!("malformed HTTP request line"));
    }
    let mut headers = std::collections::HashMap::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("failed to read HTTP request headers")?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > MAX_HTTP_HOOK_BODY_BYTES {
        return Err(anyhow!(
            "HTTP hook payload exceeded {MAX_HTTP_HOOK_BODY_BYTES} bytes"
        ));
    }
    let mut body = vec![0; content_length];
    if content_length > 0 {
        reader
            .read_exact(&mut body)
            .context("failed to read HTTP hook body")?;
    }
    Ok(HttpHookRequest {
        method,
        path,
        headers,
        body,
    })
}

fn write_http_response(
    stream: &mut TcpStream,
    status_code: u16,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let reason = match status_code {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status_code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .context("failed to write HTTP response")?;
    stream.flush().context("failed to flush HTTP response")
}

fn send_http_request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Result<HttpClientResponse> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .with_context(|| format!("failed to connect to Packet28 hook server on port {port}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .context("failed to set hook health read timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .context("failed to set hook health write timeout")?;
    let payload = body.unwrap_or_default();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nConnection: close\r\n",
        payload.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(payload);
    stream
        .write_all(request.as_bytes())
        .context("failed to send HTTP request")?;
    stream.flush().context("failed to flush HTTP request")?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .context("failed to read HTTP status line")?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("invalid HTTP status line"))?;
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("failed to read HTTP response headers")?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0; content_length];
    if content_length > 0 {
        reader
            .read_exact(&mut body)
            .context("failed to read HTTP response body")?;
    }
    Ok(HttpClientResponse {
        status_code,
        body: String::from_utf8(body).context("invalid UTF-8 in HTTP response body")?,
    })
}

fn run_reducer_runner(args: ReducerRunnerArgs) -> Result<i32> {
    let root = crate::broker_client::resolve_root(&args.root);
    crate::broker_client::ensure_daemon(&root)?;
    if args.argv.is_empty() {
        return Err(anyhow!("reducer-runner requires a command after '--'"));
    }

    let task_id = if let Some(task_id) = args
        .task_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        task_id
    } else if let Some(active) = crate::task_runtime::load_active_task(&root) {
        active.task_id
    } else {
        crate::broker_client::derive_task_id("claude-hook-runner")
    };
    crate::task_runtime::store_active_task(
        &root,
        &ActiveTaskRecord {
            task_id: task_id.clone(),
            session_id: args.session_id.clone(),
            updated_at_unix: now_unix(),
        },
    )?;

    let cwd = args
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.clone());
    let command_text = shell_join(&args.argv);
    let spec = classify_command_argv(&command_text, &args.argv)
        .ok_or_else(|| anyhow!("command is not eligible for reducer rewrite"))?;
    if spec.family != args.family
        || spec.canonical_kind != args.kind
        || spec.cache_fingerprint != args.fingerprint
    {
        return Err(anyhow!("reducer-runner classification mismatch"));
    }

    if let Some((cached_packet, exit_code)) =
        cached_reducer_packet(&root, &task_id, &spec, &command_text)
    {
        let command_id = format!("runner-cache-{}", now_unix_millis());
        let _ = crate::broker_client::hook_ingest(
            &root,
            HookIngestRequest {
                task_id,
                session_id: args.session_id,
                event_kind: HookEventKind::CommandFinished,
                matcher: None,
                source: Some("packet28-reducer-runner-cache".to_string()),
                boundary_kind: HookBoundaryKind::None,
                lifecycle_event: Some(HookLifecycleEvent {
                    kind: HookLifecycleKind::CommandFinished,
                    command_id: Some(command_id),
                    reducer_family: cached_packet.reducer_family.clone(),
                    canonical_command_kind: cached_packet.canonical_command_kind.clone(),
                    cache_fingerprint: cached_packet.cache_fingerprint.clone(),
                    elapsed_ms: Some(0),
                    exit_code: cached_packet.exit_code,
                    ..HookLifecycleEvent::default()
                }),
                reducer_packet: Some(cached_packet.clone()),
                host_context_budget_tokens: None,
            },
        )?;
        println!("{}", cached_packet.summary);
        return Ok(exit_code);
    }

    let command_id = format!("runner-{}", now_unix_millis());
    let spool_dir = task_artifact_dir(&root, &task_id).join("hook-spool");
    fs::create_dir_all(&spool_dir)?;
    let stdout_path = spool_dir.join(format!("{command_id}-stdout.log"));
    let stderr_path = spool_dir.join(format!("{command_id}-stderr.log"));
    let stdout_file = File::create(&stdout_path)
        .with_context(|| format!("failed to create '{}'", stdout_path.display()))?;
    let stderr_file = File::create(&stderr_path)
        .with_context(|| format!("failed to create '{}'", stderr_path.display()))?;

    let _ = crate::broker_client::hook_ingest(
        &root,
        HookIngestRequest {
            task_id: task_id.clone(),
            session_id: args.session_id.clone(),
            event_kind: HookEventKind::CommandStarted,
            matcher: None,
            source: Some("packet28-reducer-runner".to_string()),
            boundary_kind: HookBoundaryKind::None,
            lifecycle_event: Some(HookLifecycleEvent {
                kind: HookLifecycleKind::CommandStarted,
                command_id: Some(command_id.clone()),
                reducer_family: Some(spec.family.clone()),
                canonical_command_kind: Some(spec.canonical_kind.clone()),
                cache_fingerprint: Some(spec.cache_fingerprint.clone()),
                stdout_spool_path: Some(stdout_path.display().to_string()),
                stderr_spool_path: Some(stderr_path.display().to_string()),
                ..HookLifecycleEvent::default()
            }),
            reducer_packet: None,
            host_context_budget_tokens: None,
        },
    )?;

    let started = Instant::now();
    let mut child = Command::new(&args.argv[0])
        .args(&args.argv[1..])
        .current_dir(&cwd)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .envs(args.env.iter().filter_map(|entry| {
            entry
                .split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
        }))
        .spawn()
        .with_context(|| format!("failed to spawn '{}'", args.argv[0]))?;

    let mut last_stdout_bytes = 0_u64;
    let mut last_stderr_bytes = 0_u64;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let stdout_bytes = fs::metadata(&stdout_path)
            .map(|meta| meta.len())
            .unwrap_or(last_stdout_bytes);
        let stderr_bytes = fs::metadata(&stderr_path)
            .map(|meta| meta.len())
            .unwrap_or(last_stderr_bytes);
        if stdout_bytes != last_stdout_bytes || stderr_bytes != last_stderr_bytes {
            last_stdout_bytes = stdout_bytes;
            last_stderr_bytes = stderr_bytes;
            let _ = crate::broker_client::hook_ingest(
                &root,
                HookIngestRequest {
                    task_id: task_id.clone(),
                    session_id: args.session_id.clone(),
                    event_kind: HookEventKind::CommandProgress,
                    matcher: None,
                    source: Some("packet28-reducer-runner".to_string()),
                    boundary_kind: HookBoundaryKind::None,
                    lifecycle_event: Some(HookLifecycleEvent {
                        kind: HookLifecycleKind::CommandProgress,
                        command_id: Some(command_id.clone()),
                        reducer_family: Some(spec.family.clone()),
                        canonical_command_kind: Some(spec.canonical_kind.clone()),
                        cache_fingerprint: Some(spec.cache_fingerprint.clone()),
                        stdout_spool_path: Some(stdout_path.display().to_string()),
                        stderr_spool_path: Some(stderr_path.display().to_string()),
                        stdout_bytes: Some(stdout_bytes),
                        stderr_bytes: Some(stderr_bytes),
                        elapsed_ms: Some(started.elapsed().as_millis() as u64),
                        ..HookLifecycleEvent::default()
                    }),
                    reducer_packet: None,
                    host_context_budget_tokens: None,
                },
            );
        }
        thread::sleep(Duration::from_millis(200));
    };

    let stdout = fs::read_to_string(&stdout_path).unwrap_or_default();
    let stderr = fs::read_to_string(&stderr_path).unwrap_or_default();
    let exit_code = status.code().unwrap_or(1);
    let reduced = reduce_command_output(&spec, &stdout, &stderr, exit_code)?;
    let artifact = json!({
        "command_id": command_id,
        "command": command_text,
        "argv": args.argv,
        "cwd": cwd.display().to_string(),
        "stdout_spool_path": stdout_path.display().to_string(),
        "stderr_spool_path": stderr_path.display().to_string(),
        "stdout_preview": compact_text(&stdout, 400),
        "stderr_preview": compact_text(&stderr, 400),
        "stdout_bytes": fs::metadata(&stdout_path).map(|meta| meta.len()).unwrap_or(0),
        "stderr_bytes": fs::metadata(&stderr_path).map(|meta| meta.len()).unwrap_or(0),
        "exit_code": exit_code,
    });
    let est_bytes = reduced.summary.len() as u64;
    let est_tokens = ((est_bytes as f64) / 4.0).ceil() as u64;
    let response = crate::broker_client::hook_ingest(
        &root,
        HookIngestRequest {
            task_id,
            session_id: args.session_id,
            event_kind: HookEventKind::CommandFinished,
            matcher: None,
            source: Some("packet28-reducer-runner".to_string()),
            boundary_kind: HookBoundaryKind::None,
            lifecycle_event: Some(HookLifecycleEvent {
                kind: HookLifecycleKind::CommandFinished,
                command_id: Some(command_id),
                reducer_family: Some(reduced.family.clone()),
                canonical_command_kind: Some(reduced.canonical_kind.clone()),
                cache_fingerprint: Some(reduced.cache_fingerprint.clone()),
                stdout_spool_path: Some(stdout_path.display().to_string()),
                stderr_spool_path: Some(stderr_path.display().to_string()),
                stdout_bytes: Some(
                    fs::metadata(&stdout_path)
                        .map(|meta| meta.len())
                        .unwrap_or(0),
                ),
                stderr_bytes: Some(
                    fs::metadata(&stderr_path)
                        .map(|meta| meta.len())
                        .unwrap_or(0),
                ),
                elapsed_ms: Some(started.elapsed().as_millis() as u64),
                exit_code: Some(exit_code),
            }),
            reducer_packet: Some(HookReducerPacket {
                packet_type: reduced.packet_type,
                tool_name: "Bash".to_string(),
                operation_kind: reduced.operation_kind,
                reducer_family: Some(reduced.family),
                canonical_command_kind: Some(reduced.canonical_kind),
                summary: reduced.summary.clone(),
                compact_preview: (!reduced.compact_preview.is_empty())
                    .then_some(reduced.compact_preview.clone()),
                command: Some(command_text),
                search_query: None,
                compact_path: Some("reducer_rewrite".to_string()),
                passthrough_reason: None,
                raw_est_tokens: Some((((stdout.len() + stderr.len()) as f64) / 4.0).ceil() as u64),
                reduced_est_tokens: Some(est_tokens),
                paths: reduced.paths,
                regions: reduced.regions,
                symbols: reduced.symbols,
                equivalence_key: reduced.equivalence_key,
                est_tokens,
                est_bytes,
                failed: reduced.failed,
                error_class: reduced.error_class,
                error_message: reduced.error_message,
                retryable: reduced.retryable,
                duration_ms: Some(started.elapsed().as_millis() as u64),
                exit_code: Some(reduced.exit_code),
                cache_fingerprint: Some(reduced.cache_fingerprint),
                cacheable: Some(reduced.cacheable),
                mutation: Some(reduced.mutation),
                raw_artifact_handle: Some(stdout_path.display().to_string()),
                raw_artifact_available: true,
                artifact: Some(artifact),
            }),
            host_context_budget_tokens: None,
        },
    )?;
    let _ = response;
    println!("{}", reduced.summary);
    Ok(exit_code)
}

fn run_reduce_fixture(args: ReduceFixtureArgs) -> Result<i32> {
    let stdout = fs::read_to_string(&args.stdout_path)
        .with_context(|| format!("failed to read fixture '{}'", args.stdout_path))?;
    let stderr = if let Some(stderr_path) = args.stderr_path.as_ref() {
        fs::read_to_string(stderr_path)
            .with_context(|| format!("failed to read fixture '{}'", stderr_path))?
    } else {
        String::new()
    };
    let spec = classify_command(&args.command)
        .ok_or_else(|| anyhow!("fixture command is not eligible for reducer classification"))?;
    let reduced = reduce_command_output(&spec, &stdout, &stderr, args.exit_code)?;
    let raw_visible = format!("{stdout}{stderr}");
    let raw_tokens = estimate_text_tokens(&raw_visible);
    let reduced_tokens = estimate_text_tokens(&reduced.summary);
    let payload = json!({
        "command": args.command,
        "family": reduced.family,
        "canonical_kind": reduced.canonical_kind,
        "summary": reduced.summary,
        "failed": reduced.failed,
        "exit_code": reduced.exit_code,
        "raw_bytes": raw_visible.len(),
        "raw_est_tokens": raw_tokens,
        "reduced_bytes": payload_text_len(&reduced.summary),
        "reduced_est_tokens": reduced_tokens,
        "raw_preview": compact_text(&raw_visible, 400),
        "reduced_preview": reduced.summary,
        "token_reduction_pct": reduction_pct(raw_tokens, reduced_tokens),
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!(
            "{}",
            payload["reduced_preview"].as_str().unwrap_or_default()
        );
    }
    Ok(0)
}

fn cached_reducer_packet(
    root: &Path,
    task_id: &str,
    spec: &CommandReducerSpec,
    command_text: &str,
) -> Option<(HookReducerPacket, i32)> {
    let registry = load_task_registry(root).ok()?;
    let task = registry.tasks.get(task_id)?;
    let entry = task.hook_reducer_cache.get(&spec.cache_fingerprint)?;
    if !cache_entry_matches(task, entry, spec) {
        return None;
    }
    let est_bytes = entry.summary.len() as u64;
    let est_tokens = ((est_bytes as f64) / 4.0).ceil() as u64;
    let exit_code = entry.exit_code.unwrap_or(if entry.failed { 1 } else { 0 });
    Some((
        HookReducerPacket {
            packet_type: spec.packet_type.clone(),
            tool_name: "Bash".to_string(),
            operation_kind: spec.operation_kind,
            reducer_family: Some(spec.family.clone()),
            canonical_command_kind: Some(spec.canonical_kind.clone()),
            summary: entry.summary.clone(),
            compact_preview: entry.compact_preview.clone(),
            command: Some(command_text.to_string()),
            search_query: None,
            compact_path: Some("reducer_rewrite".to_string()),
            passthrough_reason: None,
            raw_est_tokens: None,
            reduced_est_tokens: Some(est_tokens),
            paths: entry.paths.clone(),
            regions: entry.regions.clone(),
            symbols: entry.symbols.clone(),
            equivalence_key: spec.equivalence_key.clone(),
            est_tokens,
            est_bytes,
            failed: entry.failed,
            error_class: entry.failed.then_some("cached_tool_error".to_string()),
            error_message: entry.error_message.clone(),
            retryable: entry.failed.then_some(false),
            duration_ms: Some(0),
            exit_code: Some(exit_code),
            cache_fingerprint: Some(spec.cache_fingerprint.clone()),
            cacheable: Some(spec.cacheable),
            mutation: Some(spec.mutation),
            raw_artifact_handle: entry.raw_artifact_handle.clone(),
            raw_artifact_available: entry.raw_artifact_handle.is_some(),
            artifact: None,
        },
        exit_code,
    ))
}

fn cache_entry_matches(
    task: &TaskRecord,
    entry: &HookReducerCacheEntry,
    spec: &CommandReducerSpec,
) -> bool {
    if entry.reducer_family != spec.family || entry.canonical_command_kind != spec.canonical_kind {
        return false;
    }
    if entry.git_epoch != task.hook_git_epoch
        || entry.fs_epoch != task.hook_fs_epoch
        || entry.rust_epoch != task.hook_rust_epoch
    {
        return false;
    }
    if entry.reducer_family == "github" {
        let age = now_unix().saturating_sub(entry.occurred_at_unix);
        return age <= 300;
    }
    true
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

fn parse_runtime_event_kind(runtime: ExternalHookRuntime, value: Option<&str>) -> HookEventKind {
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

fn runtime_session_id(runtime: ExternalHookRuntime, payload: &Value) -> Option<String> {
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

fn runtime_matcher(
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

fn runtime_command(payload: &Value) -> Option<String> {
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

fn build_runtime_reducer_packet(
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

fn build_runtime_pretool_rewrite(
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
    build_pretool_rewrite(
        runtime_config,
        root,
        &normalized,
        event_kind,
        task_id,
        session_id,
    )
}

fn render_runtime_hook_output(
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

fn external_runtime_name(runtime: ExternalHookRuntime) -> &'static str {
    match runtime {
        ExternalHookRuntime::Copilot => "copilot",
        ExternalHookRuntime::Cursor => "cursor",
        ExternalHookRuntime::Gemini => "gemini",
        ExternalHookRuntime::Windsurf => "windsurf",
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
    if matches!(decision.kind, crate::route_registry::RouteKind::NativeTool) {
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

fn build_reducer_packet(
    runtime_config: &HookRuntimeConfig,
    payload: &Value,
    event_kind: HookEventKind,
) -> Option<HookReducerPacket> {
    if !matches!(
        event_kind,
        HookEventKind::PostToolUse | HookEventKind::PostToolUseFailure
    ) || !runtime_config.fallback_post_tool_capture
    {
        return None;
    }
    let tool_name = json_string(payload, "tool_name")?;
    let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
    if tool_name == "Bash"
        && json_string(&input, "command")
            .as_deref()
            .is_some_and(|command| command.contains(" hook reducer-runner "))
    {
        return None;
    }
    match event_kind {
        HookEventKind::PostToolUse => {
            let response = payload.get("tool_response").cloned().unwrap_or(Value::Null);
            match tool_name.as_str() {
                "Bash" => build_bash_packet(&input, &response),
                "Read" => build_read_packet(&input, &response),
                "Grep" => build_grep_packet(&input, &response),
                "Glob" => build_glob_packet(&input, &response),
                "Edit" | "MultiEdit" | "Write" => build_edit_packet(&tool_name, &input, &response),
                _ => Some(build_generic_packet(&tool_name, &input, &response)),
            }
        }
        HookEventKind::PostToolUseFailure => {
            let error = json_string(payload, "error").unwrap_or_else(|| hook_output_text(payload));
            Some(build_failed_tool_packet(
                &tool_name, &input, payload, &error,
            ))
        }
        _ => None,
    }
}

fn build_bash_packet(input: &Value, response: &Value) -> Option<HookReducerPacket> {
    let command = json_string(input, "command")?;
    let output = hook_output_text(response);
    let summary = first_nonempty_line(&output)
        .unwrap_or_else(|| format!("command completed: {}", compact_text(&command, 100)));
    let spec = classify_command(&command);
    let (packet_type, operation_kind, family, canonical_kind, fingerprint, paths, equivalence_key) =
        if let Some(spec) = spec {
            (
                format!("packet28.hook.fallback.{}.v1", spec.family),
                spec.operation_kind,
                Some(spec.family),
                Some(spec.canonical_kind),
                Some(spec.cache_fingerprint),
                spec.paths,
                spec.equivalence_key,
            )
        } else {
            (
                "packet28.hook.command.v1".to_string(),
                suite_packet_core::ToolOperationKind::Generic,
                Some("generic".to_string()),
                None,
                None,
                extract_command_paths(&command),
                None,
            )
        };
    Some(packet_from_parts(
        &packet_type,
        "Bash",
        operation_kind,
        family,
        canonical_kind,
        summary,
        Some(command),
        None,
        paths,
        Vec::new(),
        Vec::new(),
        equivalence_key,
        fingerprint,
        Some(false),
        response.clone(),
        response_failed(response),
    ))
}

fn build_read_packet(input: &Value, response: &Value) -> Option<HookReducerPacket> {
    let path = json_string(input, "file_path")
        .or_else(|| json_string(input, "path"))
        .or_else(|| json_string(input, "target"))?;
    let line_start = input.get("offset").and_then(Value::as_u64).unwrap_or(1);
    let count = input.get("limit").and_then(Value::as_u64).unwrap_or(1);
    let line_end = line_start.saturating_add(count.saturating_sub(1));
    let summary = format!("Read {} lines from {}", count, path);
    let mut regions = json_array_strings(response, "regions");
    if regions.is_empty() {
        regions.push(format!("{path}:{line_start}-{line_end}"));
    }
    Some(packet_from_parts(
        "packet28.hook.read.v1",
        "Read",
        suite_packet_core::ToolOperationKind::Read,
        Some("claude_native".to_string()),
        Some("read".to_string()),
        summary,
        None,
        None,
        vec![path.clone()],
        regions,
        json_array_strings(response, "symbols"),
        Some(format!("read:{path}")),
        Some(format!("read:{}:{}:{}", path, line_start, line_end)),
        Some(true),
        response.clone(),
        response_failed(response),
    ))
}

fn build_grep_packet(input: &Value, response: &Value) -> Option<HookReducerPacket> {
    let query = json_string(input, "pattern")
        .or_else(|| json_string(input, "query"))
        .or_else(|| json_string(input, "search"))?;
    let paths = json_array_strings(response, "files")
        .into_iter()
        .chain(json_array_strings(input, "include"))
        .collect::<Vec<_>>();
    let count = json_array_len(response, "matches")
        .unwrap_or_else(|| hook_output_text(response).lines().count());
    let summary = format!("Grep found {count} matches for '{query}'");
    Some(packet_from_parts(
        "packet28.hook.grep.v1",
        "Grep",
        suite_packet_core::ToolOperationKind::Search,
        Some("claude_native".to_string()),
        Some("grep".to_string()),
        summary,
        None,
        Some(query.clone()),
        paths.clone(),
        Vec::new(),
        Vec::new(),
        Some(format!("grep:{}:{}", query, paths.join(","))),
        Some(format!("grep:{}:{}", query, paths.join(","))),
        Some(true),
        response.clone(),
        response_failed(response),
    ))
}

fn build_glob_packet(input: &Value, response: &Value) -> Option<HookReducerPacket> {
    let pattern = json_string(input, "pattern")?;
    let paths = if let Some(array) = response.as_array() {
        array
            .iter()
            .filter_map(|item| item.as_str().map(ToOwned::to_owned))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let summary = format!("Glob matched {} path(s) for '{}'", paths.len(), pattern);
    Some(packet_from_parts(
        "packet28.hook.glob.v1",
        "Glob",
        suite_packet_core::ToolOperationKind::Search,
        Some("claude_native".to_string()),
        Some("glob".to_string()),
        summary,
        None,
        Some(pattern.clone()),
        paths,
        Vec::new(),
        Vec::new(),
        Some(format!("glob:{pattern}")),
        Some(format!("glob:{pattern}")),
        Some(true),
        response.clone(),
        response_failed(response),
    ))
}

fn build_failed_tool_packet(
    tool_name: &str,
    input: &Value,
    payload: &Value,
    error: &str,
) -> HookReducerPacket {
    match tool_name {
        "Bash" => build_bash_failure_packet(input, payload, error)
            .unwrap_or_else(|| build_generic_failure_packet(tool_name, input, payload, error)),
        "Read" => build_read_failure_packet(input, payload, error)
            .unwrap_or_else(|| build_generic_failure_packet(tool_name, input, payload, error)),
        "Grep" => build_grep_failure_packet(input, payload, error)
            .unwrap_or_else(|| build_generic_failure_packet(tool_name, input, payload, error)),
        "Glob" => build_glob_failure_packet(input, payload, error)
            .unwrap_or_else(|| build_generic_failure_packet(tool_name, input, payload, error)),
        "Edit" | "MultiEdit" | "Write" => {
            build_edit_failure_packet(tool_name, input, payload, error)
                .unwrap_or_else(|| build_generic_failure_packet(tool_name, input, payload, error))
        }
        _ => build_generic_failure_packet(tool_name, input, payload, error),
    }
}

fn build_bash_failure_packet(
    input: &Value,
    payload: &Value,
    error: &str,
) -> Option<HookReducerPacket> {
    let command = json_string(input, "command")?;
    let spec = classify_command(&command);
    let summary = first_nonempty_line(error)
        .unwrap_or_else(|| format!("command failed: {}", compact_text(&command, 100)));
    let (packet_type, operation_kind, family, canonical_kind, paths, equivalence_key) =
        if let Some(spec) = spec {
            (
                format!("packet28.hook.fallback.{}.failure.v1", spec.family),
                spec.operation_kind,
                Some(spec.family),
                Some(spec.canonical_kind),
                spec.paths,
                spec.equivalence_key,
            )
        } else {
            (
                "packet28.hook.command.failure.v1".to_string(),
                suite_packet_core::ToolOperationKind::Generic,
                Some("generic".to_string()),
                None,
                extract_command_paths(&command),
                None,
            )
        };
    Some(packet_from_parts(
        &packet_type,
        "Bash",
        operation_kind,
        family,
        canonical_kind,
        summary,
        Some(command),
        None,
        paths,
        Vec::new(),
        Vec::new(),
        equivalence_key,
        None,
        Some(false),
        payload.clone(),
        true,
    ))
}

fn build_read_failure_packet(
    input: &Value,
    payload: &Value,
    error: &str,
) -> Option<HookReducerPacket> {
    let path = json_string(input, "file_path")
        .or_else(|| json_string(input, "path"))
        .or_else(|| json_string(input, "target"))?;
    let line_start = input.get("offset").and_then(Value::as_u64).unwrap_or(1);
    let count = input.get("limit").and_then(Value::as_u64).unwrap_or(1);
    let line_end = line_start.saturating_add(count.saturating_sub(1));
    Some(packet_from_parts(
        "packet28.hook.read.failure.v1",
        "Read",
        suite_packet_core::ToolOperationKind::Read,
        Some("claude_native".to_string()),
        Some("read".to_string()),
        format!("Read failed for {}: {}", path, compact_text(error, 140)),
        None,
        None,
        vec![path.clone()],
        vec![format!("{path}:{line_start}-{line_end}")],
        Vec::new(),
        Some(format!("read:{path}")),
        None,
        Some(false),
        payload.clone(),
        true,
    ))
}

fn build_grep_failure_packet(
    input: &Value,
    payload: &Value,
    error: &str,
) -> Option<HookReducerPacket> {
    let query = json_string(input, "pattern")
        .or_else(|| json_string(input, "query"))
        .or_else(|| json_string(input, "search"))?;
    let paths = json_array_strings(input, "include");
    Some(packet_from_parts(
        "packet28.hook.grep.failure.v1",
        "Grep",
        suite_packet_core::ToolOperationKind::Search,
        Some("claude_native".to_string()),
        Some("grep".to_string()),
        format!("Grep failed for '{}': {}", query, compact_text(error, 140)),
        None,
        Some(query.clone()),
        paths.clone(),
        Vec::new(),
        Vec::new(),
        Some(format!("grep:{}:{}", query, paths.join(","))),
        None,
        Some(false),
        payload.clone(),
        true,
    ))
}

fn build_glob_failure_packet(
    input: &Value,
    payload: &Value,
    error: &str,
) -> Option<HookReducerPacket> {
    let pattern = json_string(input, "pattern")?;
    Some(packet_from_parts(
        "packet28.hook.glob.failure.v1",
        "Glob",
        suite_packet_core::ToolOperationKind::Search,
        Some("claude_native".to_string()),
        Some("glob".to_string()),
        format!(
            "Glob failed for '{}': {}",
            pattern,
            compact_text(error, 140)
        ),
        None,
        Some(pattern.clone()),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Some(format!("glob:{pattern}")),
        None,
        Some(false),
        payload.clone(),
        true,
    ))
}

fn build_edit_failure_packet(
    tool_name: &str,
    input: &Value,
    payload: &Value,
    error: &str,
) -> Option<HookReducerPacket> {
    let path = json_string(input, "file_path")
        .or_else(|| json_string(input, "path"))
        .or_else(|| json_string(input, "target"))?;
    Some(packet_from_parts(
        "packet28.hook.edit.failure.v1",
        tool_name,
        suite_packet_core::ToolOperationKind::Edit,
        Some("claude_native".to_string()),
        Some("edit".to_string()),
        format!(
            "{} failed for {}: {}",
            tool_name,
            path,
            compact_text(error, 140)
        ),
        None,
        None,
        vec![path],
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        payload.clone(),
        true,
    ))
}

fn build_generic_failure_packet(
    tool_name: &str,
    input: &Value,
    payload: &Value,
    error: &str,
) -> HookReducerPacket {
    packet_from_parts(
        "packet28.hook.generic.failure.v1",
        tool_name,
        suite_packet_core::ToolOperationKind::Generic,
        Some("claude_native".to_string()),
        None,
        format!("{tool_name} failed: {}", compact_text(error, 140)),
        json_string(input, "command"),
        None,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        payload.clone(),
        true,
    )
}

fn build_edit_packet(
    tool_name: &str,
    input: &Value,
    response: &Value,
) -> Option<HookReducerPacket> {
    let path = json_string(input, "file_path")
        .or_else(|| json_string(input, "path"))
        .or_else(|| json_string(input, "target"))?;
    let summary = format!("{tool_name} updated {path}");
    Some(packet_from_parts(
        "packet28.hook.edit.v1",
        tool_name,
        suite_packet_core::ToolOperationKind::Edit,
        Some("claude_native".to_string()),
        Some("edit".to_string()),
        summary,
        None,
        None,
        vec![path],
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        json!({
            "tool_input": input,
            "tool_response": response,
        }),
        response_failed(response),
    ))
}

fn build_generic_packet(tool_name: &str, input: &Value, response: &Value) -> HookReducerPacket {
    packet_from_parts(
        "packet28.hook.generic.v1",
        tool_name,
        suite_packet_core::ToolOperationKind::Generic,
        Some("claude_native".to_string()),
        None,
        format!("{tool_name} completed"),
        None,
        None,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        json!({
            "tool_input": input,
            "tool_response": response,
        }),
        response_failed(response),
    )
}

#[allow(clippy::too_many_arguments)]
fn packet_from_parts(
    packet_type: &str,
    tool_name: &str,
    operation_kind: suite_packet_core::ToolOperationKind,
    reducer_family: Option<String>,
    canonical_command_kind: Option<String>,
    summary: String,
    command: Option<String>,
    search_query: Option<String>,
    paths: Vec<String>,
    regions: Vec<String>,
    symbols: Vec<String>,
    equivalence_key: Option<String>,
    cache_fingerprint: Option<String>,
    cacheable: Option<bool>,
    artifact: Value,
    failed: bool,
) -> HookReducerPacket {
    let raw_text = hook_output_text(&artifact);
    let raw_est_tokens = (((raw_text.len()) as f64) / 4.0).ceil() as u64;
    let est_bytes = summary.len() as u64;
    let est_tokens = ((est_bytes as f64) / 4.0).ceil() as u64;
    let compact_path = if reducer_family.as_deref() == Some("claude_native") {
        Some("native_tool".to_string())
    } else if tool_name == "Bash" {
        Some("raw_passthrough".to_string())
    } else {
        Some("native_tool".to_string())
    };
    let passthrough_reason = (tool_name == "Bash").then(|| "post_tool_capture".to_string());
    HookReducerPacket {
        packet_type: packet_type.to_string(),
        tool_name: tool_name.to_string(),
        operation_kind,
        reducer_family,
        canonical_command_kind,
        summary,
        compact_preview: None,
        command,
        search_query,
        compact_path,
        passthrough_reason,
        raw_est_tokens: Some(raw_est_tokens),
        reduced_est_tokens: Some(est_tokens),
        paths,
        regions,
        symbols,
        equivalence_key,
        est_tokens,
        est_bytes,
        failed,
        error_class: failed.then(|| "tool_error".to_string()),
        error_message: failed.then(|| compact_text(&hook_output_text(&artifact), 200)),
        retryable: failed.then_some(false),
        duration_ms: None,
        exit_code: None,
        cache_fingerprint,
        cacheable,
        mutation: Some(false),
        raw_artifact_handle: None,
        raw_artifact_available: false,
        artifact: Some(artifact),
    }
}

fn load_hook_runtime_config(root: &Path) -> HookRuntimeConfig {
    fs::read_to_string(hook_runtime_config_path(root))
        .ok()
        .and_then(|raw| serde_json::from_str::<HookRuntimeConfig>(&raw).ok())
        .unwrap_or_default()
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn json_nested_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn json_array_strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(ToOwned::to_owned))
        .collect()
}

fn json_array_len(value: &Value, key: &str) -> Option<usize> {
    value.get(key).and_then(Value::as_array).map(Vec::len)
}

fn hook_output_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    for key in ["stdout", "stderr", "output", "text", "content", "error"] {
        if let Some(text) = json_string(value, key) {
            return text;
        }
    }
    serde_json::to_string(value).unwrap_or_else(|_| String::new())
}

fn first_nonempty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| compact_text(line, 160))
}

fn compact_text(value: &str, limit: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let char_count = compact.chars().count();
    if char_count <= limit {
        compact
    } else if limit <= 3 {
        "...".to_string()
    } else {
        let shortened = compact
            .chars()
            .take(limit.saturating_sub(3))
            .collect::<String>();
        format!("{shortened}...")
    }
}

fn runtime_source(runtime: ExternalHookRuntime) -> &'static str {
    match runtime {
        ExternalHookRuntime::Copilot => "packet28.copilot.hook",
        ExternalHookRuntime::Cursor => "packet28.cursor.hook",
        ExternalHookRuntime::Gemini => "packet28.gemini.hook",
        ExternalHookRuntime::Windsurf => "packet28.windsurf.hook",
    }
}

fn response_failed(response: &Value) -> bool {
    response
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || response.get("error").is_some()
}

fn extract_command_paths(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .filter(|part| {
            part.contains('/')
                || part.ends_with(".rs")
                || part.ends_with(".md")
                || part.ends_with(".json")
                || part.ends_with(".toml")
        })
        .map(|part| {
            part.trim_matches(|ch| ch == '"' || ch == '\'' || ch == ',')
                .to_string()
        })
        .collect()
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '='))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn payload_text_len(text: &str) -> usize {
    text.len()
}

fn estimate_text_tokens(text: &str) -> u64 {
    let bytes = text.len() as u64;
    if bytes == 0 {
        0
    } else {
        bytes.div_ceil(4)
    }
}

fn reduction_pct(raw_tokens: u64, reduced_tokens: u64) -> f64 {
    if raw_tokens == 0 {
        0.0
    } else {
        ((raw_tokens.saturating_sub(reduced_tokens)) as f64 * 100.0 / raw_tokens as f64 * 10.0)
            .round()
            / 10.0
    }
}

fn now_unix_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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
