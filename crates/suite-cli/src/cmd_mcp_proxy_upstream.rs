use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use tokio::io::{AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, watch, Mutex as AsyncMutex, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::{timeout_at, Instant};

use super::config::McpProxyConfig;
use super::transport::{
    read_message_async, render_command_preview, write_message_async, McpMessageFraming,
};
use super::McpSessionState;

const DEFAULT_UPSTREAM_TIMEOUT_MS: u64 = 30_000;
const MAX_UPSTREAM_INFLIGHT: usize = 32;
pub(super) const MAX_PROXY_OUTPUT_MESSAGES: usize = 64;

type PendingReply = std::result::Result<Value, String>;

#[derive(Clone)]
pub(crate) struct ProxyOutput {
    sender: mpsc::Sender<OutboundMessage>,
}

pub(crate) struct OutboundMessage {
    pub(super) value: Value,
    framing: McpMessageFraming,
}

impl ProxyOutput {
    pub(crate) async fn send(&self, value: Value, framing: McpMessageFraming) -> Result<()> {
        self.sender
            .send(OutboundMessage { value, framing })
            .await
            .map_err(|_| anyhow!("MCP stdout writer stopped"))
    }

    pub(crate) fn try_send(&self, value: Value, framing: McpMessageFraming) -> Result<bool> {
        match self.sender.try_send(OutboundMessage { value, framing }) {
            Ok(()) => Ok(true),
            Err(mpsc::error::TrySendError::Full(_)) => Ok(false),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(anyhow!("MCP stdout writer stopped")),
        }
    }
}

pub(crate) fn proxy_output_channel() -> (ProxyOutput, mpsc::Receiver<OutboundMessage>) {
    let (sender, receiver) = mpsc::channel(MAX_PROXY_OUTPUT_MESSAGES);
    (ProxyOutput { sender }, receiver)
}

pub(crate) async fn write_proxy_output(
    mut receiver: mpsc::Receiver<OutboundMessage>,
) -> Result<()> {
    let mut stdout = BufWriter::new(tokio::io::stdout());
    while let Some(message) = receiver.recv().await {
        write_message_async(&mut stdout, &message.value, message.framing).await?;
    }
    stdout.shutdown().await?;
    Ok(())
}

pub(crate) struct UpstreamPool {
    clients: BTreeMap<String, Arc<UpstreamClient>>,
}

impl UpstreamPool {
    pub(crate) fn values(&self) -> impl Iterator<Item = &Arc<UpstreamClient>> {
        self.clients.values()
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Arc<UpstreamClient>> {
        self.clients.get(name)
    }

    pub(crate) async fn shutdown(&self) {
        for client in self.clients.values() {
            client.shutdown().await;
        }
    }

    pub(crate) async fn forward_client_response(&self, response: &Value) -> Result<bool> {
        for client in self.clients.values() {
            if client.forward_client_response(response).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Drop for UpstreamPool {
    fn drop(&mut self) {
        for client in self.clients.values() {
            client.request_shutdown();
        }
    }
}

pub(crate) struct UpstreamClient {
    pub(crate) name: String,
    stdin: AsyncMutex<ChildStdin>,
    pending: Mutex<HashMap<String, oneshot::Sender<PendingReply>>>,
    reverse_pending: Mutex<HashMap<String, Value>>,
    next_reverse_id: AtomicU64,
    inflight: Arc<Semaphore>,
    pub(crate) request_timeout: Duration,
    pub(crate) command_preview: String,
    pub(crate) compact_tools: Vec<String>,
    shutdown: watch::Sender<bool>,
    exit_reason: Mutex<Option<String>>,
    reaped: AtomicBool,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl UpstreamClient {
    pub(crate) async fn send_message(&self, request: &Value) -> Result<()> {
        let deadline = Instant::now() + self.request_timeout;
        self.write_before(deadline, request).await
    }

    pub(crate) async fn send_request(&self, request: &Value) -> Result<Value> {
        let request_id = request
            .get("id")
            .ok_or_else(|| anyhow!("upstream request is missing id"))?;
        let request_key = request_key(request_id)?;
        let deadline = Instant::now() + self.request_timeout;
        let permit = timeout_at(deadline, self.inflight.clone().acquire_owned())
            .await
            .map_err(|_| self.timeout_error())?
            .map_err(|_| anyhow!("upstream '{}' is shutting down", self.name))?;
        if let Some(reason) = self.exit_reason()? {
            return Err(anyhow!(reason));
        }

        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| anyhow!("failed to lock upstream '{}' pending map", self.name))?;
            if pending.contains_key(&request_key) {
                return Err(anyhow!(
                    "duplicate in-flight request id {} for upstream '{}'",
                    request_id,
                    self.name
                ));
            }
            pending.insert(request_key.clone(), sender);
        }

        if let Err(error) = self.write_before(deadline, request).await {
            self.remove_pending(&request_key);
            return Err(error);
        }
        let reply = match timeout_at(deadline, receiver).await {
            Ok(Ok(reply)) => reply.map_err(anyhow::Error::msg),
            Ok(Err(_)) => Err(anyhow!(
                "upstream '{}' exited before response id {}",
                self.name,
                request_id
            )),
            Err(_) => {
                self.remove_pending(&request_key);
                Err(self.timeout_error())
            }
        };
        drop(permit);
        reply
    }

    async fn write_before(&self, deadline: Instant, request: &Value) -> Result<()> {
        let write = async {
            let mut stdin = self.stdin.lock().await;
            write_message_async(&mut *stdin, request, McpMessageFraming::ContentLength).await
        };
        timeout_at(deadline, write)
            .await
            .map_err(|_| self.timeout_error())?
    }

    fn namespace_server_request(&self, mut request: Value) -> Result<Value> {
        let original_id = request
            .get("id")
            .cloned()
            .ok_or_else(|| anyhow!("upstream server request is missing id"))?;
        let sequence = self.next_reverse_id.fetch_add(1, Ordering::Relaxed);
        let proxy_id = Value::String(format!(
            "packet28-upstream:{}:{}",
            self.name,
            sequence.saturating_add(1)
        ));
        let key = request_key(&proxy_id)?;
        self.reverse_pending
            .lock()
            .map_err(|_| {
                anyhow!(
                    "failed to lock upstream '{}' reverse request map",
                    self.name
                )
            })?
            .insert(key, original_id);
        request
            .as_object_mut()
            .ok_or_else(|| anyhow!("upstream server request must be an object"))?
            .insert("id".to_string(), proxy_id);
        Ok(request)
    }

    async fn forward_client_response(&self, response: &Value) -> Result<bool> {
        let Some(proxy_id) = response.get("id") else {
            return Ok(false);
        };
        let key = request_key(proxy_id)?;
        let original_id = self
            .reverse_pending
            .lock()
            .map_err(|_| {
                anyhow!(
                    "failed to lock upstream '{}' reverse request map",
                    self.name
                )
            })?
            .remove(&key);
        let Some(original_id) = original_id else {
            return Ok(false);
        };
        let mut forwarded = response.clone();
        forwarded
            .as_object_mut()
            .ok_or_else(|| anyhow!("downstream MCP response must be an object"))?
            .insert("id".to_string(), original_id);
        self.send_message(&forwarded).await?;
        Ok(true)
    }

    fn timeout_error(&self) -> anyhow::Error {
        anyhow!(
            "timed out waiting for upstream '{}' after {}ms while running `{}`",
            self.name,
            self.request_timeout.as_millis(),
            self.command_preview
        )
    }

    fn exit_reason(&self) -> Result<Option<String>> {
        self.exit_reason
            .lock()
            .map(|reason| reason.clone())
            .map_err(|_| anyhow!("failed to lock upstream '{}' exit state", self.name))
    }

    fn set_exit_reason(&self, reason: String) {
        if let Ok(mut guard) = self.exit_reason.lock() {
            guard.get_or_insert_with(|| reason.clone());
        }
        self.fail_pending(reason);
    }

    fn fail_pending(&self, reason: String) {
        let pending = self
            .pending
            .lock()
            .map(|mut pending| {
                pending
                    .drain()
                    .map(|(_, sender)| sender)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for sender in pending {
            let _ = sender.send(Err(reason.clone()));
        }
    }

    fn remove_pending(&self, request_key: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(request_key);
        }
    }

    fn request_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    async fn shutdown(&self) {
        self.request_shutdown();
        let tasks = self
            .tasks
            .lock()
            .map(|mut tasks| std::mem::take(&mut *tasks))
            .unwrap_or_default();
        for task in tasks {
            let _ = task.await;
        }
    }

    #[cfg(test)]
    fn is_reaped(&self) -> bool {
        self.reaped.load(Ordering::Acquire)
    }
}

pub(crate) async fn spawn_upstream_clients(
    root: &Path,
    config: &McpProxyConfig,
    output: ProxyOutput,
    session: Arc<Mutex<McpSessionState>>,
) -> Result<Arc<UpstreamPool>> {
    let mut clients = BTreeMap::<String, Arc<UpstreamClient>>::new();
    for (name, server) in &config.mcp_servers {
        let client =
            match spawn_upstream_client(root, name, server, output.clone(), session.clone())
                .await
                .with_context(|| format!("failed to start upstream MCP server '{name}'"))
            {
                Ok(client) => client,
                Err(error) => {
                    for client in clients.values() {
                        client.shutdown().await;
                    }
                    return Err(error);
                }
            };
        clients.insert(name.clone(), client);
    }
    Ok(Arc::new(UpstreamPool { clients }))
}

async fn spawn_upstream_client(
    root: &Path,
    name: &str,
    server: &super::config::McpProxyServerConfig,
    output: ProxyOutput,
    session: Arc<Mutex<McpSessionState>>,
) -> Result<Arc<UpstreamClient>> {
    let mut command = Command::new(&server.command);
    command
        .args(&server.args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true);
    if let Some(cwd) = &server.cwd {
        command.current_dir(cwd);
    } else {
        command.current_dir(root);
    }
    command.envs(server.env.clone());
    let command_preview = render_command_preview(&server.command, &server.args);
    let timeout = Duration::from_millis(
        server
            .timeout_ms
            .unwrap_or(DEFAULT_UPSTREAM_TIMEOUT_MS)
            .max(1),
    );
    let mut child = command.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("upstream MCP server '{name}' has no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("upstream MCP server '{name}' has no stdout"))?;
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let client = Arc::new(UpstreamClient {
        name: name.to_string(),
        stdin: AsyncMutex::new(stdin),
        pending: Mutex::new(HashMap::new()),
        reverse_pending: Mutex::new(HashMap::new()),
        next_reverse_id: AtomicU64::new(0),
        inflight: Arc::new(Semaphore::new(MAX_UPSTREAM_INFLIGHT)),
        request_timeout: timeout,
        command_preview,
        compact_tools: server.compact_tools.clone(),
        shutdown,
        exit_reason: Mutex::new(None),
        reaped: AtomicBool::new(false),
        tasks: Mutex::new(Vec::new()),
    });
    let reader = tokio::spawn(read_upstream(client.clone(), stdout, output, session));
    let monitor = tokio::spawn(monitor_child(client.clone(), child, shutdown_receiver));
    client
        .tasks
        .lock()
        .map_err(|_| anyhow!("failed to lock upstream '{name}' task set"))?
        .extend([reader, monitor]);
    Ok(client)
}

async fn read_upstream(
    client: Arc<UpstreamClient>,
    stdout: tokio::process::ChildStdout,
    output: ProxyOutput,
    session: Arc<Mutex<McpSessionState>>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let message = match read_message_async(&mut reader).await {
            Ok(Some((message, _))) => message,
            Ok(None) => break,
            Err(error) => {
                client.set_exit_reason(format!(
                    "failed to read upstream '{}': {error}",
                    client.name
                ));
                client.request_shutdown();
                return;
            }
        };
        if message.get("method").is_none() {
            let Some(id) = message.get("id") else {
                continue;
            };
            if let Ok(key) = request_key(id) {
                let sender = client
                    .pending
                    .lock()
                    .ok()
                    .and_then(|mut pending| pending.remove(&key));
                if let Some(sender) = sender {
                    let _ = sender.send(Ok(message));
                }
            }
            continue;
        }

        let framing = session
            .lock()
            .ok()
            .and_then(|guard| guard.framing)
            .unwrap_or(McpMessageFraming::ContentLength);
        let forwarded = if message.get("id").is_some() {
            match client.namespace_server_request(message) {
                Ok(request) => request,
                Err(error) => {
                    client.set_exit_reason(format!(
                        "invalid server request from upstream '{}': {error}",
                        client.name
                    ));
                    client.request_shutdown();
                    return;
                }
            }
        } else {
            let mut notification = message;
            if let Some(params) = notification
                .get_mut("params")
                .and_then(Value::as_object_mut)
            {
                params
                    .entry("upstream".to_string())
                    .or_insert_with(|| Value::String(client.name.clone()));
            }
            notification
        };
        if output.send(forwarded, framing).await.is_err() {
            break;
        }
    }
    client.set_exit_reason(format!(
        "upstream '{}' exited before completing pending requests",
        client.name
    ));
    client.request_shutdown();
}

async fn monitor_child(
    client: Arc<UpstreamClient>,
    mut child: tokio::process::Child,
    mut shutdown: watch::Receiver<bool>,
) {
    let status = tokio::select! {
        status = child.wait() => status,
        changed = shutdown.changed() => {
            if changed.is_ok() && *shutdown.borrow() {
                let _ = child.kill().await;
            }
            child.wait().await
        }
    };
    client.reaped.store(true, Ordering::Release);
    client.set_exit_reason(render_exit_reason(&client.name, status));
}

fn render_exit_reason(name: &str, status: std::io::Result<ExitStatus>) -> String {
    match status {
        Ok(status) => format!("upstream '{name}' exited with status {status}"),
        Err(error) => format!("failed to reap upstream '{name}': {error}"),
    }
}

fn request_key(id: &Value) -> Result<String> {
    serde_json::to_string(id).context("failed to serialize upstream request id")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    const CONCURRENT_SERVER: &str = r#"
import json, os, sys, threading, time

WRITE_LOCK = threading.Lock()

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    return json.loads(sys.stdin.buffer.read(length))

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    with WRITE_LOCK:
        sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
        sys.stdout.buffer.write(body)
        sys.stdout.buffer.flush()

def respond(message):
    arguments = message.get("params", {}).get("arguments", {})
    mode = arguments.get("mode", "reply")
    if mode == "exit":
        os._exit(17)
    if mode == "oversized":
        with WRITE_LOCK:
            sys.stdout.buffer.write(b"Content-Length: 8388609\r\n\r\n")
            sys.stdout.buffer.flush()
        time.sleep(10)
        return
    time.sleep(arguments.get("delay_ms", 0) / 1000.0)
    write_message({
        "jsonrpc": "2.0",
        "id": message["id"],
        "result": {"value": arguments.get("value")}
    })

while True:
    message = read_message()
    if message is None:
        break
    if message.get("id") is not None:
        threading.Thread(target=respond, args=(message,), daemon=True).start()
"#;

    #[test]
    fn request_key_preserves_json_id_type() {
        assert_ne!(
            request_key(&json!(1)).unwrap(),
            request_key(&json!("1")).unwrap()
        );
    }

    fn write_test_server() -> (TempDir, super::super::config::McpProxyServerConfig) {
        let directory = TempDir::new().unwrap();
        let script = directory.path().join("concurrent_mcp.py");
        fs::write(&script, CONCURRENT_SERVER).unwrap();
        (
            directory,
            super::super::config::McpProxyServerConfig {
                command: "python3".to_string(),
                args: vec!["-u".to_string(), script.display().to_string()],
                timeout_ms: Some(1_000),
                ..super::super::config::McpProxyServerConfig::default()
            },
        )
    }

    async fn test_client(
        timeout_ms: u64,
    ) -> (
        TempDir,
        Arc<UpstreamClient>,
        mpsc::Receiver<OutboundMessage>,
    ) {
        let (directory, mut config) = write_test_server();
        config.timeout_ms = Some(timeout_ms);
        let (output, receiver) = proxy_output_channel();
        let client = spawn_upstream_client(
            directory.path(),
            "test",
            &config,
            output,
            Arc::new(Mutex::new(McpSessionState::default())),
        )
        .await
        .unwrap();
        (directory, client, receiver)
    }

    fn request(id: Value, delay_ms: u64, value: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "test.echo",
                "arguments": {
                    "delay_ms": delay_ms,
                    "value": value
                }
            }
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn routes_out_of_order_responses_by_json_rpc_id() {
        let (_directory, client, _output) = test_client(1_000).await;
        let mut tasks = tokio::task::JoinSet::new();
        let slow_client = client.clone();
        tasks.spawn(async move {
            slow_client
                .send_request(&request(json!("slow-id"), 150, "slow"))
                .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let fast_client = client.clone();
        tasks.spawn(async move {
            fast_client
                .send_request(&request(json!(2), 5, "fast"))
                .await
        });

        let first = tasks.join_next().await.unwrap().unwrap().unwrap();
        let second = tasks.join_next().await.unwrap().unwrap().unwrap();
        assert_eq!(first["id"], json!(2));
        assert_eq!(first["result"]["value"], "fast");
        assert_eq!(second["id"], "slow-id");
        assert_eq!(second["result"]["value"], "slow");
        client.shutdown().await;
        assert!(client.is_reaped());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn discards_late_response_after_timeout_without_poisoning_next_request() {
        let (_directory, client, _output) = test_client(300).await;
        let timed_out = client
            .send_request(&request(json!(10), 600, "late"))
            .await
            .unwrap_err();
        assert!(timed_out.to_string().contains("300ms"));

        let response = client
            .send_request(&request(json!(11), 5, "next"))
            .await
            .unwrap();
        assert_eq!(response["id"], 11);
        assert_eq!(response["result"]["value"], "next");

        tokio::time::sleep(Duration::from_millis(350)).await;
        let after_late = client
            .send_request(&request(json!("after-late"), 5, "still-correct"))
            .await
            .unwrap();
        assert_eq!(after_late["id"], "after-late");
        assert_eq!(after_late["result"]["value"], "still-correct");
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn child_exit_fails_pending_request_and_is_reaped() {
        let (_directory, client, _output) = test_client(1_000).await;
        let error = client
            .send_request(&json!({
                "jsonrpc": "2.0",
                "id": 20,
                "method": "tools/call",
                "params": {
                    "name": "test.echo",
                    "arguments": {"mode": "exit"}
                }
            }))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("exited"),
            "unexpected error: {error:#}"
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while !client.is_reaped() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("upstream child was not reaped");
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn oversized_upstream_frame_fails_request_before_body_allocation() {
        let (_directory, client, _output) = test_client(1_000).await;
        let error = client
            .send_request(&json!({
                "jsonrpc": "2.0",
                "id": 30,
                "method": "tools/call",
                "params": {
                    "name": "test.echo",
                    "arguments": {"mode": "oversized"}
                }
            }))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("exceeds"),
            "unexpected error: {error:#}"
        );
        client.shutdown().await;
        assert!(client.is_reaped());
    }

    #[tokio::test]
    async fn output_and_upstream_request_queues_are_bounded() {
        let (output, _receiver) = proxy_output_channel();
        for id in 0..MAX_PROXY_OUTPUT_MESSAGES {
            output
                .sender
                .try_send(OutboundMessage {
                    value: json!({"id": id}),
                    framing: McpMessageFraming::ContentLength,
                })
                .unwrap();
        }
        assert!(!output
            .try_send(json!({"id": "overflow"}), McpMessageFraming::ContentLength)
            .unwrap());

        let permits = Arc::new(Semaphore::new(MAX_UPSTREAM_INFLIGHT));
        let mut acquired = Vec::new();
        for _ in 0..MAX_UPSTREAM_INFLIGHT {
            acquired.push(permits.clone().try_acquire_owned().unwrap());
        }
        assert!(permits.clone().try_acquire_owned().is_err());
    }
}
