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
use tokio::sync::{mpsc, oneshot, watch, Mutex as AsyncMutex, Notify, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::{timeout_at, Instant};

use super::config::McpProxyConfig;
use super::transport::{
    read_message_async, render_command_preview, write_message_async, McpMessageFraming,
};
use super::McpSessionState;

const DEFAULT_UPSTREAM_TIMEOUT_MS: u64 = 30_000;
const MAX_UPSTREAM_INFLIGHT: usize = 32;
const MAX_UPSTREAM_REVERSE_REQUESTS: usize = 64;
const MAX_UPSTREAM_REVERSE_ID_BYTES: usize = 64 * 1024;
const JSON_RPC_SERVER_ERROR: i64 = -32000;
pub(super) const MAX_PROXY_OUTPUT_MESSAGES: usize = 64;

type PendingReply = std::result::Result<Value, String>;

struct ReverseRequestEntry {
    original_id: Value,
    original_id_bytes: usize,
    deadline: Instant,
}

#[derive(Default)]
struct ReverseRequestState {
    pending: HashMap<String, ReverseRequestEntry>,
    original_id_bytes: usize,
    closed: bool,
}

enum ReverseRequestDispatch {
    Forward(Value),
    Reply(Value),
}

struct ReverseRequestTracker {
    upstream_name: String,
    proxy_id_prefix: String,
    timeout: Duration,
    max_pending: usize,
    max_original_id_bytes: usize,
    next_id: AtomicU64,
    state: AsyncMutex<ReverseRequestState>,
    changed: Notify,
}

impl ReverseRequestTracker {
    fn new(upstream_name: &str, timeout: Duration) -> Self {
        Self::with_limits(
            upstream_name,
            timeout,
            MAX_UPSTREAM_REVERSE_REQUESTS,
            MAX_UPSTREAM_REVERSE_ID_BYTES,
        )
    }

    fn with_limits(
        upstream_name: &str,
        timeout: Duration,
        max_pending: usize,
        max_original_id_bytes: usize,
    ) -> Self {
        Self {
            upstream_name: upstream_name.to_string(),
            proxy_id_prefix: format!("packet28-upstream:{upstream_name}:"),
            timeout,
            max_pending,
            max_original_id_bytes,
            next_id: AtomicU64::new(0),
            state: AsyncMutex::new(ReverseRequestState::default()),
            changed: Notify::new(),
        }
    }

    async fn namespace(&self, mut request: Value) -> Result<ReverseRequestDispatch> {
        let original_id = request
            .get("id")
            .cloned()
            .ok_or_else(|| anyhow!("upstream server request is missing id"))?;
        if !request.is_object() {
            return Err(anyhow!("upstream server request must be an object"));
        }

        let sequence = self.next_id.fetch_add(1, Ordering::Relaxed);
        let proxy_id = Value::String(format!(
            "{}{}",
            self.proxy_id_prefix,
            sequence.wrapping_add(1)
        ));
        let proxy_key = request_key(&proxy_id)?;
        let original_id_bytes = request_key(&original_id)?.len();
        let deadline = Instant::now() + self.timeout;

        let rejection = {
            let mut state = self.state.lock().await;
            if state.closed {
                Some(format!(
                    "upstream '{}' reverse request tracker is closed",
                    self.upstream_name
                ))
            } else if state.pending.len() >= self.max_pending {
                Some(format!(
                    "upstream '{}' reverse request limit reached ({} pending)",
                    self.upstream_name, self.max_pending
                ))
            } else {
                match state.original_id_bytes.checked_add(original_id_bytes) {
                    Some(total) if total <= self.max_original_id_bytes => {
                        state.original_id_bytes = total;
                        state.pending.insert(
                            proxy_key,
                            ReverseRequestEntry {
                                original_id: original_id.clone(),
                                original_id_bytes,
                                deadline,
                            },
                        );
                        None
                    }
                    _ => Some(format!(
                        "upstream '{}' reverse request id budget exceeded ({} bytes)",
                        self.upstream_name, self.max_original_id_bytes
                    )),
                }
            }
        };

        if let Some(message) = rejection {
            return Ok(ReverseRequestDispatch::Reply(super::mcp_error_response(
                original_id,
                JSON_RPC_SERVER_ERROR,
                &message,
            )));
        }

        request
            .as_object_mut()
            .ok_or_else(|| anyhow!("upstream server request must be an object"))?
            .insert("id".to_string(), proxy_id);
        self.changed.notify_one();
        Ok(ReverseRequestDispatch::Forward(request))
    }

    async fn take(&self, proxy_id: &Value) -> Result<Option<Value>> {
        let proxy_key = request_key(proxy_id)?;
        let original_id = {
            let mut state = self.state.lock().await;
            let entry = state.pending.remove(&proxy_key);
            if let Some(entry) = &entry {
                state.original_id_bytes = state
                    .original_id_bytes
                    .saturating_sub(entry.original_id_bytes);
            }
            entry.map(|entry| entry.original_id)
        };
        if original_id.is_some() {
            self.changed.notify_one();
        }
        Ok(original_id)
    }

    async fn wait_for_expired(&self) -> Vec<Value> {
        loop {
            let changed = self.changed.notified();
            let next_deadline = {
                let state = self.state.lock().await;
                state.pending.values().map(|entry| entry.deadline).min()
            };
            match next_deadline {
                Some(deadline) => {
                    tokio::select! {
                        () = tokio::time::sleep_until(deadline) => {
                            let expired = self.expire(Instant::now()).await;
                            if !expired.is_empty() {
                                return expired;
                            }
                        }
                        () = changed => {}
                    }
                }
                None => changed.await,
            }
        }
    }

    async fn expire(&self, now: Instant) -> Vec<Value> {
        let expired = {
            let mut state = self.state.lock().await;
            let keys = state
                .pending
                .iter()
                .filter_map(|(key, entry)| (entry.deadline <= now).then_some(key.clone()))
                .collect::<Vec<_>>();
            let mut expired = Vec::with_capacity(keys.len());
            for key in keys {
                if let Some(entry) = state.pending.remove(&key) {
                    state.original_id_bytes = state
                        .original_id_bytes
                        .saturating_sub(entry.original_id_bytes);
                    expired.push(entry.original_id);
                }
            }
            expired
        };
        if !expired.is_empty() {
            self.changed.notify_one();
        }
        expired
    }

    async fn drain(&self) -> Vec<Value> {
        let drained = {
            let mut state = self.state.lock().await;
            state.closed = true;
            state.original_id_bytes = 0;
            std::mem::take(&mut state.pending)
                .into_values()
                .map(|entry| entry.original_id)
                .collect()
        };
        self.changed.notify_one();
        drained
    }

    fn timeout_response(&self, original_id: Value) -> Value {
        super::mcp_error_response(
            original_id,
            JSON_RPC_SERVER_ERROR,
            &format!(
                "timed out after {}ms waiting for the MCP client to answer upstream '{}' reverse request",
                self.timeout.as_millis(),
                self.upstream_name
            ),
        )
    }

    fn owns_proxy_id(&self, proxy_id: &Value) -> bool {
        proxy_id
            .as_str()
            .and_then(|id| id.strip_prefix(&self.proxy_id_prefix))
            .is_some_and(|sequence| sequence.parse::<u64>().is_ok())
    }

    #[cfg(test)]
    async fn len(&self) -> usize {
        self.state.lock().await.pending.len()
    }

    #[cfg(test)]
    async fn original_id_bytes(&self) -> usize {
        self.state.lock().await.original_id_bytes
    }

    #[cfg(test)]
    async fn is_closed(&self) -> bool {
        self.state.lock().await.closed
    }
}

enum UpstreamMessageDispatch {
    Routed,
    Forward(Value),
    Reply(Value),
}

struct UpstreamPayloadDispatch {
    forwarded: Option<Value>,
    reply: Option<Value>,
}

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
    reverse_requests: ReverseRequestTracker,
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

    async fn dispatch_upstream_payload(&self, payload: Value) -> Result<UpstreamPayloadDispatch> {
        let Value::Array(messages) = payload else {
            return match self.dispatch_upstream_message(payload).await? {
                UpstreamMessageDispatch::Routed => Ok(UpstreamPayloadDispatch {
                    forwarded: None,
                    reply: None,
                }),
                UpstreamMessageDispatch::Forward(message) => Ok(UpstreamPayloadDispatch {
                    forwarded: Some(message),
                    reply: None,
                }),
                UpstreamMessageDispatch::Reply(response) => Ok(UpstreamPayloadDispatch {
                    forwarded: None,
                    reply: Some(response),
                }),
            };
        };
        if messages.is_empty() {
            return Ok(UpstreamPayloadDispatch {
                forwarded: None,
                reply: Some(super::mcp_error_response(
                    Value::Null,
                    -32600,
                    "empty JSON-RPC batch",
                )),
            });
        }

        let mut forwarded = Vec::new();
        let mut replies = Vec::new();
        for message in messages {
            match self.dispatch_upstream_message(message).await? {
                UpstreamMessageDispatch::Routed => {}
                UpstreamMessageDispatch::Forward(message) => forwarded.push(message),
                UpstreamMessageDispatch::Reply(response) => replies.push(response),
            }
        }
        Ok(UpstreamPayloadDispatch {
            forwarded: (!forwarded.is_empty()).then_some(Value::Array(forwarded)),
            reply: (!replies.is_empty()).then_some(Value::Array(replies)),
        })
    }

    async fn dispatch_upstream_message(&self, message: Value) -> Result<UpstreamMessageDispatch> {
        let Some(object) = message.as_object() else {
            return Ok(UpstreamMessageDispatch::Reply(super::mcp_error_response(
                Value::Null,
                -32600,
                "JSON-RPC upstream message must be an object",
            )));
        };
        let is_response = !object.contains_key("method");

        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return self.invalid_upstream_message(
                object,
                is_response,
                "upstream JSON-RPC message must declare jsonrpc \"2.0\"",
            );
        }
        if object.get("id").is_some_and(|id| !is_valid_json_rpc_id(id)) {
            return self.invalid_upstream_message(
                object,
                is_response,
                "upstream JSON-RPC id must be a string, number, or null",
            );
        }
        if object
            .get("params")
            .is_some_and(|params| !params.is_object() && !params.is_array())
        {
            return self.invalid_upstream_message(
                object,
                is_response,
                "upstream JSON-RPC params must be an object or array",
            );
        }

        if is_response {
            let Some(id) = object.get("id") else {
                return self.invalid_upstream_message(
                    object,
                    true,
                    "upstream JSON-RPC message is missing method and id",
                );
            };
            if object.contains_key("result") == object.contains_key("error") {
                return self.invalid_upstream_message(
                    object,
                    true,
                    "upstream JSON-RPC response must contain exactly one of result or error",
                );
            }
            if object
                .get("error")
                .is_some_and(|error| !is_valid_json_rpc_error(error))
            {
                return self.invalid_upstream_message(
                    object,
                    true,
                    "upstream JSON-RPC error must contain an integer code and string message",
                );
            }
            let sender = self.take_pending_response(id)?;
            if let Some(sender) = sender {
                let _ = sender.send(Ok(message));
            }
            return Ok(UpstreamMessageDispatch::Routed);
        }

        if !object.get("method").is_some_and(Value::is_string) {
            return self.invalid_upstream_message(
                object,
                false,
                "upstream JSON-RPC method must be a string",
            );
        }
        if object.contains_key("result") || object.contains_key("error") {
            return self.invalid_upstream_message(
                object,
                false,
                "upstream JSON-RPC request must not contain result or error",
            );
        }

        if object.get("id").is_some() {
            return Ok(match self.reverse_requests.namespace(message).await? {
                ReverseRequestDispatch::Forward(request) => {
                    UpstreamMessageDispatch::Forward(request)
                }
                ReverseRequestDispatch::Reply(response) => UpstreamMessageDispatch::Reply(response),
            });
        }

        let mut notification = message;
        if let Some(params) = notification
            .get_mut("params")
            .and_then(Value::as_object_mut)
        {
            params
                .entry("upstream".to_string())
                .or_insert_with(|| Value::String(self.name.clone()));
        }
        Ok(UpstreamMessageDispatch::Forward(notification))
    }

    fn invalid_upstream_message(
        &self,
        object: &serde_json::Map<String, Value>,
        is_response: bool,
        message: &str,
    ) -> Result<UpstreamMessageDispatch> {
        let id = object.get("id");
        if is_response {
            if let Some(sender) = id
                .map(|id| self.take_pending_response(id))
                .transpose()?
                .flatten()
            {
                let _ = sender.send(Err(format!(
                    "invalid response from upstream '{}': {message}",
                    self.name
                )));
            }
        }
        let diagnostic_id = id
            .filter(|id| is_valid_json_rpc_id(id))
            .cloned()
            .unwrap_or(Value::Null);
        Ok(UpstreamMessageDispatch::Reply(super::mcp_error_response(
            diagnostic_id,
            -32600,
            message,
        )))
    }

    fn take_pending_response(&self, id: &Value) -> Result<Option<oneshot::Sender<PendingReply>>> {
        let key = request_key(id)?;
        self.pending
            .lock()
            .map(|mut pending| pending.remove(&key))
            .map_err(|_| anyhow!("failed to lock upstream '{}' pending map", self.name))
    }

    async fn forward_client_response(&self, response: &Value) -> Result<bool> {
        let Some(proxy_id) = response.get("id") else {
            return Ok(false);
        };
        let original_id = self.reverse_requests.take(proxy_id).await?;
        let Some(original_id) = original_id else {
            return Ok(self.reverse_requests.owns_proxy_id(proxy_id));
        };
        let mut forwarded = response.clone();
        forwarded
            .as_object_mut()
            .ok_or_else(|| anyhow!("downstream MCP response must be an object"))?
            .insert("id".to_string(), original_id);
        if let Err(error) = self.send_message(&forwarded).await {
            self.set_exit_reason(format!(
                "failed to forward reverse response to upstream '{}': {error}",
                self.name
            ));
            let _ = self.reverse_requests.drain().await;
        }
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
        self.request_shutdown();
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
        let _ = self.reverse_requests.drain().await;
        let tasks = self
            .tasks
            .lock()
            .map(|mut tasks| std::mem::take(&mut *tasks))
            .unwrap_or_default();
        for task in tasks {
            let _ = task.await;
        }
        let _ = self.reverse_requests.drain().await;
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
    let reader_shutdown = shutdown_receiver.clone();
    let expiration_shutdown = shutdown_receiver.clone();
    let client = Arc::new(UpstreamClient {
        name: name.to_string(),
        stdin: AsyncMutex::new(stdin),
        pending: Mutex::new(HashMap::new()),
        reverse_requests: ReverseRequestTracker::new(name, timeout),
        inflight: Arc::new(Semaphore::new(MAX_UPSTREAM_INFLIGHT)),
        request_timeout: timeout,
        command_preview,
        compact_tools: server.compact_tools.clone(),
        shutdown,
        exit_reason: Mutex::new(None),
        reaped: AtomicBool::new(false),
        tasks: Mutex::new(Vec::new()),
    });
    let reader = tokio::spawn(read_upstream(
        client.clone(),
        stdout,
        output,
        session,
        reader_shutdown,
    ));
    let expiration = tokio::spawn(expire_reverse_requests(client.clone(), expiration_shutdown));
    let monitor = tokio::spawn(monitor_child(client.clone(), child, shutdown_receiver));
    client
        .tasks
        .lock()
        .map_err(|_| anyhow!("failed to lock upstream '{name}' task set"))?
        .extend([reader, expiration, monitor]);
    Ok(client)
}

async fn read_upstream(
    client: Arc<UpstreamClient>,
    stdout: tokio::process::ChildStdout,
    output: ProxyOutput,
    session: Arc<Mutex<McpSessionState>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let next = tokio::select! {
            message = read_message_async(&mut reader) => message,
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
        };
        let message = match next {
            Ok(Some((message, _))) => message,
            Ok(None) => break,
            Err(error) => {
                client.set_exit_reason(format!(
                    "failed to read upstream '{}': {error}",
                    client.name
                ));
                return;
            }
        };
        let dispatch = match client.dispatch_upstream_payload(message).await {
            Ok(dispatch) => dispatch,
            Err(error) => {
                client.set_exit_reason(format!(
                    "invalid payload from upstream '{}': {error}",
                    client.name
                ));
                return;
            }
        };
        if let Some(reply) = dispatch.reply {
            if let Err(error) = client.send_message(&reply).await {
                client.set_exit_reason(format!(
                    "failed to send JSON-RPC error to upstream '{}': {error}",
                    client.name
                ));
                return;
            }
        }
        if let Some(forwarded) = dispatch.forwarded {
            let framing = session
                .lock()
                .ok()
                .and_then(|guard| guard.framing)
                .unwrap_or(McpMessageFraming::ContentLength);
            tokio::select! {
                result = output.send(forwarded, framing) => {
                    if let Err(error) = result {
                        client.set_exit_reason(format!(
                            "failed to forward message from upstream '{}': {error}",
                            client.name
                        ));
                        return;
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
    }
    client.set_exit_reason(format!(
        "upstream '{}' exited before completing pending requests",
        client.name
    ));
}

async fn expire_reverse_requests(client: Arc<UpstreamClient>, mut shutdown: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            expired = client.reverse_requests.wait_for_expired() => {
                for original_id in expired {
                    let response = client.reverse_requests.timeout_response(original_id);
                    if let Err(error) = client.send_message(&response).await {
                        client.set_exit_reason(format!(
                            "failed to expire reverse request for upstream '{}': {error}",
                            client.name
                        ));
                        let _ = client.reverse_requests.drain().await;
                        return;
                    }
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
    }
    let _ = client.reverse_requests.drain().await;
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

fn is_valid_json_rpc_id(id: &Value) -> bool {
    id.is_string() || id.is_number() || id.is_null()
}

fn is_valid_json_rpc_error(error: &Value) -> bool {
    error.as_object().is_some_and(|error| {
        error.get("code").and_then(Value::as_i64).is_some()
            && error.get("message").is_some_and(Value::is_string)
    })
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
    if mode == "malformed":
        write_message({
            "jsonrpc": "1.0",
            "id": message["id"],
            "result": {"value": arguments.get("value")}
        })
        return
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

    const REVERSE_SERVER: &str = r#"
import json, os, sys, time

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
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    if message.get("method") == "test/reverse":
        params = message.get("params", {})
        write_message({
            "jsonrpc": "2.0",
            "id": params.get("id", "server-reverse"),
            "method": "roots/list",
            "params": {}
        })
        if params.get("exit"):
            os._exit(17)
        if params.get("close_stdin"):
            os.close(0)
            write_message({
                "jsonrpc": "2.0",
                "method": "notifications/message",
                "params": {"data": {"stdin_closed": True}}
            })
            while True:
                time.sleep(1)
    elif message.get("method") == "test/fill-output":
        params = message.get("params", {})
        for sequence in range(params["count"]):
            write_message({
                "jsonrpc": "2.0",
                "method": "notifications/message",
                "params": {"data": {"sequence": sequence}}
            })
        with open(params["marker"], "w", encoding="utf-8") as marker:
            marker.write("written")
    elif message.get("id") == "server-reverse" and message.get("error"):
        write_message({
            "jsonrpc": "2.0",
            "method": "notifications/message",
            "params": {
                "data": {
                    "timeout_code": message["error"]["code"]
                }
            }
        })
"#;

    #[test]
    fn request_key_preserves_json_id_type() {
        assert_ne!(
            request_key(&json!(1)).unwrap(),
            request_key(&json!("1")).unwrap()
        );
    }

    fn reverse_request(id: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "roots/list",
            "params": {}
        })
    }

    fn forwarded_proxy_id(dispatch: ReverseRequestDispatch) -> Value {
        match dispatch {
            ReverseRequestDispatch::Forward(request) => request["id"].clone(),
            ReverseRequestDispatch::Reply(response) => {
                panic!("expected forwarded reverse request, got {response}")
            }
        }
    }

    #[tokio::test]
    async fn reverse_tracker_rejects_limit_plus_one_with_json_rpc_error() {
        let tracker =
            ReverseRequestTracker::with_limits("bounded", Duration::from_secs(30), 2, 1_024);
        for id in 1..=2 {
            let dispatch = tracker.namespace(reverse_request(json!(id))).await.unwrap();
            let _ = forwarded_proxy_id(dispatch);
        }

        let rejection = tracker.namespace(reverse_request(json!(3))).await.unwrap();
        let ReverseRequestDispatch::Reply(rejection) = rejection else {
            panic!("limit + 1 reverse request was forwarded")
        };
        assert_eq!(
            (
                rejection["id"].clone(),
                rejection["error"]["code"].clone(),
                tracker.len().await,
            ),
            (json!(3), json!(JSON_RPC_SERVER_ERROR), 2)
        );
    }

    #[tokio::test]
    async fn reverse_tracker_rejects_original_id_byte_budget_overflow() {
        let first_id = json!("first");
        let id_budget = request_key(&first_id).unwrap().len();
        let tracker =
            ReverseRequestTracker::with_limits("bounded", Duration::from_secs(30), 2, id_budget);
        let dispatch = tracker.namespace(reverse_request(first_id)).await.unwrap();
        let _ = forwarded_proxy_id(dispatch);

        let rejection = tracker
            .namespace(reverse_request(json!("second")))
            .await
            .unwrap();
        let ReverseRequestDispatch::Reply(rejection) = rejection else {
            panic!("reverse request exceeding the id byte budget was forwarded")
        };
        assert_eq!(
            (
                rejection["id"].clone(),
                rejection["error"]["code"].clone(),
                rejection["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("id budget exceeded")),
                tracker.original_id_bytes().await,
            ),
            (
                json!("second"),
                json!(JSON_RPC_SERVER_ERROR),
                true,
                id_budget,
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reverse_tracker_expires_at_deadline_on_tokio_clock() {
        let tracker = Arc::new(ReverseRequestTracker::with_limits(
            "clock",
            Duration::from_secs(5),
            2,
            1_024,
        ));
        let dispatch = tracker
            .namespace(reverse_request(json!("deadline")))
            .await
            .unwrap();
        let _ = forwarded_proxy_id(dispatch);
        let waiter_tracker = tracker.clone();
        let waiter = tokio::spawn(async move { waiter_tracker.wait_for_expired().await });
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_millis(4_999)).await;
        assert!(!waiter.is_finished());
        tokio::time::advance(Duration::from_millis(1)).await;
        let expired = waiter.await.unwrap();
        let timeout = tracker.timeout_response(expired[0].clone());
        assert_eq!(
            (
                expired,
                timeout["id"].clone(),
                timeout["error"]["code"].clone(),
                tracker.len().await,
            ),
            (
                vec![json!("deadline")],
                json!("deadline"),
                json!(JSON_RPC_SERVER_ERROR),
                0,
            )
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reverse_tracker_response_and_expiry_race_has_one_winner() {
        let tracker = Arc::new(ReverseRequestTracker::with_limits(
            "race",
            Duration::ZERO,
            2,
            1_024,
        ));
        let proxy_id = forwarded_proxy_id(
            tracker
                .namespace(reverse_request(json!("original")))
                .await
                .unwrap(),
        );
        let barrier = Arc::new(tokio::sync::Barrier::new(3));

        let take_tracker = tracker.clone();
        let take_barrier = barrier.clone();
        let take = tokio::spawn(async move {
            take_barrier.wait().await;
            take_tracker.take(&proxy_id).await.unwrap()
        });
        let expire_tracker = tracker.clone();
        let expire_barrier = barrier.clone();
        let expire = tokio::spawn(async move {
            expire_barrier.wait().await;
            expire_tracker.expire(Instant::now()).await
        });
        barrier.wait().await;

        let taken = take.await.unwrap();
        let expired = expire.await.unwrap();
        assert_eq!(
            (
                usize::from(taken.is_some()) + expired.len(),
                tracker.len().await,
                tracker.original_id_bytes().await,
            ),
            (1, 0, 0)
        );
    }

    #[tokio::test]
    async fn reverse_tracker_drain_invalidates_late_response() {
        let tracker =
            ReverseRequestTracker::with_limits("drain", Duration::from_secs(30), 2, 1_024);
        let proxy_id = forwarded_proxy_id(
            tracker
                .namespace(reverse_request(json!("original")))
                .await
                .unwrap(),
        );

        let drained = tracker.drain().await;
        let late = tracker.take(&proxy_id).await.unwrap();
        let after_close = tracker
            .namespace(reverse_request(json!("after-close")))
            .await
            .unwrap();
        let ReverseRequestDispatch::Reply(after_close) = after_close else {
            panic!("closed tracker accepted another reverse request")
        };
        assert_eq!(
            (
                drained,
                late,
                tracker.owns_proxy_id(&proxy_id),
                tracker.len().await,
                tracker.original_id_bytes().await,
                tracker.is_closed().await,
                after_close["error"]["code"].clone(),
            ),
            (
                vec![json!("original")],
                None,
                true,
                0,
                0,
                true,
                json!(JSON_RPC_SERVER_ERROR),
            )
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reverse_tracker_drain_and_registration_race_leaves_no_pending_request() {
        let tracker = Arc::new(ReverseRequestTracker::with_limits(
            "drain-race",
            Duration::from_secs(30),
            2,
            1_024,
        ));
        let barrier = Arc::new(tokio::sync::Barrier::new(3));

        let namespace_tracker = tracker.clone();
        let namespace_barrier = barrier.clone();
        let namespace = tokio::spawn(async move {
            namespace_barrier.wait().await;
            namespace_tracker
                .namespace(reverse_request(json!("racing")))
                .await
                .unwrap()
        });
        let drain_tracker = tracker.clone();
        let drain_barrier = barrier.clone();
        let drain = tokio::spawn(async move {
            drain_barrier.wait().await;
            drain_tracker.drain().await
        });
        barrier.wait().await;

        let dispatch = namespace.await.unwrap();
        let drained = drain.await.unwrap();
        let coherent_outcome = match dispatch {
            ReverseRequestDispatch::Forward(_) => drained == vec![json!("racing")],
            ReverseRequestDispatch::Reply(response) => {
                drained.is_empty() && response["error"]["code"] == JSON_RPC_SERVER_ERROR
            }
        };
        assert_eq!(
            (
                coherent_outcome,
                tracker.is_closed().await,
                tracker.len().await,
                tracker.original_id_bytes().await,
            ),
            (true, true, 0, 0)
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

    async fn reverse_test_client(
        timeout_ms: u64,
    ) -> (
        TempDir,
        Arc<UpstreamClient>,
        mpsc::Receiver<OutboundMessage>,
    ) {
        let directory = TempDir::new().unwrap();
        let script = directory.path().join("reverse_mcp.py");
        fs::write(&script, REVERSE_SERVER).unwrap();
        let config = super::super::config::McpProxyServerConfig {
            command: "python3".to_string(),
            args: vec!["-u".to_string(), script.display().to_string()],
            timeout_ms: Some(timeout_ms),
            ..super::super::config::McpProxyServerConfig::default()
        };
        let (output, receiver) = proxy_output_channel();
        let client = spawn_upstream_client(
            directory.path(),
            "reverse",
            &config,
            output,
            Arc::new(Mutex::new(McpSessionState::default())),
        )
        .await
        .unwrap();
        (directory, client, receiver)
    }

    async fn wait_until_reaped_and_reverse_requests_drained(client: &UpstreamClient) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if client.is_reaped() && client.reverse_requests.len().await == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("upstream was not reaped and drained");
    }

    fn trigger_reverse_request(exit: bool) -> Value {
        json!({
            "jsonrpc": "2.0",
            "method": "test/reverse",
            "params": {
                "id": "server-reverse",
                "exit": exit
            }
        })
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
    async fn diagnoses_invalid_upstream_json_rpc_members() {
        let (_directory, client, _output) = test_client(1_000).await;
        let invalid = json!([
            {"id": "missing-version", "result": {}},
            {"jsonrpc": "1.0", "id": "wrong-version", "result": {}},
            {"jsonrpc": "2.0", "id": true, "result": {}},
            {"jsonrpc": "2.0", "id": [], "result": {}},
            {"jsonrpc": "2.0", "id": {}, "result": {}},
            {
                "jsonrpc": "2.0",
                "id": "bad-error",
                "error": {"code": "not-an-integer", "message": 17}
            },
            {
                "jsonrpc": "2.0",
                "id": "bad-params",
                "method": "roots/list",
                "params": 17
            },
            {
                "jsonrpc": "2.0",
                "id": "request-with-result",
                "method": "roots/list",
                "result": {}
            }
        ]);

        let dispatch = client.dispatch_upstream_payload(invalid).await.unwrap();
        assert!(dispatch.forwarded.is_none());
        let replies = dispatch.reply.unwrap();
        let replies = replies.as_array().unwrap();
        assert_eq!(replies.len(), 8);
        assert!(replies.iter().all(|reply| reply["error"]["code"] == -32600));
        assert_eq!(replies[0]["id"], "missing-version");
        assert_eq!(replies[1]["id"], "wrong-version");
        assert!(replies[2..5].iter().all(|reply| reply["id"].is_null()));
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn malformed_live_response_fails_waiter_without_request_timeout() {
        let (_directory, client, _output) = test_client(10_000).await;
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            client.send_request(&json!({
                "jsonrpc": "2.0",
                "id": "malformed-live",
                "method": "tools/call",
                "params": {
                    "name": "test.echo",
                    "arguments": {
                        "mode": "malformed",
                        "value": "invalid"
                    }
                }
            })),
        )
        .await
        .expect("malformed response left the live request pending")
        .unwrap_err();
        assert!(
            error.to_string().contains("must declare jsonrpc \"2.0\""),
            "unexpected error: {error:#}"
        );
        client.shutdown().await;
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
    async fn child_exit_drains_reverse_request_and_consumes_late_response() {
        let (_directory, client, mut output) = reverse_test_client(1_000).await;
        client
            .send_message(&trigger_reverse_request(true))
            .await
            .unwrap();
        let forwarded = tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap()
            .unwrap()
            .value;
        let proxy_id = forwarded["id"].clone();

        wait_until_reaped_and_reverse_requests_drained(&client).await;
        let late_was_consumed = client
            .forward_client_response(&json!({
                "jsonrpc": "2.0",
                "id": proxy_id,
                "result": {"roots": []}
            }))
            .await
            .unwrap();
        assert!(late_was_consumed);
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn reverse_request_timeout_sends_json_rpc_error_upstream() {
        let (_directory, client, mut output) = reverse_test_client(50).await;
        client
            .send_message(&trigger_reverse_request(false))
            .await
            .unwrap();
        let _forwarded = tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap()
            .unwrap();
        let acknowledgement = tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap()
            .unwrap()
            .value;

        assert_eq!(
            (
                acknowledgement["method"].clone(),
                acknowledgement["params"]["data"]["timeout_code"].clone(),
                client.reverse_requests.len().await,
            ),
            (
                json!("notifications/message"),
                json!(JSON_RPC_SERVER_ERROR),
                0,
            )
        );
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn explicit_shutdown_drains_reverse_request_and_consumes_late_response() {
        let (_directory, client, mut output) = reverse_test_client(1_000).await;
        client
            .send_message(&trigger_reverse_request(false))
            .await
            .unwrap();
        let forwarded = tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap()
            .unwrap()
            .value;
        let proxy_id = forwarded["id"].clone();
        assert_eq!(client.reverse_requests.len().await, 1);

        client.shutdown().await;
        let late_was_consumed = client
            .forward_client_response(&json!({
                "jsonrpc": "2.0",
                "id": proxy_id,
                "result": {"roots": []}
            }))
            .await
            .unwrap();
        assert_eq!(
            (
                client.reverse_requests.len().await,
                late_was_consumed,
                client.is_reaped(),
            ),
            (0, true, true)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn reverse_response_write_failure_is_consumed_and_closes_upstream() {
        let (_directory, client, mut output) = reverse_test_client(1_000).await;
        client
            .send_message(&json!({
                "jsonrpc": "2.0",
                "method": "test/reverse",
                "params": {
                    "id": "server-reverse",
                    "close_stdin": true
                }
            }))
            .await
            .unwrap();
        let forwarded = tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap()
            .unwrap()
            .value;
        let stdin_closed = tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap()
            .unwrap()
            .value;
        assert_eq!(stdin_closed["params"]["data"]["stdin_closed"], true);

        let handled = tokio::time::timeout(
            Duration::from_secs(2),
            client.forward_client_response(&json!({
                "jsonrpc": "2.0",
                "id": forwarded["id"],
                "result": {"roots": []}
            })),
        )
        .await
        .expect("reverse response write did not terminate")
        .unwrap();
        assert!(handled);
        wait_until_reaped_and_reverse_requests_drained(&client).await;
        assert!(client
            .exit_reason()
            .unwrap()
            .is_some_and(|reason| reason.contains("failed to forward reverse response")));
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn downstream_output_failure_drains_reverse_request() {
        let (_directory, client, output) = reverse_test_client(1_000).await;
        drop(output);
        client
            .send_message(&trigger_reverse_request(false))
            .await
            .unwrap();

        wait_until_reaped_and_reverse_requests_drained(&client).await;
        let late_was_consumed = client
            .forward_client_response(&json!({
                "jsonrpc": "2.0",
                "id": "packet28-upstream:reverse:1",
                "result": {"roots": []}
            }))
            .await
            .unwrap();
        assert!(late_was_consumed);
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn full_downstream_output_queue_does_not_block_shutdown() {
        let (directory, client, output) = reverse_test_client(1_000).await;
        let marker = directory.path().join("output-filled");
        client
            .send_message(&json!({
                "jsonrpc": "2.0",
                "method": "test/fill-output",
                "params": {
                    "count": MAX_PROXY_OUTPUT_MESSAGES + 1,
                    "marker": marker
                }
            }))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if marker.is_file() && output.len() == MAX_PROXY_OUTPUT_MESSAGES {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("upstream did not fill the downstream output queue");

        tokio::time::timeout(Duration::from_secs(1), client.shutdown())
            .await
            .expect("shutdown blocked behind the full downstream output queue");
        assert!(client.is_reaped());
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
