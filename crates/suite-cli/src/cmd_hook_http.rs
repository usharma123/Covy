use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use packet28_daemon_protocol::hooks::HookRuntimeConfig;
use packet28_daemon_protocol::paths::daemon_dir;
use serde_json::{json, Value};

use crate::cmd_hook::{process_claude_hook_payload, HookHttpServerArgs};

const CLAUDE_HTTP_HOOK_PATH: &str = "/packet28/claude-hook";
const CLAUDE_HTTP_HEALTH_PATH: &str = "/packet28/health";
const CLAUDE_HTTP_TOKEN_HEADER: &str = "x-packet28-hook-token";
const HOOK_HTTP_START_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HTTP_HOOK_BODY_BYTES: usize = 8 * 1024 * 1024;

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

pub(crate) fn run_hook_http_server(args: HookHttpServerArgs) -> Result<i32> {
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

pub(crate) fn ensure_hook_http_server(
    root: &Path,
    runtime_config: &HookRuntimeConfig,
) -> Result<()> {
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
