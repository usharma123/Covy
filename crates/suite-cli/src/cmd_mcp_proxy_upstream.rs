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
use super::proxy_catalog::invalidate_resource_catalog;
use super::transport::{
    read_message_async, render_command_preview, write_message_async, McpMessageFraming,
    MAX_MCP_BATCH_MESSAGES, MAX_MCP_MESSAGE_BYTES,
};
use super::McpSessionState;

const DEFAULT_UPSTREAM_TIMEOUT_MS: u64 = 30_000;
const MAX_UPSTREAM_INFLIGHT: usize = 32;
const JSON_RPC_SERVER_ERROR: i64 = -32000;
pub(super) const MAX_PROXY_OUTPUT_MESSAGES: usize = 64;

#[path = "cmd_mcp_proxy_reverse_requests.rs"]
mod reverse_requests;

use reverse_requests::{
    ReverseBatchPlan, ReverseRequestCompletion, ReverseRequestDispatch, ReverseRequestTracker,
};

type PendingReply = std::result::Result<Value, String>;

struct PendingRequest {
    original_id: Value,
    reply: oneshot::Sender<PendingReply>,
}

enum UpstreamMessageDispatch {
    Routed,
    Forward(Value),
    Reply(Value),
}

enum ClassifiedUpstreamMessage {
    Response {
        message: Value,
        id: Value,
    },
    ReverseRequest(Value),
    Notification(Value),
    Invalid {
        response: Option<Value>,
        pending_response_id: Option<Value>,
        reason: &'static str,
    },
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

    pub(crate) fn request_shutdown(&self) {
        for client in self.clients.values() {
            client.request_shutdown();
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
    pending: Mutex<HashMap<String, PendingRequest>>,
    request_id_prefix: String,
    next_request_id: AtomicU64,
    reverse_requests: ReverseRequestTracker,
    inflight: Arc<Semaphore>,
    pub(crate) request_timeout: Duration,
    pub(crate) command_preview: String,
    pub(crate) compact_tools: Vec<String>,
    framing: McpMessageFraming,
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
        let original_id = request
            .get("id")
            .cloned()
            .ok_or_else(|| anyhow!("upstream request is missing id"))?;
        let mut outbound = request.clone();
        let outbound_object = outbound
            .as_object_mut()
            .ok_or_else(|| anyhow!("upstream request must be an object"))?;
        let deadline = Instant::now() + self.request_timeout;
        let permit = timeout_at(deadline, self.inflight.clone().acquire_owned())
            .await
            .map_err(|_| self.timeout_error())?
            .map_err(|_| anyhow!("upstream '{}' is shutting down", self.name))?;
        if let Some(reason) = self.exit_reason()? {
            return Err(anyhow!(reason));
        }

        let (sender, receiver) = oneshot::channel();
        let (proxy_id, request_key) = self.register_pending(original_id.clone(), sender)?;
        outbound_object.insert("id".to_string(), proxy_id);

        if let Err(error) = self.write_before(deadline, &outbound).await {
            self.remove_pending(&request_key);
            return Err(error);
        }
        let reply = match timeout_at(deadline, receiver).await {
            Ok(Ok(reply)) => reply.map_err(anyhow::Error::msg),
            Ok(Err(_)) => Err(anyhow!(
                "upstream '{}' exited before response id {}",
                self.name,
                original_id
            )),
            Err(_) => {
                self.remove_pending(&request_key);
                Err(self.timeout_error())
            }
        };
        drop(permit);
        reply
    }

    fn register_pending(
        &self,
        original_id: Value,
        reply: oneshot::Sender<PendingReply>,
    ) -> Result<(Value, String)> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow!("failed to lock upstream '{}' pending map", self.name))?;
        for _ in 0..=MAX_UPSTREAM_INFLIGHT {
            let sequence = self.next_request_id.fetch_add(1, Ordering::Relaxed);
            let proxy_id = Value::String(format!(
                "{}{}",
                self.request_id_prefix,
                sequence.wrapping_add(1)
            ));
            let proxy_key = request_key(&proxy_id)?;
            if pending.contains_key(&proxy_key) {
                continue;
            }
            pending.insert(proxy_key.clone(), PendingRequest { original_id, reply });
            return Ok((proxy_id, proxy_key));
        }
        Err(anyhow!(
            "upstream '{}' request id namespace is exhausted",
            self.name
        ))
    }

    async fn write_before(&self, deadline: Instant, request: &Value) -> Result<()> {
        let write = async {
            let mut stdin = self.stdin.lock().await;
            write_message_async(&mut *stdin, request, self.framing).await
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
        if messages.len() > MAX_MCP_BATCH_MESSAGES {
            return Ok(UpstreamPayloadDispatch {
                forwarded: None,
                reply: Some(Value::Array(vec![super::mcp_error_response(
                    Value::Null,
                    JSON_RPC_SERVER_ERROR,
                    &format!(
                        "upstream JSON-RPC batch member limit exceeded ({MAX_MCP_BATCH_MESSAGES})"
                    ),
                )])),
            });
        }

        let mut classified = messages
            .into_iter()
            .map(Self::classify_upstream_message)
            .collect::<Vec<_>>();
        let mut plan = ReverseBatchPlan::default();
        for (position, message) in classified.iter_mut().enumerate() {
            match message {
                ClassifiedUpstreamMessage::ReverseRequest(request) => {
                    let original_id = request
                        .get("id")
                        .cloned()
                        .ok_or_else(|| anyhow!("upstream server request is missing id"))?;
                    plan.pending(position, original_id);
                }
                ClassifiedUpstreamMessage::Invalid { response, .. } => {
                    let response = response
                        .take()
                        .ok_or_else(|| anyhow!("upstream batch error response disappeared"))?;
                    plan.immediate(position, response);
                }
                ClassifiedUpstreamMessage::Response { .. }
                | ClassifiedUpstreamMessage::Notification(_) => {}
            }
        }

        if plan.is_empty() {
            let forwarded = self.route_classified_batch(classified, None)?;
            return Ok(UpstreamPayloadDispatch {
                forwarded: (!forwarded.is_empty()).then_some(Value::Array(forwarded)),
                reply: None,
            });
        }

        let (forwarded, reply) = match self.reverse_requests.admit_batch(plan).await? {
            Ok(admission) => {
                let forwarded =
                    match self.route_classified_batch(classified, Some(&admission.proxy_ids)) {
                        Ok(forwarded) => forwarded,
                        Err(error) => {
                            self.reverse_requests
                                .abort_batch(admission.group_id)
                                .await?;
                            return Err(error);
                        }
                    };
                let reply = self.reverse_requests.seal_batch(admission.group_id).await?;
                (forwarded, reply)
            }
            Err(rejection) => {
                let forwarded = self.route_classified_batch(classified, None)?;
                if serialized_value_bytes(&rejection.response)? > MAX_MCP_MESSAGE_BYTES {
                    return Err(anyhow!(
                        "{}; rejected response array exceeds the MCP transport limit",
                        rejection.reason
                    ));
                }
                (forwarded, Some(rejection.response))
            }
        };
        Ok(UpstreamPayloadDispatch {
            forwarded: (!forwarded.is_empty()).then_some(Value::Array(forwarded)),
            reply,
        })
    }

    async fn dispatch_upstream_message(&self, message: Value) -> Result<UpstreamMessageDispatch> {
        match Self::classify_upstream_message(message) {
            ClassifiedUpstreamMessage::Response { message, id } => {
                self.route_upstream_response(message, &id)
            }
            ClassifiedUpstreamMessage::ReverseRequest(request) => {
                Ok(match self.reverse_requests.namespace(request).await? {
                    ReverseRequestDispatch::Forward(request) => {
                        UpstreamMessageDispatch::Forward(request)
                    }
                    ReverseRequestDispatch::Reply(response) => {
                        UpstreamMessageDispatch::Reply(response)
                    }
                })
            }
            ClassifiedUpstreamMessage::Notification(notification) => Ok(
                UpstreamMessageDispatch::Forward(self.augment_notification(notification)),
            ),
            ClassifiedUpstreamMessage::Invalid {
                response,
                pending_response_id,
                reason,
            } => {
                self.fail_invalid_pending_response(pending_response_id.as_ref(), reason)?;
                Ok(UpstreamMessageDispatch::Reply(response.ok_or_else(
                    || anyhow!("upstream error response disappeared"),
                )?))
            }
        }
    }

    fn classify_upstream_message(message: Value) -> ClassifiedUpstreamMessage {
        let Some(object) = message.as_object() else {
            return ClassifiedUpstreamMessage::Invalid {
                response: Some(super::mcp_error_response(
                    Value::Null,
                    -32600,
                    "JSON-RPC upstream message must be an object",
                )),
                pending_response_id: None,
                reason: "JSON-RPC upstream message must be an object",
            };
        };
        let is_response = !object.contains_key("method");

        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Self::classify_invalid_upstream_message(
                object,
                is_response,
                "upstream JSON-RPC message must declare jsonrpc \"2.0\"",
            );
        }
        if object.get("id").is_some_and(|id| !is_valid_json_rpc_id(id)) {
            return Self::classify_invalid_upstream_message(
                object,
                is_response,
                "upstream JSON-RPC id must be a string, number, or null",
            );
        }
        if object
            .get("params")
            .is_some_and(|params| !params.is_object() && !params.is_array())
        {
            return Self::classify_invalid_upstream_message(
                object,
                is_response,
                "upstream JSON-RPC params must be an object or array",
            );
        }

        if is_response {
            let Some(id) = object.get("id") else {
                return Self::classify_invalid_upstream_message(
                    object,
                    true,
                    "upstream JSON-RPC message is missing method and id",
                );
            };
            if object.contains_key("result") == object.contains_key("error") {
                return Self::classify_invalid_upstream_message(
                    object,
                    true,
                    "upstream JSON-RPC response must contain exactly one of result or error",
                );
            }
            if object
                .get("error")
                .is_some_and(|error| !is_valid_json_rpc_error(error))
            {
                return Self::classify_invalid_upstream_message(
                    object,
                    true,
                    "upstream JSON-RPC error must contain an integer code and string message",
                );
            }
            let id = id.clone();
            return ClassifiedUpstreamMessage::Response { message, id };
        }

        if !object.get("method").is_some_and(Value::is_string) {
            return Self::classify_invalid_upstream_message(
                object,
                false,
                "upstream JSON-RPC method must be a string",
            );
        }
        if object.contains_key("result") || object.contains_key("error") {
            return Self::classify_invalid_upstream_message(
                object,
                false,
                "upstream JSON-RPC request must not contain result or error",
            );
        }

        if object.get("id").is_some() {
            return ClassifiedUpstreamMessage::ReverseRequest(message);
        }

        ClassifiedUpstreamMessage::Notification(message)
    }

    fn classify_invalid_upstream_message(
        object: &serde_json::Map<String, Value>,
        is_response: bool,
        reason: &'static str,
    ) -> ClassifiedUpstreamMessage {
        let id = object.get("id");
        let diagnostic_id = id
            .filter(|id| is_valid_json_rpc_id(id))
            .cloned()
            .unwrap_or(Value::Null);
        ClassifiedUpstreamMessage::Invalid {
            response: Some(super::mcp_error_response(diagnostic_id, -32600, reason)),
            pending_response_id: is_response.then(|| id.cloned()).flatten(),
            reason,
        }
    }

    fn route_classified_batch(
        &self,
        messages: Vec<ClassifiedUpstreamMessage>,
        proxy_ids: Option<&BTreeMap<usize, Value>>,
    ) -> Result<Vec<Value>> {
        let mut forwarded = Vec::new();
        for (position, message) in messages.into_iter().enumerate() {
            match message {
                ClassifiedUpstreamMessage::Response { message, id } => {
                    let _ = self.route_upstream_response(message, &id)?;
                }
                ClassifiedUpstreamMessage::ReverseRequest(mut request) => {
                    let Some(proxy_id) = proxy_ids.and_then(|ids| ids.get(&position)) else {
                        continue;
                    };
                    request
                        .as_object_mut()
                        .ok_or_else(|| anyhow!("upstream server request must be an object"))?
                        .insert("id".to_string(), proxy_id.clone());
                    forwarded.push(request);
                }
                ClassifiedUpstreamMessage::Notification(notification) => {
                    forwarded.push(self.augment_notification(notification));
                }
                ClassifiedUpstreamMessage::Invalid {
                    pending_response_id,
                    reason,
                    ..
                } => {
                    self.fail_invalid_pending_response(pending_response_id.as_ref(), reason)?;
                }
            }
        }
        Ok(forwarded)
    }

    fn augment_notification(&self, mut notification: Value) -> Value {
        if let Some(params) = notification
            .get_mut("params")
            .and_then(Value::as_object_mut)
        {
            params
                .entry("upstream".to_string())
                .or_insert_with(|| Value::String(self.name.clone()));
        }
        notification
    }

    fn route_upstream_response(
        &self,
        mut message: Value,
        id: &Value,
    ) -> Result<UpstreamMessageDispatch> {
        if let Some(pending) = self.take_pending_response(id)? {
            message
                .as_object_mut()
                .ok_or_else(|| anyhow!("upstream response must be an object"))?
                .insert("id".to_string(), pending.original_id);
            let _ = pending.reply.send(Ok(message));
        }
        Ok(UpstreamMessageDispatch::Routed)
    }

    fn fail_invalid_pending_response(&self, id: Option<&Value>, reason: &str) -> Result<()> {
        if let Some(pending) = id
            .map(|id| self.take_pending_response(id))
            .transpose()?
            .flatten()
        {
            let _ = pending.reply.send(Err(format!(
                "invalid response from upstream '{}': {reason}",
                self.name
            )));
        }
        Ok(())
    }

    fn take_pending_response(&self, id: &Value) -> Result<Option<PendingRequest>> {
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
        let delivery = match self
            .reverse_requests
            .complete_response(proxy_id, response.clone())
            .await?
        {
            ReverseRequestCompletion::NotOwned => return Ok(false),
            ReverseRequestCompletion::Handled(delivery) => delivery,
        };
        if let Some(delivery) = delivery {
            if let Err(error) = self.send_message(&delivery).await {
                self.set_exit_reason(format!(
                    "failed to forward reverse response to upstream '{}': {error}",
                    self.name
                ));
                let _ = self.reverse_requests.drain().await;
            }
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
                    .map(|(_, pending)| pending.reply)
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
        request_id_prefix: format!("packet28-proxy-request:{name}:"),
        next_request_id: AtomicU64::new(0),
        reverse_requests: ReverseRequestTracker::new(name, timeout),
        inflight: Arc::new(Semaphore::new(MAX_UPSTREAM_INFLIGHT)),
        request_timeout: timeout,
        command_preview,
        compact_tools: server.compact_tools.clone(),
        framing: server.framing.into(),
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
            if contains_resource_list_changed(&forwarded) {
                if let Err(error) = invalidate_resource_catalog(&session) {
                    client.set_exit_reason(format!(
                        "failed to invalidate resource catalog for upstream '{}': {error}",
                        client.name
                    ));
                    return;
                }
            }
            let framing = session
                .lock()
                .ok()
                .and_then(|guard| guard.framing)
                .unwrap_or(McpMessageFraming::ContentLength);
            tokio::select! {
                biased;
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

fn contains_resource_list_changed(payload: &Value) -> bool {
    fn is_list_changed_notification(message: &Value) -> bool {
        message.get("id").is_none()
            && message.get("method").and_then(Value::as_str)
                == Some("notifications/resources/list_changed")
    }

    payload.as_array().map_or_else(
        || is_list_changed_notification(payload),
        |messages| messages.iter().any(is_list_changed_notification),
    )
}

async fn expire_reverse_requests(client: Arc<UpstreamClient>, mut shutdown: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            result = client.reverse_requests.wait_for_expired() => {
                let deliveries = match result {
                    Ok(deliveries) => deliveries,
                    Err(error) => {
                        client.set_exit_reason(format!(
                            "failed to expire reverse request for upstream '{}': {error}",
                            client.name
                        ));
                        let _ = client.reverse_requests.drain().await;
                        return;
                    }
                };
                for delivery in deliveries {
                    if let Err(error) = client.send_message(&delivery).await {
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

fn serialized_value_bytes(value: &Value) -> Result<usize> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .context("failed to serialize upstream JSON-RPC value")
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
        "result": {
            "value": arguments.get("value"),
            "wire_id": message["id"]
        }
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
    if message.get("method") == "test/exit":
        os._exit(17)
    elif message.get("method") == "test/reverse":
        params = message.get("params", {})
        reverse_request = {
            "jsonrpc": "2.0",
            "id": params.get("id", "server-reverse"),
            "method": "roots/list",
            "params": {}
        }
        if params.get("batch"):
            write_message([
                reverse_request,
                {
                    "jsonrpc": "2.0",
                    "method": "notifications/message",
                    "params": {"data": {"batched": True}}
                }
            ])
        else:
            write_message(reverse_request)
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

    #[test]
    fn resource_list_changed_detection_accepts_notifications_in_singletons_and_batches() {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/resources/list_changed",
            "params": {"upstream": "alpha"}
        });

        assert!(contains_resource_list_changed(&notification));
        assert!(contains_resource_list_changed(&json!([
            {"jsonrpc":"2.0","method":"notifications/message"},
            notification
        ])));
    }

    #[test]
    fn resource_list_changed_detection_ignores_same_named_requests() {
        assert!(!contains_resource_list_changed(&json!({
            "jsonrpc": "2.0",
            "id": "server-request",
            "method": "notifications/resources/list_changed"
        })));
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
                framing: super::super::config::McpProxyStdioFraming::ContentLength,
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
            framing: super::super::config::McpProxyStdioFraming::ContentLength,
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
    async fn upstream_batch_member_limit_plus_one_returns_one_bounded_array() {
        let (_directory, client, _output) = test_client(1_000).await;
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/message",
            "params": {}
        });
        let dispatch = client
            .dispatch_upstream_payload(Value::Array(vec![notification; MAX_MCP_BATCH_MESSAGES + 1]))
            .await
            .unwrap();
        let response = dispatch.reply.unwrap();
        assert_eq!(
            (
                dispatch.forwarded,
                response.as_array().unwrap().len(),
                response[0]["id"].clone(),
                response[0]["error"]["code"].clone(),
                client.reverse_requests.batch_group_len().await,
            ),
            (None, 1, Value::Null, json!(JSON_RPC_SERVER_ERROR), 0)
        );
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn response_and_notification_only_upstream_batch_emits_no_response_array() {
        let (_directory, client, _output) = test_client(1_000).await;
        let dispatch = client
            .dispatch_upstream_payload(json!([
                {
                    "jsonrpc": "2.0",
                    "id": "unknown-response",
                    "result": {}
                },
                {
                    "jsonrpc": "2.0",
                    "method": "notifications/message",
                    "params": {"data": {"kind": "only-forwarded-member"}}
                }
            ]))
            .await
            .unwrap();
        let forwarded = dispatch.forwarded.unwrap();
        assert_eq!(
            (
                dispatch.reply,
                forwarded.as_array().unwrap().len(),
                forwarded[0]["method"].clone(),
                client.reverse_requests.batch_group_len().await,
            ),
            (None, 1, json!("notifications/message"), 0,)
        );
        client.shutdown().await;
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
    async fn namespaces_duplicate_client_ids_on_the_upstream_wire() {
        let (_directory, client, _output) = test_client(1_000).await;
        let mut tasks = tokio::task::JoinSet::new();
        for (delay_ms, value) in [(100, "first"), (5, "second")] {
            let client = client.clone();
            tasks.spawn(async move {
                client
                    .send_request(&request(json!("same-client-id"), delay_ms, value))
                    .await
            });
        }

        let first = tasks.join_next().await.unwrap().unwrap().unwrap();
        let second = tasks.join_next().await.unwrap().unwrap().unwrap();
        assert_eq!(first["id"], "same-client-id");
        assert_eq!(second["id"], "same-client-id");
        assert_ne!(
            first["result"]["wire_id"], second["result"]["wire_id"],
            "concurrent requests reused an upstream wire id"
        );
        assert!(first["result"]["wire_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("packet28-proxy-request:test:")));
        assert!(second["result"]["wire_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("packet28-proxy-request:test:")));
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn restores_numeric_client_id_after_namespaced_upstream_round_trip() {
        let (_directory, client, _output) = test_client(1_000).await;

        let response = client
            .send_request(&request(json!(7), 0, "numeric"))
            .await
            .unwrap();

        assert_eq!(response["id"], 7);
        assert!(response["result"]["wire_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("packet28-proxy-request:test:")));
        client.shutdown().await;
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
            .send_message(&trigger_reverse_request(false))
            .await
            .unwrap();
        let forwarded = tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap()
            .unwrap()
            .value;
        let proxy_id = forwarded["id"].clone();

        // Wait for forwarding before exiting: otherwise child reaping can
        // correctly close the output channel before the reader sees the frame.
        client
            .send_message(&json!({"jsonrpc": "2.0", "method": "test/exit"}))
            .await
            .unwrap();
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
    async fn explicit_shutdown_drains_reverse_batch_and_consumes_late_response() {
        let (_directory, client, mut output) = reverse_test_client(1_000).await;
        client
            .send_message(&json!({
                "jsonrpc": "2.0",
                "method": "test/reverse",
                "params": {
                    "id": "server-reverse",
                    "batch": true
                }
            }))
            .await
            .unwrap();
        let forwarded = tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap()
            .unwrap()
            .value;
        let proxy_id = forwarded[0]["id"].clone();
        assert_eq!(client.reverse_requests.len().await, 1);
        assert_eq!(client.reverse_requests.batch_group_len().await, 1);

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
                client.reverse_requests.batch_group_len().await,
                client.reverse_requests.batch_response_bytes().await,
                late_was_consumed,
                client.is_reaped(),
            ),
            (0, 0, 0, true, true)
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
                    "batch": true,
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
                "id": forwarded[0]["id"],
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
        assert!(permits.try_acquire_owned().is_err());
    }
}
