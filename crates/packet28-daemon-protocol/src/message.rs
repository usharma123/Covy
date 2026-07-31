//! Top-level daemon request, response, status, and event messages.

use super::*;

pub use suite_packet_core::{InstructionRenderMode, InstructionStableConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonRequest {
    Execute {
        request: KernelRequest,
    },
    ExecuteSequence {
        spec: TaskSubmitSpec,
    },
    Status,
    Stop,
    TaskStatus {
        task_id: String,
    },
    TaskAwaitHandoff {
        request: TaskAwaitHandoffRequest,
    },
    TaskMarkHandoffConsumed {
        request: TaskMarkHandoffConsumedRequest,
    },
    TaskLaunchAgent {
        request: TaskLaunchAgentRequest,
    },
    TaskCancel {
        task_id: String,
    },
    TaskSubscribe {
        task_id: String,
        replay_last: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_seq: Option<u64>,
    },
    WatchList {
        task_id: Option<String>,
    },
    WatchRemove {
        watch_id: String,
    },
    CoverCheck {
        request: CoverCheckRequest,
    },
    PacketFetch {
        request: PacketFetchRequest,
    },
    TestShard {
        request: TestShardRequest,
    },
    TestMap {
        request: TestMapRequest,
    },
    ContextStoreList {
        request: ContextStoreListRequest,
    },
    ContextStoreGet {
        request: ContextStoreGetRequest,
    },
    ContextStorePrune {
        request: ContextStorePruneDaemonRequest,
    },
    ContextStoreStats {
        request: ContextStoreStatsRequest,
    },
    ContextRecall {
        request: ContextRecallRequest,
    },
    BrokerGetContext {
        request: BrokerGetContextRequest,
    },
    BrokerEstimateContext {
        request: BrokerEstimateContextRequest,
    },
    BrokerPrepareHandoff {
        request: BrokerPrepareHandoffRequest,
    },
    BrokerValidatePlan {
        request: BrokerValidatePlanRequest,
    },
    BrokerDecompose {
        request: BrokerDecomposeRequest,
    },
    BrokerWriteState {
        request: BrokerWriteStateRequest,
    },
    BrokerWriteStateBatch {
        request: BrokerWriteStateBatchRequest,
    },
    BrokerTaskStatus {
        request: BrokerTaskStatusRequest,
    },
    ContextResolve {
        request: ContextResolveRequest,
    },
    InstructionFileResolve {
        request: InstructionFileResolveRequest,
    },
    HookIngest {
        request: HookIngestRequest,
    },
    Packet28Search {
        request: Packet28SearchRequest,
    },
    Packet28SearchGuard {
        request: Packet28SearchRequest,
    },
    DaemonIndexStatus {
        request: DaemonIndexStatusRequest,
    },
    DaemonIndexRebuild {
        request: DaemonIndexRebuildRequest,
    },
    DaemonIndexClear {
        request: DaemonIndexClearRequest,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Packet28SearchRequest {
    pub request: suite_packet_core::search::SearchRequest,
    pub force_indexed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Packet28SearchGuardResponse {
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextSourceKind {
    #[default]
    InstructionFile,
    SystemPromptFragment,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextBackendKind {
    #[default]
    LinuxPreload,
    LinuxOci,
    MacosSwap,
    MacosFuse,
    WindowsFuse,
    ProxyOnly,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ContextResolveRequest {
    pub workspace_root: String,
    pub source_kind: ContextSourceKind,
    pub source_path: Option<String>,
    pub source_sha256: String,
    pub source_content: String,
    pub render_mode: Option<InstructionRenderMode>,
    pub stable_config: Option<InstructionStableConfig>,
    pub task_id: Option<String>,
    pub task_label: Option<String>,
    pub budget_tokens: Option<u64>,
    pub schema_version: u32,
    pub agent_family: Option<String>,
    pub backend_kind: ContextBackendKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ContextResolveOutcome {
    Rewrite {
        content: String,
        content_sha256: String,
        #[serde(default)]
        render_mode: InstructionRenderMode,
        #[serde(default)]
        stable_config_sha256: String,
        #[serde(default)]
        snapshot_sha256: Option<String>,
        #[serde(default)]
        rendered_sha256: String,
        task_label: String,
        original_bytes: usize,
        rewritten_bytes: usize,
        cache_hit: bool,
        matched_terms: Vec<String>,
        section_titles: Vec<String>,
        schema_version: u32,
    },
    Passthrough {
        reason: String,
        content_sha256: Option<String>,
        task_label: Option<String>,
        original_bytes: Option<usize>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextResolveResponse {
    pub source_kind: ContextSourceKind,
    pub source_path: Option<String>,
    pub outcome: ContextResolveOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct InstructionFileResolveRequest {
    pub workspace_root: String,
    pub path: String,
    pub content_sha256: String,
    pub content: String,
    pub render_mode: Option<InstructionRenderMode>,
    pub stable_config: Option<InstructionStableConfig>,
    pub task_id: Option<String>,
    pub budget_tokens: Option<u64>,
    pub schema_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum InstructionFileResolveOutcome {
    Rewrite {
        content: String,
        content_sha256: String,
        #[serde(default)]
        render_mode: InstructionRenderMode,
        #[serde(default)]
        stable_config_sha256: String,
        #[serde(default)]
        snapshot_sha256: Option<String>,
        #[serde(default)]
        rendered_sha256: String,
        task_label: String,
        original_bytes: usize,
        rewritten_bytes: usize,
        cache_hit: bool,
        matched_terms: Vec<String>,
        section_titles: Vec<String>,
    },
    Passthrough {
        reason: String,
        content_sha256: Option<String>,
        task_label: Option<String>,
        original_bytes: Option<usize>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructionFileResolveResponse {
    pub path: String,
    pub outcome: InstructionFileResolveOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonResponse {
    Execute {
        response: KernelResponse,
    },
    ExecuteSequence {
        response: KernelSequenceResponse,
        task: TaskRecord,
        watches: Vec<WatchRegistration>,
    },
    Status {
        status: DaemonStatus,
    },
    Ack {
        message: String,
    },
    TaskStatus {
        task: Option<TaskRecord>,
    },
    TaskAwaitHandoff {
        response: TaskAwaitHandoffResponse,
    },
    TaskMarkHandoffConsumed {
        response: TaskMarkHandoffConsumedResponse,
    },
    TaskLaunchAgent {
        response: TaskLaunchAgentResponse,
    },
    TaskCancel {
        task: Option<TaskRecord>,
        removed_watch_ids: Vec<String>,
    },
    TaskSubscribeAck {
        task_id: String,
        replayed: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_seq: Option<u64>,
    },
    WatchList {
        watches: Vec<WatchRegistration>,
    },
    WatchRemove {
        removed: Option<WatchRegistration>,
    },
    CoverCheck {
        response: CoverCheckResponse,
    },
    PacketFetch {
        response: PacketFetchResponse,
    },
    TestShard {
        response: TestShardResponse,
    },
    TestMap {
        response: TestMapResponse,
    },
    ContextStoreList {
        response: ContextStoreListResponse,
    },
    ContextStoreGet {
        response: ContextStoreGetResponse,
    },
    ContextStorePrune {
        response: ContextStorePruneResponse,
    },
    ContextStoreStats {
        response: ContextStoreStatsResponse,
    },
    ContextRecall {
        response: ContextRecallResponse,
    },
    BrokerGetContext {
        response: BrokerGetContextResponse,
    },
    BrokerEstimateContext {
        response: BrokerEstimateContextResponse,
    },
    BrokerPrepareHandoff {
        response: BrokerPrepareHandoffResponse,
    },
    BrokerValidatePlan {
        response: BrokerValidatePlanResponse,
    },
    BrokerDecompose {
        response: BrokerDecomposeResponse,
    },
    BrokerWriteState {
        response: BrokerWriteStateResponse,
    },
    BrokerWriteStateBatch {
        response: BrokerWriteStateBatchResponse,
    },
    BrokerTaskStatus {
        response: BrokerTaskStatusResponse,
    },
    ContextResolve {
        response: ContextResolveResponse,
    },
    InstructionFileResolve {
        response: InstructionFileResolveResponse,
    },
    HookIngest {
        response: HookIngestResponse,
    },
    Packet28Search {
        response: suite_packet_core::search::SearchResult,
    },
    Packet28SearchGuard {
        response: Packet28SearchGuardResponse,
    },
    DaemonIndexStatus {
        response: DaemonIndexStatusResponse,
    },
    DaemonIndexRebuild {
        response: DaemonIndexRebuildResponse,
    },
    DaemonIndexClear {
        response: DaemonIndexClearResponse,
    },
    Error {
        message: String,
    },
}

/// Current version of the loopback TCP transport authentication prelude.
pub const DAEMON_TRANSPORT_AUTH_SCHEMA_VERSION: u32 = 1;
/// Entropy carried by each daemon-instance transport capability.
pub const DAEMON_TRANSPORT_SECRET_BYTES: usize = 32;

/// Owner-only capability sent before any request on loopback TCP transports.
///
/// The secret is deliberately redacted from [`Debug`]. It is serialized only
/// into the owner-authenticated runtime discovery file and the first framed
/// TCP message.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonTransportAuth {
    schema_version: u32,
    secret: String,
}

impl DaemonTransportAuth {
    /// Constructs a versioned capability from 256 bits of operating-system
    /// randomness.
    pub fn from_secret_bytes(secret: [u8; DAEMON_TRANSPORT_SECRET_BYTES]) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(DAEMON_TRANSPORT_SECRET_BYTES * 2);
        for byte in secret {
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Self {
            schema_version: DAEMON_TRANSPORT_AUTH_SCHEMA_VERSION,
            secret: encoded,
        }
    }

    /// Returns the transport authentication schema version.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Compares a client prelude with this capability without
    /// secret-dependent early exit.
    pub fn authenticates(&self, candidate: &Self) -> bool {
        let schema_matches = self.has_supported_shape() && candidate.has_supported_shape();
        let secret_matches =
            constant_time_eq::constant_time_eq(self.secret.as_bytes(), candidate.secret.as_bytes());
        schema_matches & secret_matches
    }

    fn has_supported_shape(&self) -> bool {
        self.schema_version == DAEMON_TRANSPORT_AUTH_SCHEMA_VERSION
            && self.secret.len() == DAEMON_TRANSPORT_SECRET_BYTES * 2
            && self
                .secret
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }
}

impl std::fmt::Debug for DaemonTransportAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DaemonTransportAuth")
            .field("schema_version", &self.schema_version)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonRuntimeInfo {
    pub pid: u32,
    pub version: String,
    pub started_at_unix: u64,
    pub ready_at_unix: Option<u64>,
    pub socket_path: String,
    pub workspace_root: String,
    pub log_path: String,
    /// Capability required before a loopback TCP client may issue requests.
    ///
    /// Unix transports leave this unset and authenticate the peer through
    /// operating-system credentials.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_auth: Option<DaemonTransportAuth>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonStatus {
    pub pid: u32,
    pub version: String,
    pub socket_path: String,
    pub workspace_root: String,
    pub started_at_unix: u64,
    pub ready_at_unix: Option<u64>,
    pub log_path: String,
    pub uptime_secs: u64,
    pub tasks: Vec<TaskRecord>,
    pub watches: Vec<WatchRegistration>,
    pub index: Option<DaemonIndexStatusResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonEvent {
    pub kind: String,
    pub occurred_at_unix: u64,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonEventFrame {
    pub seq: u64,
    pub task_id: String,
    pub event: DaemonEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_variant_tag(request: &DaemonRequest) -> &'static str {
        match request {
            DaemonRequest::Execute { .. } => "execute",
            DaemonRequest::ExecuteSequence { .. } => "execute_sequence",
            DaemonRequest::Status => "status",
            DaemonRequest::Stop => "stop",
            DaemonRequest::TaskStatus { .. } => "task_status",
            DaemonRequest::TaskAwaitHandoff { .. } => "task_await_handoff",
            DaemonRequest::TaskMarkHandoffConsumed { .. } => "task_mark_handoff_consumed",
            DaemonRequest::TaskLaunchAgent { .. } => "task_launch_agent",
            DaemonRequest::TaskCancel { .. } => "task_cancel",
            DaemonRequest::TaskSubscribe { .. } => "task_subscribe",
            DaemonRequest::WatchList { .. } => "watch_list",
            DaemonRequest::WatchRemove { .. } => "watch_remove",
            DaemonRequest::CoverCheck { .. } => "cover_check",
            DaemonRequest::PacketFetch { .. } => "packet_fetch",
            DaemonRequest::TestShard { .. } => "test_shard",
            DaemonRequest::TestMap { .. } => "test_map",
            DaemonRequest::ContextStoreList { .. } => "context_store_list",
            DaemonRequest::ContextStoreGet { .. } => "context_store_get",
            DaemonRequest::ContextStorePrune { .. } => "context_store_prune",
            DaemonRequest::ContextStoreStats { .. } => "context_store_stats",
            DaemonRequest::ContextRecall { .. } => "context_recall",
            DaemonRequest::BrokerGetContext { .. } => "broker_get_context",
            DaemonRequest::BrokerEstimateContext { .. } => "broker_estimate_context",
            DaemonRequest::BrokerPrepareHandoff { .. } => "broker_prepare_handoff",
            DaemonRequest::BrokerValidatePlan { .. } => "broker_validate_plan",
            DaemonRequest::BrokerDecompose { .. } => "broker_decompose",
            DaemonRequest::BrokerWriteState { .. } => "broker_write_state",
            DaemonRequest::BrokerWriteStateBatch { .. } => "broker_write_state_batch",
            DaemonRequest::BrokerTaskStatus { .. } => "broker_task_status",
            DaemonRequest::ContextResolve { .. } => "context_resolve",
            DaemonRequest::InstructionFileResolve { .. } => "instruction_file_resolve",
            DaemonRequest::HookIngest { .. } => "hook_ingest",
            DaemonRequest::Packet28Search { .. } => "packet28_search",
            DaemonRequest::Packet28SearchGuard { .. } => "packet28_search_guard",
            DaemonRequest::DaemonIndexStatus { .. } => "daemon_index_status",
            DaemonRequest::DaemonIndexRebuild { .. } => "daemon_index_rebuild",
            DaemonRequest::DaemonIndexClear { .. } => "daemon_index_clear",
        }
    }

    fn response_variant_tag(response: &DaemonResponse) -> &'static str {
        match response {
            DaemonResponse::Execute { .. } => "execute",
            DaemonResponse::ExecuteSequence { .. } => "execute_sequence",
            DaemonResponse::Status { .. } => "status",
            DaemonResponse::Ack { .. } => "ack",
            DaemonResponse::TaskStatus { .. } => "task_status",
            DaemonResponse::TaskAwaitHandoff { .. } => "task_await_handoff",
            DaemonResponse::TaskMarkHandoffConsumed { .. } => "task_mark_handoff_consumed",
            DaemonResponse::TaskLaunchAgent { .. } => "task_launch_agent",
            DaemonResponse::TaskCancel { .. } => "task_cancel",
            DaemonResponse::TaskSubscribeAck { .. } => "task_subscribe_ack",
            DaemonResponse::WatchList { .. } => "watch_list",
            DaemonResponse::WatchRemove { .. } => "watch_remove",
            DaemonResponse::CoverCheck { .. } => "cover_check",
            DaemonResponse::PacketFetch { .. } => "packet_fetch",
            DaemonResponse::TestShard { .. } => "test_shard",
            DaemonResponse::TestMap { .. } => "test_map",
            DaemonResponse::ContextStoreList { .. } => "context_store_list",
            DaemonResponse::ContextStoreGet { .. } => "context_store_get",
            DaemonResponse::ContextStorePrune { .. } => "context_store_prune",
            DaemonResponse::ContextStoreStats { .. } => "context_store_stats",
            DaemonResponse::ContextRecall { .. } => "context_recall",
            DaemonResponse::BrokerGetContext { .. } => "broker_get_context",
            DaemonResponse::BrokerEstimateContext { .. } => "broker_estimate_context",
            DaemonResponse::BrokerPrepareHandoff { .. } => "broker_prepare_handoff",
            DaemonResponse::BrokerValidatePlan { .. } => "broker_validate_plan",
            DaemonResponse::BrokerDecompose { .. } => "broker_decompose",
            DaemonResponse::BrokerWriteState { .. } => "broker_write_state",
            DaemonResponse::BrokerWriteStateBatch { .. } => "broker_write_state_batch",
            DaemonResponse::BrokerTaskStatus { .. } => "broker_task_status",
            DaemonResponse::ContextResolve { .. } => "context_resolve",
            DaemonResponse::InstructionFileResolve { .. } => "instruction_file_resolve",
            DaemonResponse::HookIngest { .. } => "hook_ingest",
            DaemonResponse::Packet28Search { .. } => "packet28_search",
            DaemonResponse::Packet28SearchGuard { .. } => "packet28_search_guard",
            DaemonResponse::DaemonIndexStatus { .. } => "daemon_index_status",
            DaemonResponse::DaemonIndexRebuild { .. } => "daemon_index_rebuild",
            DaemonResponse::DaemonIndexClear { .. } => "daemon_index_clear",
            DaemonResponse::Error { .. } => "error",
        }
    }

    fn sample_kernel_response() -> KernelResponse {
        KernelResponse {
            request_id: 1,
            target: "context.assemble".to_string(),
            output_packets: Vec::new(),
            audit: suite_packet_core::kernel::KernelAudit {
                reducer: "context.assemble".to_string(),
                input_packets: 0,
                output_packets: 0,
                budget: suite_packet_core::kernel::ExecutionBudget::default(),
                input_usage: suite_packet_core::kernel::BudgetUsage::default(),
                output_usage: suite_packet_core::kernel::BudgetUsage::default(),
                total_usage: suite_packet_core::kernel::BudgetUsage::default(),
                governance: suite_packet_core::kernel::GovernanceAudit::default(),
            },
            metadata: Value::Null,
        }
    }

    fn sample_sequence_response() -> KernelSequenceResponse {
        KernelSequenceResponse {
            request_id: 1,
            scheduled: Vec::new(),
            skipped: Vec::new(),
            budget_exhausted: false,
            step_results: Vec::new(),
            metadata: Value::Null,
        }
    }

    #[test]
    fn every_request_variant_tag_is_stable() {
        let requests = vec![
            DaemonRequest::Execute {
                request: KernelRequest::default(),
            },
            DaemonRequest::ExecuteSequence {
                spec: TaskSubmitSpec::default(),
            },
            DaemonRequest::Status,
            DaemonRequest::Stop,
            DaemonRequest::TaskStatus {
                task_id: "task".to_string(),
            },
            DaemonRequest::TaskAwaitHandoff {
                request: TaskAwaitHandoffRequest::default(),
            },
            DaemonRequest::TaskMarkHandoffConsumed {
                request: TaskMarkHandoffConsumedRequest::default(),
            },
            DaemonRequest::TaskLaunchAgent {
                request: TaskLaunchAgentRequest::default(),
            },
            DaemonRequest::TaskCancel {
                task_id: "task".to_string(),
            },
            DaemonRequest::TaskSubscribe {
                task_id: "task".to_string(),
                replay_last: 0,
                after_seq: None,
            },
            DaemonRequest::WatchList { task_id: None },
            DaemonRequest::WatchRemove {
                watch_id: "watch".to_string(),
            },
            DaemonRequest::CoverCheck {
                request: CoverCheckRequest::default(),
            },
            DaemonRequest::PacketFetch {
                request: PacketFetchRequest::default(),
            },
            DaemonRequest::TestShard {
                request: TestShardRequest::default(),
            },
            DaemonRequest::TestMap {
                request: TestMapRequest::default(),
            },
            DaemonRequest::ContextStoreList {
                request: ContextStoreListRequest::default(),
            },
            DaemonRequest::ContextStoreGet {
                request: ContextStoreGetRequest::default(),
            },
            DaemonRequest::ContextStorePrune {
                request: ContextStorePruneDaemonRequest::default(),
            },
            DaemonRequest::ContextStoreStats {
                request: ContextStoreStatsRequest::default(),
            },
            DaemonRequest::ContextRecall {
                request: ContextRecallRequest::default(),
            },
            DaemonRequest::BrokerGetContext {
                request: BrokerGetContextRequest::default(),
            },
            DaemonRequest::BrokerEstimateContext {
                request: BrokerEstimateContextRequest::default(),
            },
            DaemonRequest::BrokerPrepareHandoff {
                request: BrokerPrepareHandoffRequest::default(),
            },
            DaemonRequest::BrokerValidatePlan {
                request: BrokerValidatePlanRequest::default(),
            },
            DaemonRequest::BrokerDecompose {
                request: BrokerDecomposeRequest::default(),
            },
            DaemonRequest::BrokerWriteState {
                request: BrokerWriteStateRequest::default(),
            },
            DaemonRequest::BrokerWriteStateBatch {
                request: BrokerWriteStateBatchRequest::default(),
            },
            DaemonRequest::BrokerTaskStatus {
                request: BrokerTaskStatusRequest::default(),
            },
            DaemonRequest::ContextResolve {
                request: ContextResolveRequest::default(),
            },
            DaemonRequest::InstructionFileResolve {
                request: InstructionFileResolveRequest::default(),
            },
            DaemonRequest::HookIngest {
                request: HookIngestRequest::default(),
            },
            DaemonRequest::Packet28Search {
                request: Packet28SearchRequest::default(),
            },
            DaemonRequest::Packet28SearchGuard {
                request: Packet28SearchRequest::default(),
            },
            DaemonRequest::DaemonIndexStatus {
                request: DaemonIndexStatusRequest::default(),
            },
            DaemonRequest::DaemonIndexRebuild {
                request: DaemonIndexRebuildRequest::default(),
            },
            DaemonRequest::DaemonIndexClear {
                request: DaemonIndexClearRequest::default(),
            },
        ];

        for request in requests {
            let expected = request_variant_tag(&request);
            assert_eq!(
                serde_json::to_value(request).unwrap()["type"],
                serde_json::json!(expected)
            );
        }
    }

    #[test]
    fn every_response_variant_tag_is_stable() {
        let responses = vec![
            DaemonResponse::Execute {
                response: sample_kernel_response(),
            },
            DaemonResponse::ExecuteSequence {
                response: sample_sequence_response(),
                task: TaskRecord::default(),
                watches: Vec::new(),
            },
            DaemonResponse::Status {
                status: DaemonStatus::default(),
            },
            DaemonResponse::Ack {
                message: "ok".to_string(),
            },
            DaemonResponse::TaskStatus { task: None },
            DaemonResponse::TaskAwaitHandoff {
                response: TaskAwaitHandoffResponse::default(),
            },
            DaemonResponse::TaskMarkHandoffConsumed {
                response: TaskMarkHandoffConsumedResponse::default(),
            },
            DaemonResponse::TaskLaunchAgent {
                response: TaskLaunchAgentResponse::default(),
            },
            DaemonResponse::TaskCancel {
                task: None,
                removed_watch_ids: Vec::new(),
            },
            DaemonResponse::TaskSubscribeAck {
                task_id: "task".to_string(),
                replayed: 0,
                after_seq: None,
            },
            DaemonResponse::WatchList {
                watches: Vec::new(),
            },
            DaemonResponse::WatchRemove { removed: None },
            DaemonResponse::CoverCheck {
                response: CoverCheckResponse {
                    exit_code: 0,
                    packet_type: String::new(),
                    envelope: suite_packet_core::EnvelopeV1::default(),
                },
            },
            DaemonResponse::PacketFetch {
                response: PacketFetchResponse {
                    wrapper: suite_packet_core::PacketWrapperV1::default(),
                },
            },
            DaemonResponse::TestShard {
                response: TestShardResponse::default(),
            },
            DaemonResponse::TestMap {
                response: TestMapResponse::default(),
            },
            DaemonResponse::ContextStoreList {
                response: ContextStoreListResponse::default(),
            },
            DaemonResponse::ContextStoreGet {
                response: ContextStoreGetResponse::default(),
            },
            DaemonResponse::ContextStorePrune {
                response: ContextStorePruneResponse::default(),
            },
            DaemonResponse::ContextStoreStats {
                response: ContextStoreStatsResponse::default(),
            },
            DaemonResponse::ContextRecall {
                response: ContextRecallResponse::default(),
            },
            DaemonResponse::BrokerGetContext {
                response: BrokerGetContextResponse::default(),
            },
            DaemonResponse::BrokerEstimateContext {
                response: BrokerEstimateContextResponse::default(),
            },
            DaemonResponse::BrokerPrepareHandoff {
                response: BrokerPrepareHandoffResponse::default(),
            },
            DaemonResponse::BrokerValidatePlan {
                response: BrokerValidatePlanResponse::default(),
            },
            DaemonResponse::BrokerDecompose {
                response: BrokerDecomposeResponse::default(),
            },
            DaemonResponse::BrokerWriteState {
                response: BrokerWriteStateResponse::default(),
            },
            DaemonResponse::BrokerWriteStateBatch {
                response: BrokerWriteStateBatchResponse::default(),
            },
            DaemonResponse::BrokerTaskStatus {
                response: BrokerTaskStatusResponse::default(),
            },
            DaemonResponse::ContextResolve {
                response: ContextResolveResponse {
                    source_kind: ContextSourceKind::InstructionFile,
                    source_path: None,
                    outcome: ContextResolveOutcome::Passthrough {
                        reason: "unchanged".to_string(),
                        content_sha256: None,
                        task_label: None,
                        original_bytes: None,
                    },
                },
            },
            DaemonResponse::InstructionFileResolve {
                response: InstructionFileResolveResponse {
                    path: "AGENTS.md".to_string(),
                    outcome: InstructionFileResolveOutcome::Passthrough {
                        reason: "unchanged".to_string(),
                        content_sha256: None,
                        task_label: None,
                        original_bytes: None,
                    },
                },
            },
            DaemonResponse::HookIngest {
                response: HookIngestResponse::default(),
            },
            DaemonResponse::Packet28Search {
                response: suite_packet_core::search::SearchResult::default(),
            },
            DaemonResponse::Packet28SearchGuard {
                response: Packet28SearchGuardResponse::default(),
            },
            DaemonResponse::DaemonIndexStatus {
                response: DaemonIndexStatusResponse::default(),
            },
            DaemonResponse::DaemonIndexRebuild {
                response: DaemonIndexRebuildResponse::default(),
            },
            DaemonResponse::DaemonIndexClear {
                response: DaemonIndexClearResponse::default(),
            },
            DaemonResponse::Error {
                message: "failed".to_string(),
            },
        ];

        for response in responses {
            let expected = response_variant_tag(&response);
            assert_eq!(
                serde_json::to_value(response).unwrap()["type"],
                serde_json::json!(expected)
            );
        }
    }

    #[test]
    fn task_subscribe_missing_after_seq_defaults_to_none() {
        let request: DaemonRequest = serde_json::from_value(serde_json::json!({
            "type": "task_subscribe",
            "task_id": "task-1",
            "replay_last": 10
        }))
        .unwrap();

        match request {
            DaemonRequest::TaskSubscribe {
                task_id,
                replay_last,
                after_seq,
            } => {
                assert_eq!(task_id, "task-1");
                assert_eq!(replay_last, 10);
                assert_eq!(after_seq, None);
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn task_subscribe_none_after_seq_serializes_like_legacy_request() {
        let request = DaemonRequest::TaskSubscribe {
            task_id: "task-1".to_string(),
            replay_last: 10,
            after_seq: None,
        };

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "task_subscribe",
                "task_id": "task-1",
                "replay_last": 10
            })
        );
    }

    #[test]
    fn task_subscribe_ack_missing_after_seq_defaults_to_none() {
        let response: DaemonResponse = serde_json::from_value(serde_json::json!({
            "type": "task_subscribe_ack",
            "task_id": "task-1",
            "replayed": 2
        }))
        .unwrap();

        match response {
            DaemonResponse::TaskSubscribeAck {
                task_id,
                replayed,
                after_seq,
            } => {
                assert_eq!(task_id, "task-1");
                assert_eq!(replayed, 2);
                assert_eq!(after_seq, None);
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn legacy_context_rewrite_response_defaults_new_instruction_telemetry() {
        let response: DaemonResponse = serde_json::from_value(serde_json::json!({
            "type": "context_resolve",
            "response": {
                "source_kind": "instruction_file",
                "source_path": "AGENTS.md",
                "outcome": {
                    "decision": "rewrite",
                    "content": "# compact",
                    "content_sha256": "source",
                    "task_label": "task-a",
                    "original_bytes": 100,
                    "rewritten_bytes": 10,
                    "cache_hit": false,
                    "matched_terms": [],
                    "section_titles": [],
                    "schema_version": 1
                }
            }
        }))
        .unwrap();

        let DaemonResponse::ContextResolve { response } = response else {
            panic!("expected context resolve response");
        };
        let ContextResolveOutcome::Rewrite {
            render_mode,
            stable_config_sha256,
            snapshot_sha256,
            rendered_sha256,
            ..
        } = response.outcome
        else {
            panic!("expected rewrite outcome");
        };
        assert_eq!(render_mode, InstructionRenderMode::Passthrough);
        assert!(stable_config_sha256.is_empty());
        assert_eq!(snapshot_sha256, None);
        assert!(rendered_sha256.is_empty());
    }

    #[test]
    fn legacy_context_request_defaults_to_passthrough_selection() {
        let request: DaemonRequest = serde_json::from_value(serde_json::json!({
            "type": "context_resolve",
            "request": {
                "workspace_root": "/repo",
                "source_kind": "instruction_file",
                "source_path": "AGENTS.md",
                "source_sha256": "source",
                "source_content": "# Instructions",
                "task_id": "task-a",
                "budget_tokens": 512,
                "schema_version": 1,
                "backend_kind": "linux_preload"
            }
        }))
        .unwrap();

        let DaemonRequest::ContextResolve { request } = request else {
            panic!("expected context resolve request");
        };
        assert_eq!(request.render_mode, None);
        assert_eq!(request.stable_config, None);
    }

    #[test]
    fn legacy_instruction_file_request_defaults_to_passthrough_selection() {
        let request: DaemonRequest = serde_json::from_value(serde_json::json!({
            "type": "instruction_file_resolve",
            "request": {
                "workspace_root": "/repo",
                "path": "AGENTS.md",
                "content_sha256": "source",
                "content": "# Instructions",
                "task_id": "task-a",
                "budget_tokens": 512,
                "schema_version": 1
            }
        }))
        .unwrap();

        let DaemonRequest::InstructionFileResolve { request } = request else {
            panic!("expected instruction file resolve request");
        };
        assert_eq!(request.render_mode, None);
        assert_eq!(request.stable_config, None);
    }

    #[test]
    fn legacy_instruction_file_rewrite_defaults_new_instruction_telemetry() {
        let response: DaemonResponse = serde_json::from_value(serde_json::json!({
            "type": "instruction_file_resolve",
            "response": {
                "path": "AGENTS.md",
                "outcome": {
                    "decision": "rewrite",
                    "content": "# compact",
                    "content_sha256": "source",
                    "task_label": "task-a",
                    "original_bytes": 100,
                    "rewritten_bytes": 10,
                    "cache_hit": false,
                    "matched_terms": [],
                    "section_titles": []
                }
            }
        }))
        .unwrap();

        let DaemonResponse::InstructionFileResolve { response } = response else {
            panic!("expected instruction file resolve response");
        };
        let InstructionFileResolveOutcome::Rewrite {
            render_mode,
            stable_config_sha256,
            snapshot_sha256,
            rendered_sha256,
            ..
        } = response.outcome
        else {
            panic!("expected rewrite outcome");
        };
        assert_eq!(render_mode, InstructionRenderMode::Passthrough);
        assert!(stable_config_sha256.is_empty());
        assert_eq!(snapshot_sha256, None);
        assert!(rendered_sha256.is_empty());
    }

    #[test]
    fn daemon_transport_auth_accepts_only_the_same_versioned_capability() {
        let expected =
            DaemonTransportAuth::from_secret_bytes([0x5a; DAEMON_TRANSPORT_SECRET_BYTES]);
        let wrong_secret =
            DaemonTransportAuth::from_secret_bytes([0xa5; DAEMON_TRANSPORT_SECRET_BYTES]);
        let wrong_version: DaemonTransportAuth = serde_json::from_value(serde_json::json!({
            "schema_version": DAEMON_TRANSPORT_AUTH_SCHEMA_VERSION + 1,
            "secret": "5a".repeat(DAEMON_TRANSPORT_SECRET_BYTES)
        }))
        .unwrap();

        assert!(expected.authenticates(&expected));
        assert!(!expected.authenticates(&wrong_secret));
        assert!(!expected.authenticates(&wrong_version));
    }

    #[test]
    fn daemon_transport_auth_debug_output_redacts_the_secret() {
        let auth = DaemonTransportAuth::from_secret_bytes([0x5a; DAEMON_TRANSPORT_SECRET_BYTES]);
        let rendered = format!("{auth:?}");

        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains(&"5a".repeat(DAEMON_TRANSPORT_SECRET_BYTES)));
    }

    #[test]
    fn legacy_runtime_metadata_defaults_to_unix_peer_authentication() {
        let runtime: DaemonRuntimeInfo = serde_json::from_value(serde_json::json!({
            "pid": 42,
            "socket_path": "/tmp/packet28d.sock"
        }))
        .unwrap();

        assert!(runtime.transport_auth.is_none());
    }
}
