use super::support::*;
use super::*;

#[test]
fn compute_handoff_state_requires_checkpoint_and_tracks_newer_intentions() {
    let empty_snapshot = suite_packet_core::AgentSnapshotPayload::default();
    let (ready_without_checkpoint, _) = compute_handoff_state(None, &empty_snapshot);
    assert!(!ready_without_checkpoint);

    let snapshot = suite_packet_core::AgentSnapshotPayload {
        latest_checkpoint_id: Some("cp-1".to_string()),
        latest_intention: Some(suite_packet_core::AgentIntention {
            text: "Resume editing beta".to_string(),
            occurred_at_unix: 20,
            ..suite_packet_core::AgentIntention::default()
        }),
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let (ready_initial, _) = compute_handoff_state(None, &snapshot);
    assert!(ready_initial);

    let task = TaskRecord {
        task_id: "task-a".to_string(),
        latest_handoff_generated_at_unix: Some(10),
        latest_handoff_checkpoint_id: Some("cp-1".to_string()),
        ..TaskRecord::default()
    };
    let (ready_newer_intention, _) = compute_handoff_state(Some(&task), &snapshot);
    assert!(ready_newer_intention);

    let stale_snapshot = suite_packet_core::AgentSnapshotPayload {
        latest_checkpoint_id: Some("cp-1".to_string()),
        latest_intention: Some(suite_packet_core::AgentIntention {
            text: "Resume editing beta".to_string(),
            occurred_at_unix: 5,
            ..suite_packet_core::AgentIntention::default()
        }),
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let (ready_stale, _) = compute_handoff_state(Some(&task), &stale_snapshot);
    assert!(!ready_stale);
}

#[test]
fn compute_handoff_state_accepts_newer_hook_boundaries_with_legacy_second_timestamps() {
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        latest_intention: Some(suite_packet_core::AgentIntention {
            text: "Resume editing beta".to_string(),
            occurred_at_unix: 1_700_000_001_500,
            ..suite_packet_core::AgentIntention::default()
        }),
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let task = TaskRecord {
        task_id: "task-legacy-boundary".to_string(),
        latest_handoff_generated_at_unix: Some(1_700_000_000_250),
        latest_hook_boundary_at_unix: Some(1_700_000_001),
        latest_hook_boundary_kind: Some("stop".to_string()),
        ..TaskRecord::default()
    };

    let (ready, reason) = compute_handoff_state(Some(&task), &snapshot);

    assert!(ready);
    assert!(reason.contains("stop"));
}

#[test]
fn checkpoint_context_lines_surface_saved_focus() {
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        latest_checkpoint_id: Some("cp-42".to_string()),
        checkpoint_note: Some("Seeded shuffle plan".to_string()),
        checkpoint_focus_paths: vec![
            "apache/src/main/java/org/apache/commons/lang3/ArrayUtils.java".to_string(),
        ],
        checkpoint_focus_symbols: vec!["shuffle".to_string()],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };

    let rendered = render_checkpoint_context_lines(&snapshot);

    assert!(rendered
        .iter()
        .any(|line| line.contains("checkpoint: cp-42")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("note: Seeded shuffle plan")));
    assert!(rendered.iter().any(|line| line
        .contains("focus path: apache/src/main/java/org/apache/commons/lang3/ArrayUtils.java")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("focus symbol: shuffle")));
}

#[test]
fn prepare_handoff_only_resumes_recorded_handoff_artifacts() {
    let state = daemon_test_state();
    let root = state.lock().unwrap().root.clone();
    let context = BrokerGetContextResponse {
        context_version: "ctx-1".to_string(),
        response_mode: BrokerResponseMode::Full,
        artifact_id: Some("ctx-1".to_string()),
        brief: "context".to_string(),
        ..BrokerGetContextResponse::default()
    };
    insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: "task-resume-guard".to_string(),
            ..TaskRecord::default()
        },
    );
    let task_id = TaskStorageId::try_from("task-resume-guard").unwrap();
    let context_version = ContextVersionStorageId::try_from("ctx-1").unwrap();
    let version_path = task_version_json_path(&root, &task_id, &context_version);
    std::fs::create_dir_all(version_path.parent().unwrap()).unwrap();
    std::fs::write(&version_path, serde_json::to_vec_pretty(&context).unwrap()).unwrap();

    {
        let mut guard = state.lock().unwrap();
        let task = guard.tasks.tasks.get_mut("task-resume-guard").unwrap();
        task.latest_context_version = Some("ctx-1".to_string());
        task.latest_handoff_artifact_id = Some("handoff-1".to_string());
        persist_state(&guard).unwrap();
    }

    let response = broker_prepare_handoff(
        state,
        BrokerPrepareHandoffRequest {
            task_id: "task-resume-guard".to_string(),
            query: None,
            response_mode: Some(BrokerResponseMode::Slim),
            include_debug_memory: false,
        },
    )
    .unwrap();

    assert!(!response.handoff_ready);
    assert!(response.context.is_none());
    assert_eq!(
        response.latest_handoff_artifact_id.as_deref(),
        Some("handoff-1")
    );
}

#[test]
fn prepare_handoff_warns_when_tool_evidence_contradicts_active_hypothesis() {
    let state = daemon_test_state();
    state.lock().unwrap().agent_snapshots.insert(
        "task-contradiction".to_string(),
        suite_packet_core::AgentSnapshotPayload {
            task_id: "task-contradiction".to_string(),
            latest_checkpoint_id: Some("checkpoint-1".to_string()),
            active_decisions: vec![suite_packet_core::AgentDecision {
                id: "hypothesis:auth-cache".to_string(),
                text: "hypothesis active: Auth cache invalidation is suspect".to_string(),
                related_paths: vec!["src/auth.rs".to_string()],
                related_symbols: vec!["AuthCache".to_string()],
                related_artifact_ids: Vec::new(),
            }],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "tool-contradict".to_string(),
                sequence: 42,
                tool_name: "cargo test".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some(
                    "refuted hypothesis auth-cache while testing src/auth.rs".to_string(),
                ),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    let response = broker_prepare_handoff(
        state,
        BrokerPrepareHandoffRequest {
            task_id: "task-contradiction".to_string(),
            query: None,
            response_mode: Some(BrokerResponseMode::Slim),
            include_debug_memory: false,
        },
    )
    .unwrap();

    assert!(response.handoff_ready);
    assert_eq!(response.warnings.len(), 1);
    assert!(response.warnings[0].contains("handoff_contradiction"));
    assert!(response.warnings[0].contains("hypothesis:auth-cache"));
    assert!(response.warnings[0].contains("tool #42"));
    assert_eq!(response.readiness.status, "caution");
    assert!(response.readiness.score < 85);
    assert!(response
        .readiness
        .reasons
        .iter()
        .any(|reason| reason == "contradictions=1"));
}

#[test]
fn prepare_handoff_readiness_score_rises_after_verification_evidence() {
    let state = daemon_test_state();
    for (task_id, with_verification) in [
        ("task-readiness-unverified", false),
        ("task-readiness-verified", true),
    ] {
        state.lock().unwrap().agent_snapshots.insert(
            task_id.to_string(),
            suite_packet_core::AgentSnapshotPayload {
                task_id: task_id.to_string(),
                latest_checkpoint_id: Some("checkpoint-1".to_string()),
                changed_paths_since_checkpoint: vec!["src/lib.rs".to_string()],
                latest_intention: Some(suite_packet_core::AgentIntention {
                    text: "Hand off after library edit".to_string(),
                    occurred_at_unix: 1,
                    ..suite_packet_core::AgentIntention::default()
                }),
                recent_tool_invocations: if with_verification {
                    vec![suite_packet_core::ToolInvocationSummary {
                        invocation_id: "test-1".to_string(),
                        sequence: 7,
                        tool_name: "cargo test".to_string(),
                        operation_kind: suite_packet_core::ToolOperationKind::Test,
                        result_summary: Some("tests passed".to_string()),
                        ..suite_packet_core::ToolInvocationSummary::default()
                    }]
                } else {
                    Vec::new()
                },
                ..suite_packet_core::AgentSnapshotPayload::default()
            },
        );
    }

    let unverified = broker_prepare_handoff(
        state.clone(),
        BrokerPrepareHandoffRequest {
            task_id: "task-readiness-unverified".to_string(),
            response_mode: Some(BrokerResponseMode::Slim),
            ..BrokerPrepareHandoffRequest::default()
        },
    )
    .unwrap();
    let verified = broker_prepare_handoff(
        state,
        BrokerPrepareHandoffRequest {
            task_id: "task-readiness-verified".to_string(),
            response_mode: Some(BrokerResponseMode::Slim),
            ..BrokerPrepareHandoffRequest::default()
        },
    )
    .unwrap();

    assert!(verified.readiness.score > unverified.readiness.score);
    assert!(unverified
        .readiness
        .reasons
        .iter()
        .any(|reason| reason == "missing_recent_verification"));
    assert!(!verified
        .readiness
        .reasons
        .iter()
        .any(|reason| reason == "missing_recent_verification"));
    assert!(serde_json::to_string(&verified.readiness).unwrap().len() < 512);
}
