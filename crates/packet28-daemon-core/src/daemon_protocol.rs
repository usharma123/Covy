use super::*;

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
    pub request: packet28_reducer_core::SearchRequest,
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
        response: packet28_reducer_core::SearchResult,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonRuntimeInfo {
    pub pid: u32,
    pub started_at_unix: u64,
    pub ready_at_unix: Option<u64>,
    pub socket_path: String,
    pub workspace_root: String,
    pub log_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonStatus {
    pub pid: u32,
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
