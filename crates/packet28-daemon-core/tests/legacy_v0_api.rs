#![expect(
    unused_imports,
    reason = "the compile fixture imports every symbol in the frozen compatibility facade"
)]

use std::io::Cursor;

use packet28_daemon_core::{
    active_task_path, agent_runtime_dir, append_task_event, daemon_dir, ensure_daemon_dir,
    hook_runtime_config_path, index_dir, index_manifest_path, index_snapshot_path, integrity,
    load_task_events, load_task_events_from_offset, load_task_registry, load_watch_registry,
    log_path, now_unix, pid_path, read_runtime_info, read_socket_message, ready_path,
    resolve_workspace_root, retention, runtime_path, save_task_registry, save_watch_registry,
    socket_path, storage, task_artifact_dir, task_artifacts_dir, task_brief_json_path,
    task_brief_markdown_path, task_event_log_len, task_event_log_path, task_events_dir,
    task_registry_path, task_state_json_path, task_store_lease, task_version_json_path,
    task_versions_dir, trust, watch_registry_path, workspace_socket_path, write_runtime_info,
    write_socket_message, ActiveTaskRecord, BrokerAction, BrokerDecision, BrokerDecomposeIntent,
    BrokerDecomposeRequest, BrokerDecomposeResponse, BrokerDecomposedStep, BrokerDeltaResponse,
    BrokerEstimateContextRequest, BrokerEstimateContextResponse, BrokerEvictionCandidate,
    BrokerGetContextRequest, BrokerGetContextResponse, BrokerHandoffDescriptor,
    BrokerHandoffReadiness, BrokerHandoffStatus, BrokerPacketRef, BrokerPlanStep,
    BrokerPlanViolation, BrokerPrepareHandoffRequest, BrokerPrepareHandoffResponse, BrokerQuestion,
    BrokerRecommendedAction, BrokerResolvedQuestion, BrokerResponseMode, BrokerSection,
    BrokerSectionEstimate, BrokerSourceKind, BrokerSupersessionMode, BrokerTaskStatusRequest,
    BrokerTaskStatusResponse, BrokerToolResultKind, BrokerValidatePlanRequest,
    BrokerValidatePlanResponse, BrokerVerbosity, BrokerWriteOp, BrokerWriteStateBatchRequest,
    BrokerWriteStateBatchResponse, BrokerWriteStateRequest, BrokerWriteStateResponse,
    ContextBackendKind, ContextRecallRequest, ContextRecallResponse, ContextResolveOutcome,
    ContextResolveRequest, ContextResolveResponse, ContextSourceKind, ContextStoreGetRequest,
    ContextStoreGetResponse, ContextStoreListRequest, ContextStoreListResponse,
    ContextStorePruneDaemonRequest, ContextStorePruneResponse, ContextStoreStatsRequest,
    ContextStoreStatsResponse, CoverCheckRequest, CoverCheckResponse, DaemonCoreError, DaemonEvent,
    DaemonEventFrame, DaemonIndexClearRequest, DaemonIndexClearResponse, DaemonIndexManifest,
    DaemonIndexRebuildRequest, DaemonIndexRebuildResponse, DaemonIndexState,
    DaemonIndexStateParseError, DaemonIndexStatusRequest, DaemonIndexStatusResponse,
    DaemonIndexTransitionError, DaemonRequest, DaemonResponse, DaemonRuntimeInfo, DaemonStatus,
    HookBoundaryKind, HookEventKind, HookIngestRequest, HookIngestResponse, HookLifecycleEvent,
    HookLifecycleKind, HookReducerCacheEntry, HookReducerPacket, HookRuntimeConfig,
    InstructionFileResolveOutcome, InstructionFileResolveRequest, InstructionFileResolveResponse,
    InstructionRenderMode, InstructionStableConfig, Packet28SearchGuardResponse,
    Packet28SearchRequest, PacketFetchRequest, PacketFetchResponse, RelaunchPreference,
    Result as DaemonCoreResult, SequenceSubmitResponse, TaskAwaitHandoffRequest,
    TaskAwaitHandoffResponse, TaskEventLogRead, TaskLaunchAgentRequest, TaskLaunchAgentResponse,
    TaskLifecycle, TaskLifecycleAction, TaskLifecycleTransitionError,
    TaskMarkHandoffConsumedRequest, TaskMarkHandoffConsumedResponse, TaskRecord, TaskRegistry,
    TaskSubmitSpec, TestMapRequest, TestMapResponse, TestMapSummary, TestShardRequest,
    TestShardResponse, ThresholdLevel, WatchKind, WatchRegistration, WatchRegistry, WatchSpec,
    AGENT_ACTIVE_TASK_FILE_NAME, DAEMON_DIR_NAME, HOOK_RUNTIME_CONFIG_FILE_NAME, INDEX_DIR_NAME,
    INDEX_MANIFEST_FILE_NAME, INDEX_SNAPSHOT_FILE_NAME, LOG_FILE_NAME, MAX_SOCKET_MESSAGE_BYTES,
    PID_FILE_NAME, READY_FILE_NAME, RUNTIME_FILE_NAME, SOCKET_FILE_NAME, TASK_ARTIFACTS_DIR_NAME,
    TASK_BRIEF_JSON_FILE_NAME, TASK_BRIEF_MARKDOWN_FILE_NAME, TASK_EVENTS_DIR_NAME,
    TASK_REGISTRY_FILE_NAME, TASK_STATE_JSON_FILE_NAME, WATCH_REGISTRY_FILE_NAME,
};

#[test]
fn legacy_root_paths_remain_source_and_wire_compatible() {
    let _watch = WatchSpec::default();
    let _recall = ContextRecallRequest::default();
    let _broker = BrokerGetContextRequest::default();
    let _hook = HookIngestRequest::default();
    let _index = DaemonIndexStatusRequest::default();
    let _await_handoff = TaskAwaitHandoffRequest::default();
    let _registry = TaskRegistry::default();

    let root = std::path::Path::new("/workspace");
    assert!(daemon_dir(root).ends_with(".packet28/daemon"));
    assert_eq!(
        socket_path(root)
            .extension()
            .and_then(|value| value.to_str()),
        Some("sock")
    );

    let mut bytes = Vec::new();
    write_socket_message(&mut bytes, &DaemonRequest::Status).unwrap();
    let request: DaemonRequest = read_socket_message(&mut Cursor::new(bytes)).unwrap();
    assert!(matches!(request, DaemonRequest::Status));

    let response = DaemonResponse::Ack {
        message: "ready".to_string(),
    };
    assert_eq!(
        serde_json::to_value(response).unwrap()["type"],
        serde_json::json!("ack")
    );
}

#[test]
fn legacy_root_facade_is_an_explicit_frozen_allowlist() {
    let root = include_str!("../src/lib.rs");
    let adapter = include_str!("../src/compat_v0.rs");

    assert!(!root.contains("pub use compat_v0::*"));
    assert!(!adapter.contains("::*"));
    assert!(root.contains("Frozen v0 root facade"));
    assert!(root.contains("packet28_daemon_protocol::broker::{"));
    assert!(root.contains("packet28_daemon_protocol::message::{"));
    assert!(root.contains("packet28_daemon_protocol::paths::{"));
    assert!(root.contains("pub use storage::{"));
}
