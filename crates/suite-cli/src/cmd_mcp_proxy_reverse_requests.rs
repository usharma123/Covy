//! Bounded correlation for requests initiated by an upstream MCP server.
//!
//! This owner namespaces IDs, accounts for retained IDs and batch responses,
//! and resolves completion, timeout, and shutdown races. It returns JSON-RPC
//! messages to the adapter; it does not own processes or perform transport I/O.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::Value;
use tokio::sync::{Mutex as AsyncMutex, Notify};
use tokio::time::Instant;

use super::super::mcp_error_response;
use super::super::transport::MAX_MCP_MESSAGE_BYTES;
use super::{request_key, serialized_value_bytes, JSON_RPC_SERVER_ERROR};

const MAX_UPSTREAM_REVERSE_REQUESTS: usize = 64;
const MAX_UPSTREAM_REVERSE_ID_BYTES: usize = 64 * 1024;

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
pub(super) struct ReverseBatchPlan {
    responses: BTreeMap<usize, ReverseBatchPlannedResponse>,
}

impl ReverseBatchPlan {
    pub(super) fn pending(&mut self, position: usize, original_id: Value) {
        self.responses
            .insert(position, ReverseBatchPlannedResponse::Pending(original_id));
    }

    pub(super) fn immediate(&mut self, position: usize, response: Value) {
        self.responses
            .insert(position, ReverseBatchPlannedResponse::Immediate(response));
    }

    pub(super) fn is_empty(&self) -> bool {
        self.responses.is_empty()
    }
}

pub(super) struct ReverseBatchAdmission {
    pub(super) group_id: u64,
    pub(super) proxy_ids: BTreeMap<usize, Value>,
}

pub(super) struct ReverseBatchRejection {
    pub(super) reason: String,
    pub(super) response: Value,
}

#[derive(Default)]
struct ReverseRequestState {
    pending: HashMap<String, ReverseRequestEntry>,
    batch_groups: HashMap<u64, ReverseBatchGroup>,
    original_id_bytes: usize,
    batch_response_bytes: usize,
    closed: bool,
}

impl ReverseRequestState {
    fn remove_pending(&mut self, proxy_key: &str) -> Result<Option<ReverseRequestEntry>> {
        let Some(entry) = self.pending.remove(proxy_key) else {
            return Ok(None);
        };
        self.original_id_bytes = self
            .original_id_bytes
            .checked_sub(entry.original_id_bytes)
            .ok_or_else(|| anyhow!("reverse request id byte accounting underflowed"))?;
        Ok(Some(entry))
    }
}

pub(super) enum ReverseRequestDispatch {
    Forward(Value),
    Reply(Value),
}

pub(super) enum ReverseRequestCompletion {
    NotOwned,
    Handled(Option<Value>),
}

pub(super) struct ReverseRequestTracker {
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
    pub(super) fn new(upstream_name: &str, timeout: Duration) -> Self {
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

    pub(super) async fn admit_batch(
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

    pub(super) async fn namespace(&self, mut request: Value) -> Result<ReverseRequestDispatch> {
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
            return Ok(ReverseRequestDispatch::Reply(mcp_error_response(
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

    pub(super) async fn seal_batch(&self, group_id: u64) -> Result<Option<Value>> {
        let mut state = self.state.lock().await;
        let Some(group) = state.batch_groups.get_mut(&group_id) else {
            return Ok(None);
        };
        group.sealed = true;
        self.take_ready_batch(&mut state, group_id)
    }

    pub(super) async fn complete_response(
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
            let Some(entry) = state.remove_pending(&proxy_key)? else {
                return Ok(ReverseRequestCompletion::Handled(None));
            };
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

    pub(super) async fn wait_for_expired(&self) -> Result<Vec<Value>> {
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
                if let Some(entry) = state.remove_pending(&key)? {
                    removed_any = true;
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

    pub(super) async fn drain(&self) -> Vec<Value> {
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

    pub(super) async fn abort_batch(&self, group_id: u64) -> Result<()> {
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
            state.remove_pending(&key)?;
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
        mcp_error_response(
            original_id,
            JSON_RPC_SERVER_ERROR,
            &format!(
                "upstream '{}' reverse batch response exceeded proxy limits",
                self.upstream_name
            ),
        )
    }

    fn timeout_response(&self, original_id: Value) -> Value {
        mcp_error_response(
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
    pub(super) async fn len(&self) -> usize {
        self.state.lock().await.pending.len()
    }

    #[cfg(test)]
    async fn original_id_bytes(&self) -> usize {
        self.state.lock().await.original_id_bytes
    }

    #[cfg(test)]
    pub(super) async fn batch_group_len(&self) -> usize {
        self.state.lock().await.batch_groups.len()
    }

    #[cfg(test)]
    pub(super) async fn batch_response_bytes(&self) -> usize {
        self.state.lock().await.batch_response_bytes
    }

    #[cfg(test)]
    async fn is_closed(&self) -> bool {
        self.state.lock().await.closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

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
    async fn abort_batch_releases_only_its_requests_and_reserved_bytes() {
        let tracker =
            ReverseRequestTracker::with_limits("abort", Duration::from_secs(30), 3, 8_192);
        let singleton_id = forwarded_proxy_id(
            tracker
                .namespace(reverse_request(json!("singleton")))
                .await
                .unwrap(),
        );
        let retained = admitted_batch(&tracker, pending_batch_plan([(0, json!("retained"))])).await;
        let retained_bytes = tracker.batch_response_bytes().await;
        let aborted = admitted_batch(&tracker, pending_batch_plan([(0, json!("aborted"))])).await;
        let aborted_id = aborted.proxy_ids[&0].clone();

        tracker.abort_batch(aborted.group_id).await.unwrap();
        tracker.abort_batch(aborted.group_id).await.unwrap();
        assert_eq!(tracker.len().await, 2);
        assert_eq!(tracker.batch_group_len().await, 1);
        assert_eq!(tracker.batch_response_bytes().await, retained_bytes);
        assert_eq!(
            tracker.original_id_bytes().await,
            request_key(&json!("singleton")).unwrap().len()
                + request_key(&json!("retained")).unwrap().len(),
        );
        assert!(matches!(
            tracker
                .complete_response(&aborted_id, json!({"id": aborted_id, "result": {}}))
                .await
                .unwrap(),
            ReverseRequestCompletion::Handled(None),
        ));

        tracker.seal_batch(retained.group_id).await.unwrap();
        let retained_id = &retained.proxy_ids[&0];
        let retained_reply = tracker
            .complete_response(retained_id, json!({"id": retained_id, "result": {}}))
            .await
            .unwrap();
        assert!(matches!(retained_reply,
            ReverseRequestCompletion::Handled(Some(reply)) if reply[0]["id"] == "retained"
        ));
        let singleton_reply = tracker
            .complete_response(&singleton_id, json!({"id": singleton_id, "result": {}}))
            .await
            .unwrap();
        assert!(matches!(singleton_reply,
            ReverseRequestCompletion::Handled(Some(reply)) if reply["id"] == "singleton"
        ));
        assert_eq!(tracker.len().await, 0);
        assert_eq!(tracker.original_id_bytes().await, 0);
        assert_eq!(tracker.batch_response_bytes().await, 0);
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
            ReverseBatchPlannedResponse::Immediate(mcp_error_response(
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
            ReverseBatchPlannedResponse::Immediate(mcp_error_response(
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
}
