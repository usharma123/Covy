//! Persistence and local runtime support for the Packet28 daemon.
//!
//! Wire messages, framing, and endpoint paths live in
//! `packet28-daemon-protocol`. The pre-split root imports remain available
//! unconditionally for source compatibility through the `0.2.x` line. This
//! compatibility surface is frozen: new protocol items are available only
//! from their named `packet28-daemon-protocol` modules, and the root facade may
//! be removed in `0.3.0`.
//!
//! Fallible library operations return [`DaemonCoreError`] and retain typed
//! filesystem, JSON, and framing causes. Executables may add presentation
//! context at their process boundary without losing the source chain.
//!
//! # Preferred storage API
//!
//! ```
//! use packet28_daemon_core::storage::{
//!     load_task_registry, save_task_watch_registry_checkpoint,
//! };
//! use packet28_daemon_protocol::task::{TaskRegistry, WatchRegistry};
//!
//! let directory = tempfile::tempdir()?;
//! save_task_watch_registry_checkpoint(
//!     directory.path(),
//!     &TaskRegistry::default(),
//!     &WatchRegistry::default(),
//! )?;
//! let loaded = load_task_registry(directory.path())?;
//! assert!(loaded.tasks.is_empty());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Standalone task or watch saves remain available for creating legacy,
//! unpaired state. Once either registry carries paired-checkpoint authority,
//! mutations must use `save_task_watch_registry_checkpoint`.
//!
//! Compatibility helpers remain at the crate root, but implementation and
//! protocol-module details are deliberately not part of that facade:
//!
//! ```compile_fail
//! use packet28_daemon_core::write_frame;
//! ```

#[cfg(unix)]
#[path = "retention/capability.rs"]
mod capability;
mod compat_v0;
mod error;
pub mod integrity;
pub mod retention;
pub mod storage;
pub mod task_store_lease;
pub mod trust;

pub use compat_v0::{read_socket_message, write_socket_message};
pub use error::{DaemonCoreError, Result};

// Frozen v0 root facade. Keep this list explicit so additions to the protocol
// and storage modules do not silently become daemon-core API.
pub use packet28_daemon_protocol::broker::{
    BrokerAction, BrokerDecision, BrokerDecomposeIntent, BrokerDecomposeRequest,
    BrokerDecomposeResponse, BrokerDecomposedStep, BrokerDeltaResponse,
    BrokerEstimateContextRequest, BrokerEstimateContextResponse, BrokerEvictionCandidate,
    BrokerGetContextRequest, BrokerGetContextResponse, BrokerHandoffDescriptor,
    BrokerHandoffReadiness, BrokerHandoffStatus, BrokerPacketRef, BrokerPlanStep,
    BrokerPlanViolation, BrokerPrepareHandoffRequest, BrokerPrepareHandoffResponse, BrokerQuestion,
    BrokerRecommendedAction, BrokerResolvedQuestion, BrokerResponseMode, BrokerSection,
    BrokerSectionEstimate, BrokerSourceKind, BrokerSupersessionMode, BrokerTaskStatusRequest,
    BrokerTaskStatusResponse, BrokerToolResultKind, BrokerValidatePlanRequest,
    BrokerValidatePlanResponse, BrokerVerbosity, BrokerWriteOp, BrokerWriteStateBatchRequest,
    BrokerWriteStateBatchResponse, BrokerWriteStateRequest, BrokerWriteStateResponse,
};
pub use packet28_daemon_protocol::commands::{
    CoverCheckRequest, CoverCheckResponse, PacketFetchRequest, PacketFetchResponse,
    SequenceSubmitResponse, TaskSubmitSpec, TestMapRequest, TestMapResponse, TestMapSummary,
    TestShardRequest, TestShardResponse, WatchKind, WatchSpec,
};
pub use packet28_daemon_protocol::context_store::{
    ContextRecallRequest, ContextRecallResponse, ContextStoreGetRequest, ContextStoreGetResponse,
    ContextStoreListRequest, ContextStoreListResponse, ContextStorePruneDaemonRequest,
    ContextStorePruneResponse, ContextStoreStatsRequest, ContextStoreStatsResponse,
};
pub use packet28_daemon_protocol::frame::MAX_SOCKET_MESSAGE_BYTES;
pub use packet28_daemon_protocol::hooks::{
    ActiveTaskRecord, HookBoundaryKind, HookEventKind, HookIngestRequest, HookIngestResponse,
    HookLifecycleEvent, HookLifecycleKind, HookReducerCacheEntry, HookReducerPacket,
    HookRuntimeConfig, RelaunchPreference, ThresholdLevel,
};
pub use packet28_daemon_protocol::index::{
    DaemonIndexClearRequest, DaemonIndexClearResponse, DaemonIndexManifest,
    DaemonIndexRebuildRequest, DaemonIndexRebuildResponse, DaemonIndexState,
    DaemonIndexStateParseError, DaemonIndexStatusRequest, DaemonIndexStatusResponse,
    DaemonIndexTransitionError,
};
pub use packet28_daemon_protocol::message::{
    ContextBackendKind, ContextResolveOutcome, ContextResolveRequest, ContextResolveResponse,
    ContextSourceKind, DaemonEvent, DaemonEventFrame, DaemonRequest, DaemonResponse,
    DaemonRuntimeInfo, DaemonStatus, InstructionFileResolveOutcome, InstructionFileResolveRequest,
    InstructionFileResolveResponse, InstructionRenderMode, InstructionStableConfig,
    Packet28SearchGuardResponse, Packet28SearchRequest,
};
pub use packet28_daemon_protocol::paths::{
    active_task_path, agent_runtime_dir, daemon_dir, hook_runtime_config_path, index_dir,
    index_manifest_path, index_snapshot_path, log_path, pid_path, ready_path,
    resolve_workspace_root, runtime_path, socket_path, task_artifact_dir, task_artifacts_dir,
    task_brief_json_path, task_brief_markdown_path, task_event_log_path, task_events_dir,
    task_registry_path, task_state_json_path, task_version_json_path, task_versions_dir,
    watch_registry_path, workspace_socket_path, AGENT_ACTIVE_TASK_FILE_NAME, DAEMON_DIR_NAME,
    HOOK_RUNTIME_CONFIG_FILE_NAME, INDEX_DIR_NAME, INDEX_MANIFEST_FILE_NAME,
    INDEX_SNAPSHOT_FILE_NAME, LOG_FILE_NAME, PID_FILE_NAME, READY_FILE_NAME, RUNTIME_FILE_NAME,
    SOCKET_FILE_NAME, TASK_ARTIFACTS_DIR_NAME, TASK_BRIEF_JSON_FILE_NAME,
    TASK_BRIEF_MARKDOWN_FILE_NAME, TASK_EVENTS_DIR_NAME, TASK_REGISTRY_FILE_NAME,
    TASK_STATE_JSON_FILE_NAME, WATCH_REGISTRY_FILE_NAME,
};
pub use packet28_daemon_protocol::task::{
    TaskAwaitHandoffRequest, TaskAwaitHandoffResponse, TaskLaunchAgentRequest,
    TaskLaunchAgentResponse, TaskLifecycle, TaskLifecycleAction, TaskLifecycleTransitionError,
    TaskMarkHandoffConsumedRequest, TaskMarkHandoffConsumedResponse, TaskRecord, TaskRegistry,
    WatchRegistration, WatchRegistry,
};
pub use storage::{
    append_task_event, ensure_daemon_dir, load_task_events, load_task_events_from_offset,
    load_task_registry, load_watch_registry, now_unix, read_runtime_info, remove_runtime_files,
    save_task_registry, save_watch_registry, task_event_log_len, write_runtime_info,
    TaskEventLogRead,
};
