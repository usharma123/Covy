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
use super::proxy_catalog::invalidate_resource_catalog;
use super::transport::{
    read_message_async, render_command_preview, write_message_async, McpMessageFraming,
    MAX_MCP_BATCH_MESSAGES, MAX_MCP_MESSAGE_BYTES,
};
use super::McpSessionState;

const DEFAULT_UPSTREAM_TIMEOUT_MS: u64 = 30_000;
const MAX_UPSTREAM_INFLIGHT: usize = 32;
const MAX_UPSTREAM_REVERSE_REQUESTS: usize = 64;
const MAX_UPSTREAM_REVERSE_ID_BYTES: usize = 64 * 1024;
const JSON_RPC_SERVER_ERROR: i64 = -32000;
pub(super) const MAX_PROXY_OUTPUT_MESSAGES: usize = 64;

type PendingReply = std::result::Result<Value, String>;

struct PendingRequest {
    original_id: Value,
    reply: oneshot::Sender<PendingReply>,
}

struct ReverseRequestEntry {
    original_id: Value,
    original_id_bytes: usize,
    deadline: Instant,
    batch_member: Option<ReverseBatchMember>,
}

#[derive(Clone, Copy)]
struct ReverseBatchMember {
    group_id: u64,
    position: usize,
}

struct ReverseBatchSlot {
    response: Value,
    accounted_bytes: usize,
}

struct ReverseBatchGroup {
    outstanding: usize,
    expected_responses: usize,
    responses: BTreeMap<usize, ReverseBatchSlot>,
    response_bytes: usize,
    sealed: bool,
}

enum ReverseBatchPlannedResponse {
    Pending(Value),
    Immediate(Value),
}

#[derive(Default)]
struct ReverseBatchPlan {
    responses: BTreeMap<usize, ReverseBatchPlannedResponse>,
}

struct ReverseBatchAdmission {
    group_id: u64,
    proxy_ids: BTreeMap<usize, Value>,
}

struct ReverseBatchRejection {
    reason: String,
    response: Value,
}

#[derive(Default)]
struct ReverseRequestState {
    pending: HashMap<String, ReverseRequestEntry>,
    batch_groups: HashMap<u64, ReverseBatchGroup>,
    original_id_bytes: usize,
    batch_response_bytes: usize,
    closed: bool,
}

enum ReverseRequestDispatch {
    Forward(Value),
    Reply(Value),
}

enum ReverseRequestCompletion {
    NotOwned,
    Handled(Option<Value>),
}

struct ReverseRequestTracker {
    upstream_name: String,
    proxy_id_prefix: String,
    timeout: Duration,
    max_pending: usize,
    max_original_id_bytes: usize,
    max_batch_groups: usize,
    max_batch_response_bytes: usize,
    next_id: AtomicU64,
    next_batch_id: AtomicU64,
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
        Self::with_resource_limits(
            upstream_name,
            timeout,
            max_pending,
            max_original_id_bytes,
            max_pending.max(1),
            max_original_id_bytes,
        )
    }

    fn with_resource_limits(
        upstream_name: &str,
        timeout: Duration,
        max_pending: usize,
        max_original_id_bytes: usize,
        max_batch_groups: usize,
        max_batch_response_bytes: usize,
    ) -> Self {
        Self {
            upstream_name: upstream_name.to_string(),
            proxy_id_prefix: format!("packet28-upstream:{upstream_name}:"),
            timeout,
            max_pending,
            max_original_id_bytes,
            max_batch_groups,
            max_batch_response_bytes,
            next_id: AtomicU64::new(0),
            next_batch_id: AtomicU64::new(0),
            state: AsyncMutex::new(ReverseRequestState::default()),
            changed: Notify::new(),
        }
    }

    async fn admit_batch(
        &self,
        plan: ReverseBatchPlan,
    ) -> Result<std::result::Result<ReverseBatchAdmission, ReverseBatchRejection>> {
        let expected_responses = plan.responses.len();
        let mut responses = BTreeMap::new();
        let mut pending = Vec::new();
        let mut response_bytes = usize::from(expected_responses > 0) * 2;
        for (position, planned) in plan.responses {
            let (response, accounted_bytes) = match planned {
                ReverseBatchPlannedResponse::Pending(original_id) => {
                    let original_id_bytes = request_key(&original_id)?.len();
                    pending.push((position, original_id.clone(), original_id_bytes));
                    let fallback = self.batch_fallback_response(original_id.clone());
                    let fallback_bytes = serialized_value_bytes(&fallback)?;
                    let timeout_bytes =
                        serialized_value_bytes(&self.timeout_response(original_id))?;
                    (fallback, fallback_bytes.max(timeout_bytes))
                }
                ReverseBatchPlannedResponse::Immediate(response) => {
                    let response_bytes = serialized_value_bytes(&response)?;
                    (response, response_bytes)
                }
            };
            let separator_bytes = usize::from(!responses.is_empty());
            response_bytes = response_bytes
                .checked_add(separator_bytes)
                .and_then(|total| total.checked_add(accounted_bytes))
                .ok_or_else(|| anyhow!("upstream reverse batch response byte count overflowed"))?;
            responses.insert(
                position,
                ReverseBatchSlot {
                    response,
                    accounted_bytes,
                },
            );
        }
        let group = ReverseBatchGroup {
            outstanding: pending.len(),
            expected_responses,
            responses,
            response_bytes,
            sealed: false,
        };

        let mut state = self.state.lock().await;
        let pending_total = state.pending.len().checked_add(pending.len());
        let original_id_total = pending
            .iter()
            .try_fold(state.original_id_bytes, |total, (_, _, bytes)| {
                total.checked_add(*bytes)
            });
        let batch_response_total = state.batch_response_bytes.checked_add(group.response_bytes);
        let rejection = if state.closed {
            Some(format!(
                "upstream '{}' reverse request tracker is closed",
                self.upstream_name
            ))
        } else if state.batch_groups.len() >= self.max_batch_groups {
            Some(format!(
                "upstream '{}' reverse batch group limit reached ({} active)",
                self.upstream_name, self.max_batch_groups
            ))
        } else if pending_total.is_none_or(|total| total > self.max_pending) {
            Some(format!(
                "upstream '{}' reverse request limit reached ({} pending)",
                self.upstream_name, self.max_pending
            ))
        } else if original_id_total.is_none_or(|total| total > self.max_original_id_bytes) {
            Some(format!(
                "upstream '{}' reverse request id budget exceeded ({} bytes)",
                self.upstream_name, self.max_original_id_bytes
            ))
        } else if group.response_bytes > self.max_batch_response_bytes
            || group.response_bytes > MAX_MCP_MESSAGE_BYTES
            || batch_response_total.is_none_or(|total| total > self.max_batch_response_bytes)
        {
            Some(format!(
                "upstream '{}' reverse batch response budget exceeded ({} bytes)",
                self.upstream_name, self.max_batch_response_bytes
            ))
        } else {
            None
        };
        if let Some(reason) = rejection {
            return Ok(Err(ReverseBatchRejection {
                reason,
                response: Self::batch_group_response(group),
            }));
        }

        let sequence = self.next_batch_id.fetch_add(1, Ordering::Relaxed);
        let group_id = sequence.wrapping_add(1);
        if state.batch_groups.contains_key(&group_id) {
            return Ok(Err(ReverseBatchRejection {
                reason: format!(
                    "upstream '{}' reverse batch group id space exhausted",
                    self.upstream_name
                ),
                response: Self::batch_group_response(group),
            }));
        }

        let mut prepared_pending = Vec::with_capacity(pending.len());
        let mut proxy_ids = BTreeMap::new();
        for (position, original_id, original_id_bytes) in pending {
            let sequence = self.next_id.fetch_add(1, Ordering::Relaxed);
            let proxy_id = Value::String(format!(
                "{}{}",
                self.proxy_id_prefix,
                sequence.wrapping_add(1)
            ));
            let proxy_key = request_key(&proxy_id)?;
            if state.pending.contains_key(&proxy_key) {
                return Ok(Err(ReverseBatchRejection {
                    reason: format!(
                        "upstream '{}' reverse request id space exhausted",
                        self.upstream_name
                    ),
                    response: Self::batch_group_response(group),
                }));
            }
            proxy_ids.insert(position, proxy_id);
            prepared_pending.push((position, proxy_key, original_id, original_id_bytes));
        }

        let deadline = Instant::now() + self.timeout;
        let original_id_total = original_id_total
            .ok_or_else(|| anyhow!("reverse request id byte accounting overflowed"))?;
        let batch_response_total = batch_response_total
            .ok_or_else(|| anyhow!("reverse batch response byte accounting overflowed"))?;
        for (position, proxy_key, original_id, original_id_bytes) in prepared_pending {
            state.pending.insert(
                proxy_key,
                ReverseRequestEntry {
                    original_id,
                    original_id_bytes,
                    deadline,
                    batch_member: Some(ReverseBatchMember { group_id, position }),
                },
            );
        }
        state.original_id_bytes = original_id_total;
        state.batch_response_bytes = batch_response_total;
        state.batch_groups.insert(group_id, group);
        self.changed.notify_one();
        Ok(Ok(ReverseBatchAdmission {
            group_id,
            proxy_ids,
        }))
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
                                batch_member: None,
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

    async fn seal_batch(&self, group_id: u64) -> Result<Option<Value>> {
        let mut state = self.state.lock().await;
        let Some(group) = state.batch_groups.get_mut(&group_id) else {
            return Ok(None);
        };
        group.sealed = true;
        self.take_ready_batch(&mut state, group_id)
    }

    async fn complete_response(
        &self,
        proxy_id: &Value,
        mut response: Value,
    ) -> Result<ReverseRequestCompletion> {
        if !self.owns_proxy_id(proxy_id) {
            return Ok(ReverseRequestCompletion::NotOwned);
        }
        if !response.is_object() {
            return Err(anyhow!("downstream MCP response must be an object"));
        }
        let proxy_key = request_key(proxy_id)?;
        let delivery = {
            let mut state = self.state.lock().await;
            let Some(entry) = state.pending.remove(&proxy_key) else {
                return Ok(ReverseRequestCompletion::Handled(None));
            };
            state.original_id_bytes = state
                .original_id_bytes
                .checked_sub(entry.original_id_bytes)
                .ok_or_else(|| anyhow!("reverse request id byte accounting underflowed"))?;
            response
                .as_object_mut()
                .ok_or_else(|| anyhow!("downstream MCP response must be an object"))?
                .insert("id".to_string(), entry.original_id);
            if let Some(member) = entry.batch_member {
                self.complete_batch_member(&mut state, member, response)?
            } else {
                Some(response)
            }
        };
        self.changed.notify_one();
        Ok(ReverseRequestCompletion::Handled(delivery))
    }

    async fn wait_for_expired(&self) -> Result<Vec<Value>> {
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
                            let expired = self.expire(Instant::now()).await?;
                            if !expired.is_empty() {
                                return Ok(expired);
                            }
                        }
                        () = changed => {}
                    }
                }
                None => changed.await,
            }
        }
    }

    async fn expire(&self, now: Instant) -> Result<Vec<Value>> {
        let (deliveries, removed_any) = {
            let mut state = self.state.lock().await;
            let keys = state
                .pending
                .iter()
                .filter_map(|(key, entry)| (entry.deadline <= now).then_some(key.clone()))
                .collect::<Vec<_>>();
            let mut deliveries = Vec::with_capacity(keys.len());
            let mut removed_any = false;
            for key in keys {
                if let Some(entry) = state.pending.remove(&key) {
                    removed_any = true;
                    state.original_id_bytes = state
                        .original_id_bytes
                        .checked_sub(entry.original_id_bytes)
                        .ok_or_else(|| anyhow!("reverse request id byte accounting underflowed"))?;
                    let response = self.timeout_response(entry.original_id);
                    if let Some(member) = entry.batch_member {
                        if let Some(delivery) =
                            self.complete_batch_member(&mut state, member, response)?
                        {
                            deliveries.push(delivery);
                        }
                    } else {
                        deliveries.push(response);
                    }
                }
            }
            (deliveries, removed_any)
        };
        if removed_any {
            self.changed.notify_one();
        }
        Ok(deliveries)
    }

    async fn drain(&self) -> Vec<Value> {
        let drained = {
            let mut state = self.state.lock().await;
            state.closed = true;
            state.original_id_bytes = 0;
            state.batch_response_bytes = 0;
            state.batch_groups.clear();
            std::mem::take(&mut state.pending)
                .into_values()
                .map(|entry| entry.original_id)
                .collect()
        };
        self.changed.notify_one();
        drained
    }

    async fn abort_batch(&self, group_id: u64) -> Result<()> {
        let mut state = self.state.lock().await;
        let keys = state
            .pending
            .iter()
            .filter_map(|(key, entry)| {
                entry
                    .batch_member
                    .is_some_and(|member| member.group_id == group_id)
                    .then_some(key.clone())
            })
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(entry) = state.pending.remove(&key) {
                state.original_id_bytes = state
                    .original_id_bytes
                    .checked_sub(entry.original_id_bytes)
                    .ok_or_else(|| anyhow!("reverse request id byte accounting underflowed"))?;
            }
        }
        if let Some(group) = state.batch_groups.remove(&group_id) {
            state.batch_response_bytes = state
                .batch_response_bytes
                .checked_sub(group.response_bytes)
                .ok_or_else(|| anyhow!("reverse batch response byte accounting underflowed"))?;
        }
        self.changed.notify_one();
        Ok(())
    }

    fn complete_batch_member(
        &self,
        state: &mut ReverseRequestState,
        member: ReverseBatchMember,
        response: Value,
    ) -> Result<Option<Value>> {
        let outstanding = state
            .batch_groups
            .get(&member.group_id)
            .ok_or_else(|| anyhow!("reverse batch response group disappeared"))?
            .outstanding
            .checked_sub(1)
            .ok_or_else(|| anyhow!("reverse batch outstanding count underflowed"))?;
        self.replace_batch_response(state, member, response)?;
        if let Some(group) = state.batch_groups.get_mut(&member.group_id) {
            group.outstanding = outstanding;
        }
        self.take_ready_batch(state, member.group_id)
    }

    fn replace_batch_response(
        &self,
        state: &mut ReverseRequestState,
        member: ReverseBatchMember,
        response: Value,
    ) -> Result<()> {
        let response_bytes = serialized_value_bytes(&response)?;
        let (previous_bytes, current_group_bytes) = {
            let group = state
                .batch_groups
                .get(&member.group_id)
                .ok_or_else(|| anyhow!("reverse batch response group disappeared"))?;
            let slot = group
                .responses
                .get(&member.position)
                .ok_or_else(|| anyhow!("reverse batch response slot disappeared"))?;
            (slot.accounted_bytes, group.response_bytes)
        };
        let group_total = current_group_bytes
            .checked_sub(previous_bytes)
            .ok_or_else(|| anyhow!("reverse batch group byte accounting underflowed"))?
            .checked_add(response_bytes)
            .ok_or_else(|| anyhow!("reverse batch group byte accounting overflowed"))?;
        let global_total = state
            .batch_response_bytes
            .checked_sub(previous_bytes)
            .ok_or_else(|| anyhow!("reverse batch response byte accounting underflowed"))?
            .checked_add(response_bytes)
            .ok_or_else(|| anyhow!("reverse batch response byte accounting overflowed"))?;
        if group_total <= self.max_batch_response_bytes
            && group_total <= MAX_MCP_MESSAGE_BYTES
            && global_total <= self.max_batch_response_bytes
        {
            let group = state
                .batch_groups
                .get_mut(&member.group_id)
                .ok_or_else(|| anyhow!("reverse batch response group disappeared"))?;
            let slot = group
                .responses
                .get_mut(&member.position)
                .ok_or_else(|| anyhow!("reverse batch response slot disappeared"))?;
            slot.response = response;
            slot.accounted_bytes = response_bytes;
            group.response_bytes = group_total;
            state.batch_response_bytes = global_total;
        }
        Ok(())
    }

    fn take_ready_batch(
        &self,
        state: &mut ReverseRequestState,
        group_id: u64,
    ) -> Result<Option<Value>> {
        let Some(group) = state.batch_groups.get(&group_id) else {
            return Ok(None);
        };
        let ready = group.sealed && group.outstanding == 0;
        if !ready {
            return Ok(None);
        }
        if group.responses.len() != group.expected_responses {
            return Err(anyhow!(
                "reverse batch response group resolved with {} of {} slots",
                group.responses.len(),
                group.expected_responses
            ));
        }
        let response_bytes = group.response_bytes;
        state.batch_response_bytes = state
            .batch_response_bytes
            .checked_sub(response_bytes)
            .ok_or_else(|| anyhow!("reverse batch response byte accounting underflowed"))?;
        let group = state
            .batch_groups
            .remove(&group_id)
            .ok_or_else(|| anyhow!("reverse batch response group disappeared"))?;
        Ok(Some(Self::batch_group_response(group)))
    }

    fn batch_group_response(group: ReverseBatchGroup) -> Value {
        Value::Array(
            group
                .responses
                .into_values()
                .map(|slot| slot.response)
                .collect(),
        )
    }

    fn batch_fallback_response(&self, original_id: Value) -> Value {
        super::mcp_error_response(
            original_id,
            JSON_RPC_SERVER_ERROR,
            &format!(
                "upstream '{}' reverse batch response exceeded proxy limits",
                self.upstream_name
            ),
        )
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
    async fn batch_group_len(&self) -> usize {
        self.state.lock().await.batch_groups.len()
    }

    #[cfg(test)]
    async fn batch_response_bytes(&self) -> usize {
        self.state.lock().await.batch_response_bytes
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
                    plan.responses
                        .insert(position, ReverseBatchPlannedResponse::Pending(original_id));
                }
                ClassifiedUpstreamMessage::Invalid { response, .. } => {
                    let response = response
                        .take()
                        .ok_or_else(|| anyhow!("upstream batch error response disappeared"))?;
                    plan.responses
                        .insert(position, ReverseBatchPlannedResponse::Immediate(response));
                }
                ClassifiedUpstreamMessage::Response { .. }
                | ClassifiedUpstreamMessage::Notification(_) => {}
            }
        }

        if plan.responses.is_empty() {
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
    if message.get("method") == "test/reverse":
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

    async fn admitted_batch(
        tracker: &ReverseRequestTracker,
        plan: ReverseBatchPlan,
    ) -> ReverseBatchAdmission {
        match tracker.admit_batch(plan).await.unwrap() {
            Ok(admission) => admission,
            Err(rejection) => panic!("batch admission rejected: {}", rejection.reason),
        }
    }

    fn pending_batch_plan(members: impl IntoIterator<Item = (usize, Value)>) -> ReverseBatchPlan {
        ReverseBatchPlan {
            responses: members
                .into_iter()
                .map(|(position, id)| (position, ReverseBatchPlannedResponse::Pending(id)))
                .collect(),
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
        let deliveries = waiter.await.unwrap().unwrap();
        assert_eq!(
            (
                deliveries.len(),
                deliveries[0]["id"].clone(),
                deliveries[0]["error"]["code"].clone(),
                tracker.len().await,
            ),
            (1, json!("deadline"), json!(JSON_RPC_SERVER_ERROR), 0,)
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

        let response_tracker = tracker.clone();
        let response_barrier = barrier.clone();
        let response = tokio::spawn(async move {
            response_barrier.wait().await;
            response_tracker
                .complete_response(
                    &proxy_id,
                    json!({
                        "jsonrpc": "2.0",
                        "id": proxy_id,
                        "result": {"roots": []}
                    }),
                )
                .await
                .unwrap()
        });
        let expire_tracker = tracker.clone();
        let expire_barrier = barrier.clone();
        let expire = tokio::spawn(async move {
            expire_barrier.wait().await;
            expire_tracker.expire(Instant::now()).await
        });
        barrier.wait().await;

        let response = response.await.unwrap();
        let expired = expire.await.unwrap().unwrap();
        let response_deliveries = match response {
            ReverseRequestCompletion::Handled(Some(_)) => 1,
            ReverseRequestCompletion::Handled(None) => 0,
            ReverseRequestCompletion::NotOwned => panic!("proxy response id was not owned"),
        };
        assert_eq!(
            (
                response_deliveries + expired.len(),
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
        let late = tracker
            .complete_response(
                &proxy_id,
                json!({
                    "jsonrpc": "2.0",
                    "id": proxy_id,
                    "result": {"roots": []}
                }),
            )
            .await
            .unwrap();
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
                matches!(late, ReverseRequestCompletion::Handled(None)),
                tracker.owns_proxy_id(&proxy_id),
                tracker.len().await,
                tracker.original_id_bytes().await,
                tracker.is_closed().await,
                after_close["error"]["code"].clone(),
            ),
            (
                vec![json!("original")],
                true,
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

    #[tokio::test]
    async fn reverse_batch_completion_before_seal_emits_one_ordered_array() {
        let tracker =
            ReverseRequestTracker::with_limits("batch", Duration::from_secs(30), 4, 8_192);
        let mut plan = pending_batch_plan([(1, json!("first")), (3, json!("second"))]);
        plan.responses.insert(
            0,
            ReverseBatchPlannedResponse::Immediate(super::super::mcp_error_response(
                json!("invalid"),
                -32600,
                "invalid member",
            )),
        );
        let admission = admitted_batch(&tracker, plan).await;
        let second_proxy_id = admission.proxy_ids[&3].clone();
        let first_proxy_id = admission.proxy_ids[&1].clone();

        let second = tracker
            .complete_response(
                &second_proxy_id,
                json!({
                    "jsonrpc": "2.0",
                    "id": second_proxy_id,
                    "result": {"order": 2}
                }),
            )
            .await
            .unwrap();
        let first = tracker
            .complete_response(
                &first_proxy_id,
                json!({
                    "jsonrpc": "2.0",
                    "id": first_proxy_id,
                    "result": {"order": 1}
                }),
            )
            .await
            .unwrap();
        assert!(matches!(
            (second, first),
            (
                ReverseRequestCompletion::Handled(None),
                ReverseRequestCompletion::Handled(None)
            )
        ));

        let delivery = tracker
            .seal_batch(admission.group_id)
            .await
            .unwrap()
            .unwrap();
        let responses = delivery.as_array().unwrap();
        assert_eq!(
            (
                responses
                    .iter()
                    .map(|response| response["id"].clone())
                    .collect::<Vec<_>>(),
                responses[1]["result"]["order"].clone(),
                responses[2]["result"]["order"].clone(),
                tracker.len().await,
                tracker.batch_group_len().await,
                tracker.batch_response_bytes().await,
            ),
            (
                vec![json!("invalid"), json!("first"), json!("second")],
                json!(1),
                json!(2),
                0,
                0,
                0,
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reverse_batch_timeout_waits_and_preserves_immediate_response_order() {
        let tracker = Arc::new(ReverseRequestTracker::with_limits(
            "batch-timeout",
            Duration::from_secs(5),
            2,
            8_192,
        ));
        let mut plan = pending_batch_plan([(2, json!("timeout"))]);
        plan.responses.insert(
            0,
            ReverseBatchPlannedResponse::Immediate(super::super::mcp_error_response(
                json!("invalid"),
                -32600,
                "invalid member",
            )),
        );
        let admission = admitted_batch(&tracker, plan).await;
        assert!(tracker
            .seal_batch(admission.group_id)
            .await
            .unwrap()
            .is_none());
        let waiter_tracker = tracker.clone();
        let waiter = tokio::spawn(async move { waiter_tracker.wait_for_expired().await });
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_secs(5)).await;
        let deliveries = waiter.await.unwrap().unwrap();
        let responses = deliveries[0].as_array().unwrap();
        assert_eq!(
            (
                deliveries.len(),
                responses.len(),
                responses[0]["id"].clone(),
                responses[1]["id"].clone(),
                responses[1]["error"]["code"].clone(),
                tracker.batch_group_len().await,
            ),
            (
                1,
                2,
                json!("invalid"),
                json!("timeout"),
                json!(JSON_RPC_SERVER_ERROR),
                0,
            )
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reverse_batch_last_response_and_timeout_race_emits_once() {
        let tracker = Arc::new(ReverseRequestTracker::with_limits(
            "batch-race",
            Duration::ZERO,
            2,
            8_192,
        ));
        let admission =
            admitted_batch(&tracker, pending_batch_plan([(0, json!("original"))])).await;
        assert!(tracker
            .seal_batch(admission.group_id)
            .await
            .unwrap()
            .is_none());
        let proxy_id = admission.proxy_ids[&0].clone();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));

        let response_tracker = tracker.clone();
        let response_barrier = barrier.clone();
        let response_proxy_id = proxy_id.clone();
        let response = tokio::spawn(async move {
            response_barrier.wait().await;
            response_tracker
                .complete_response(
                    &response_proxy_id,
                    json!({
                        "jsonrpc": "2.0",
                        "id": response_proxy_id,
                        "result": {"roots": []}
                    }),
                )
                .await
                .unwrap()
        });
        let expiry_tracker = tracker.clone();
        let expiry_barrier = barrier.clone();
        let expiry = tokio::spawn(async move {
            expiry_barrier.wait().await;
            expiry_tracker.expire(Instant::now()).await.unwrap()
        });
        barrier.wait().await;

        let response = response.await.unwrap();
        let expired = expiry.await.unwrap();
        let response_delivery = match response {
            ReverseRequestCompletion::Handled(delivery) => delivery,
            ReverseRequestCompletion::NotOwned => panic!("batch response id was not owned"),
        };
        assert_eq!(
            (
                usize::from(response_delivery.is_some()) + expired.len(),
                response_delivery
                    .as_ref()
                    .or_else(|| expired.first())
                    .is_some_and(Value::is_array),
                tracker.len().await,
                tracker.batch_group_len().await,
                tracker.batch_response_bytes().await,
            ),
            (1, true, 0, 0, 0)
        );
    }

    #[tokio::test]
    async fn reverse_batch_groups_complete_without_cross_group_responses() {
        let tracker =
            ReverseRequestTracker::with_limits("interleave", Duration::from_secs(30), 4, 8_192);
        let first = admitted_batch(
            &tracker,
            pending_batch_plan([(0, json!("a-1")), (1, json!("a-2"))]),
        )
        .await;
        let second = admitted_batch(&tracker, pending_batch_plan([(0, json!("b-1"))])).await;
        assert!(tracker.seal_batch(first.group_id).await.unwrap().is_none());
        assert!(tracker.seal_batch(second.group_id).await.unwrap().is_none());

        let first_partial = tracker
            .complete_response(
                &first.proxy_ids[&0],
                json!({
                    "jsonrpc": "2.0",
                    "id": first.proxy_ids[&0],
                    "result": {}
                }),
            )
            .await
            .unwrap();
        let second_delivery = tracker
            .complete_response(
                &second.proxy_ids[&0],
                json!({
                    "jsonrpc": "2.0",
                    "id": second.proxy_ids[&0],
                    "result": {}
                }),
            )
            .await
            .unwrap();
        let first_delivery = tracker
            .complete_response(
                &first.proxy_ids[&1],
                json!({
                    "jsonrpc": "2.0",
                    "id": first.proxy_ids[&1],
                    "result": {}
                }),
            )
            .await
            .unwrap();

        let ids = |completion: ReverseRequestCompletion| match completion {
            ReverseRequestCompletion::Handled(Some(delivery)) => delivery
                .as_array()
                .unwrap()
                .iter()
                .map(|response| response["id"].clone())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        assert_eq!(
            (
                matches!(first_partial, ReverseRequestCompletion::Handled(None)),
                ids(second_delivery),
                ids(first_delivery),
                tracker.batch_group_len().await,
            ),
            (
                true,
                vec![json!("b-1")],
                vec![json!("a-1"), json!("a-2")],
                0,
            )
        );
    }

    #[tokio::test]
    async fn reverse_batch_limits_reject_atomically_and_oversized_result_keeps_same_id_fallback() {
        let sizing_tracker = ReverseRequestTracker::with_resource_limits(
            "bounded-batch",
            Duration::from_secs(30),
            4,
            8_192,
            1,
            8_192,
        );
        let fallback = sizing_tracker.batch_fallback_response(json!("oversized-result"));
        let timeout = sizing_tracker.timeout_response(json!("oversized-result"));
        let exact_response_bytes = 2 + serialized_value_bytes(&fallback)
            .unwrap()
            .max(serialized_value_bytes(&timeout).unwrap());
        let response_budget_rejection = match ReverseRequestTracker::with_resource_limits(
            "bounded-batch",
            Duration::from_secs(30),
            4,
            8_192,
            1,
            exact_response_bytes - 1,
        )
        .admit_batch(pending_batch_plan([(0, json!("oversized-result"))]))
        .await
        .unwrap()
        {
            Ok(_) => panic!("response byte limit + 1 was admitted"),
            Err(rejection) => rejection,
        };
        assert_eq!(
            (
                response_budget_rejection.response[0]["id"].clone(),
                response_budget_rejection.response[0]["error"]["code"].clone(),
            ),
            (json!("oversized-result"), json!(JSON_RPC_SERVER_ERROR))
        );
        let tracker = ReverseRequestTracker::with_resource_limits(
            "bounded-batch",
            Duration::from_secs(30),
            4,
            8_192,
            1,
            exact_response_bytes,
        );
        let admission = admitted_batch(
            &tracker,
            pending_batch_plan([(0, json!("oversized-result"))]),
        )
        .await;
        assert!(tracker
            .seal_batch(admission.group_id)
            .await
            .unwrap()
            .is_none());

        let group_rejection = match tracker
            .admit_batch(pending_batch_plan([(0, json!("rejected-group"))]))
            .await
            .unwrap()
        {
            Ok(_) => panic!("group limit + 1 was admitted"),
            Err(rejection) => rejection,
        };
        assert_eq!(
            (
                group_rejection.response[0]["id"].clone(),
                group_rejection.response[0]["error"]["code"].clone(),
                tracker.len().await,
                tracker.batch_group_len().await,
            ),
            (json!("rejected-group"), json!(JSON_RPC_SERVER_ERROR), 1, 1,)
        );

        let proxy_id = admission.proxy_ids[&0].clone();
        let completion = tracker
            .complete_response(
                &proxy_id,
                json!({
                    "jsonrpc": "2.0",
                    "id": proxy_id,
                    "result": {"oversized": "x".repeat(exact_response_bytes)}
                }),
            )
            .await
            .unwrap();
        let ReverseRequestCompletion::Handled(Some(delivery)) = completion else {
            panic!("oversized final response did not resolve the batch")
        };
        assert_eq!(
            (
                delivery[0]["id"].clone(),
                delivery[0]["error"]["code"].clone(),
                tracker.len().await,
                tracker.batch_group_len().await,
                tracker.batch_response_bytes().await,
            ),
            (
                json!("oversized-result"),
                json!(JSON_RPC_SERVER_ERROR),
                0,
                0,
                0,
            )
        );
    }

    #[tokio::test]
    async fn reverse_batch_exact_reserved_boundary_preserves_timeout_response() {
        let sizing_tracker = ReverseRequestTracker::with_resource_limits(
            "timeout-boundary",
            Duration::ZERO,
            1,
            8_192,
            1,
            8_192,
        );
        let original_id = json!("deadline");
        let fallback = sizing_tracker.batch_fallback_response(original_id.clone());
        let timeout = sizing_tracker.timeout_response(original_id.clone());
        let exact_response_bytes = 2 + serialized_value_bytes(&fallback)
            .unwrap()
            .max(serialized_value_bytes(&timeout).unwrap());
        let tracker = ReverseRequestTracker::with_resource_limits(
            "timeout-boundary",
            Duration::ZERO,
            1,
            8_192,
            1,
            exact_response_bytes,
        );
        let admission =
            admitted_batch(&tracker, pending_batch_plan([(0, original_id.clone())])).await;
        assert_eq!(tracker.batch_response_bytes().await, exact_response_bytes);
        assert!(tracker
            .seal_batch(admission.group_id)
            .await
            .unwrap()
            .is_none());

        let deliveries = tracker.expire(Instant::now()).await.unwrap();

        assert_eq!(deliveries.len(), 1);
        let responses = deliveries[0].as_array().unwrap();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], original_id);
        assert_eq!(responses[0]["error"]["code"], JSON_RPC_SERVER_ERROR);
        assert!(responses[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("timed out"));
        assert_eq!(
            (
                tracker.len().await,
                tracker.batch_group_len().await,
                tracker.batch_response_bytes().await,
            ),
            (0, 0, 0)
        );
    }

    #[tokio::test]
    async fn reverse_batch_pending_and_id_limits_reject_every_request_atomically() {
        let pending_tracker = ReverseRequestTracker::with_resource_limits(
            "batch-pending",
            Duration::from_secs(30),
            1,
            8_192,
            2,
            8_192,
        );
        let pending_rejection = match pending_tracker
            .admit_batch(pending_batch_plan([
                (0, json!("first")),
                (1, json!("second")),
            ]))
            .await
            .unwrap()
        {
            Ok(_) => panic!("pending limit + 1 batch was partially admitted"),
            Err(rejection) => rejection,
        };
        assert_eq!(
            (
                pending_rejection
                    .response
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|response| response["id"].clone())
                    .collect::<Vec<_>>(),
                pending_tracker.len().await,
                pending_tracker.batch_group_len().await,
                pending_tracker.original_id_bytes().await,
                pending_tracker.batch_response_bytes().await,
            ),
            (vec![json!("first"), json!("second")], 0, 0, 0, 0)
        );

        let exact_id_bytes = request_key(&json!("exact-id")).unwrap().len();
        let exact_tracker = ReverseRequestTracker::with_resource_limits(
            "batch-id",
            Duration::from_secs(30),
            1,
            exact_id_bytes,
            1,
            8_192,
        );
        let _exact =
            admitted_batch(&exact_tracker, pending_batch_plan([(0, json!("exact-id"))])).await;
        assert_eq!(exact_tracker.original_id_bytes().await, exact_id_bytes);
        exact_tracker.drain().await;

        let id_rejection = match ReverseRequestTracker::with_resource_limits(
            "batch-id",
            Duration::from_secs(30),
            1,
            exact_id_bytes - 1,
            1,
            8_192,
        )
        .admit_batch(pending_batch_plan([(0, json!("exact-id"))]))
        .await
        .unwrap()
        {
            Ok(_) => panic!("id byte limit + 1 batch was admitted"),
            Err(rejection) => rejection,
        };
        assert_eq!(
            (
                id_rejection.response[0]["id"].clone(),
                id_rejection.response[0]["error"]["code"].clone(),
            ),
            (json!("exact-id"), json!(JSON_RPC_SERVER_ERROR))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reverse_batch_completion_and_drain_race_clears_group_and_consumes_late_response() {
        let tracker = Arc::new(ReverseRequestTracker::with_limits(
            "batch-drain",
            Duration::from_secs(30),
            2,
            8_192,
        ));
        let admission =
            admitted_batch(&tracker, pending_batch_plan([(0, json!("original"))])).await;
        assert!(tracker
            .seal_batch(admission.group_id)
            .await
            .unwrap()
            .is_none());
        let proxy_id = admission.proxy_ids[&0].clone();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));

        let response_tracker = tracker.clone();
        let response_barrier = barrier.clone();
        let response_proxy_id = proxy_id.clone();
        let response = tokio::spawn(async move {
            response_barrier.wait().await;
            response_tracker
                .complete_response(
                    &response_proxy_id,
                    json!({
                        "jsonrpc": "2.0",
                        "id": response_proxy_id,
                        "result": {}
                    }),
                )
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

        let response = response.await.unwrap();
        let drained = drain.await.unwrap();
        let handled = matches!(
            response,
            ReverseRequestCompletion::Handled(Some(_)) | ReverseRequestCompletion::Handled(None)
        );
        let original_accounted_once = usize::from(!drained.is_empty())
            + usize::from(matches!(
                response,
                ReverseRequestCompletion::Handled(Some(_))
            ));
        let late = tracker
            .complete_response(
                &proxy_id,
                json!({
                    "jsonrpc": "2.0",
                    "id": proxy_id,
                    "result": {}
                }),
            )
            .await
            .unwrap();
        assert_eq!(
            (
                handled,
                original_accounted_once,
                matches!(late, ReverseRequestCompletion::Handled(None)),
                tracker.len().await,
                tracker.batch_group_len().await,
                tracker.original_id_bytes().await,
                tracker.batch_response_bytes().await,
            ),
            (true, 1, true, 0, 0, 0, 0)
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
