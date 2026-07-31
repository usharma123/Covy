#![expect(
    unused_imports,
    reason = "the compile fixture imports every symbol in the frozen compatibility facade"
)]

use std::{collections::BTreeSet, io::Cursor};

macro_rules! frozen_root_facade {
    ($($name:ident),+ $(,)?) => {
        use packet28_daemon_core::{ $($name),+ };
        const FROZEN_ROOT_EXPORTS: &[&str] = &[$(stringify!($name)),+];
    };
}

frozen_root_facade! {
    active_task_path, agent_runtime_dir, append_task_event, daemon_dir, ensure_daemon_dir,
    hook_runtime_config_path, index_dir, index_manifest_path, index_snapshot_path, integrity,
    load_task_events, load_task_events_from_offset, load_task_registry, load_watch_registry,
    log_path, now_unix, pid_path, read_runtime_info, read_socket_message, ready_path,
    remove_runtime_files, resolve_workspace_root, retention, runtime_path, save_task_registry,
    save_watch_registry, socket_path, storage, task_artifact_dir, task_artifacts_dir,
    task_brief_json_path, task_brief_markdown_path, task_event_log_len, task_event_log_path,
    task_events_dir, task_registry_path, task_state_json_path, task_store_lease,
    task_version_json_path, task_versions_dir, trust, watch_registry_path, workspace_socket_path,
    write_runtime_info, write_socket_message, ActiveTaskRecord, BrokerAction, BrokerDecision,
    BrokerDecomposeIntent, BrokerDecomposeRequest, BrokerDecomposeResponse, BrokerDecomposedStep,
    BrokerDeltaResponse, BrokerEstimateContextRequest, BrokerEstimateContextResponse,
    BrokerEvictionCandidate, BrokerGetContextRequest, BrokerGetContextResponse,
    BrokerHandoffDescriptor, BrokerHandoffReadiness, BrokerHandoffStatus, BrokerPacketRef,
    BrokerPlanStep, BrokerPlanViolation, BrokerPrepareHandoffRequest,
    BrokerPrepareHandoffResponse, BrokerQuestion, BrokerRecommendedAction, BrokerResolvedQuestion,
    BrokerResponseMode, BrokerSection, BrokerSectionEstimate, BrokerSourceKind,
    BrokerSupersessionMode, BrokerTaskStatusRequest, BrokerTaskStatusResponse,
    BrokerToolResultKind, BrokerValidatePlanRequest, BrokerValidatePlanResponse, BrokerVerbosity,
    BrokerWriteOp, BrokerWriteStateBatchRequest, BrokerWriteStateBatchResponse,
    BrokerWriteStateRequest, BrokerWriteStateResponse, ContextBackendKind, ContextRecallRequest,
    ContextRecallResponse, ContextResolveOutcome, ContextResolveRequest, ContextResolveResponse,
    ContextSourceKind, ContextStoreGetRequest, ContextStoreGetResponse, ContextStoreListRequest,
    ContextStoreListResponse, ContextStorePruneDaemonRequest, ContextStorePruneResponse,
    ContextStoreStatsRequest, ContextStoreStatsResponse, CoverCheckRequest, CoverCheckResponse,
    DaemonCoreError, DaemonEvent, DaemonEventFrame, DaemonIndexClearRequest,
    DaemonIndexClearResponse, DaemonIndexManifest, DaemonIndexRebuildRequest,
    DaemonIndexRebuildResponse, DaemonIndexState, DaemonIndexStateParseError,
    DaemonIndexStatusRequest, DaemonIndexStatusResponse, DaemonIndexTransitionError, DaemonRequest,
    DaemonResponse, DaemonRuntimeInfo, DaemonStatus, HookBoundaryKind, HookEventKind,
    HookIngestRequest, HookIngestResponse, HookLifecycleEvent, HookLifecycleKind,
    HookReducerCacheEntry, HookReducerPacket, HookRuntimeConfig, InstructionFileResolveOutcome,
    InstructionFileResolveRequest, InstructionFileResolveResponse, InstructionRenderMode,
    InstructionStableConfig, Packet28SearchGuardResponse, Packet28SearchRequest,
    PacketFetchRequest, PacketFetchResponse, RelaunchPreference, Result, SequenceSubmitResponse,
    TaskAwaitHandoffRequest, TaskAwaitHandoffResponse, TaskEventLogRead, TaskLaunchAgentRequest,
    TaskLaunchAgentResponse, TaskLifecycle, TaskLifecycleAction, TaskLifecycleTransitionError,
    TaskMarkHandoffConsumedRequest, TaskMarkHandoffConsumedResponse, TaskRecord, TaskRegistry,
    TaskSubmitSpec, TestMapRequest, TestMapResponse, TestMapSummary, TestShardRequest,
    TestShardResponse, ThresholdLevel, WatchKind, WatchRegistration, WatchRegistry, WatchSpec,
    AGENT_ACTIVE_TASK_FILE_NAME, DAEMON_DIR_NAME, HOOK_RUNTIME_CONFIG_FILE_NAME, INDEX_DIR_NAME,
    INDEX_MANIFEST_FILE_NAME, INDEX_SNAPSHOT_FILE_NAME, LOG_FILE_NAME, MAX_SOCKET_MESSAGE_BYTES,
    PID_FILE_NAME, READY_FILE_NAME, RUNTIME_FILE_NAME, SOCKET_FILE_NAME, TASK_ARTIFACTS_DIR_NAME,
    TASK_BRIEF_JSON_FILE_NAME, TASK_BRIEF_MARKDOWN_FILE_NAME, TASK_EVENTS_DIR_NAME,
    TASK_REGISTRY_FILE_NAME, TASK_STATE_JSON_FILE_NAME, WATCH_REGISTRY_FILE_NAME,
}

fn root_public_items(source: &str) -> BTreeSet<String> {
    let mut items = BTreeSet::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if let Some(module) = trimmed
            .strip_prefix("pub mod ")
            .and_then(|value| value.strip_suffix(';'))
        {
            assert!(
                items.insert(module.to_string()),
                "duplicate public root item {module}"
            );
            continue;
        }
        let Some(mut declaration) = trimmed.strip_prefix("pub use ").map(str::to_owned) else {
            continue;
        };
        while !declaration.ends_with(';') {
            declaration.push(' ');
            declaration.push_str(
                lines
                    .next()
                    .expect("public use declaration must end with a semicolon")
                    .trim(),
            );
        }
        declaration.pop();
        if let Some((_, grouped)) = declaration.split_once('{') {
            let grouped = grouped
                .strip_suffix('}')
                .expect("grouped public use must end with a closing brace");
            assert!(
                !grouped.contains('{'),
                "nested public use is not inventoried"
            );
            for item in grouped
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                assert!(
                    !item.contains("::"),
                    "nested public use item is not inventoried"
                );
                assert!(
                    items.insert(item.to_string()),
                    "duplicate public root item {item}"
                );
            }
        } else {
            let item = declaration
                .split_whitespace()
                .last()
                .expect("public use must name an item");
            let item = if declaration.contains(" as ") {
                item
            } else {
                item.rsplit("::")
                    .next()
                    .expect("public use path is nonempty")
            };
            assert!(
                items.insert(item.to_string()),
                "duplicate public root item {item}"
            );
        }
    }
    items
}

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
    let expected = FROZEN_ROOT_EXPORTS
        .iter()
        .map(|item| (*item).to_string())
        .collect::<BTreeSet<_>>();
    let actual = root_public_items(root);

    assert!(!root.contains("pub use compat_v0::*"));
    assert!(!adapter.contains("::*"));
    assert!(root.contains("Frozen v0 root facade"));
    assert!(root.contains("packet28_daemon_protocol::broker::{"));
    assert!(root.contains("packet28_daemon_protocol::message::{"));
    assert!(root.contains("packet28_daemon_protocol::paths::{"));
    assert!(root.contains("pub use storage::{"));
    assert_eq!(
        expected.len(),
        FROZEN_ROOT_EXPORTS.len(),
        "frozen root allowlist contains a duplicate"
    );
    assert_eq!(actual, expected);
}

#[test]
fn legacy_root_inventory_detects_added_and_removed_exports() {
    let root = include_str!("../src/lib.rs");
    let actual = root_public_items(root);

    let added = root_public_items(&format!("{root}\npub use storage::surprise_export;\n"));
    assert_eq!(added.len(), actual.len() + 1);
    assert!(added.contains("surprise_export"));

    let removed_source = root.replacen(", remove_runtime_files,", ",", 1);
    assert_ne!(removed_source, root);
    let removed = root_public_items(&removed_source);
    assert_eq!(removed.len() + 1, actual.len());
    assert!(!removed.contains("remove_runtime_files"));
}
