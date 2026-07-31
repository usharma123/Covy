use super::*;

#[test]
fn executes_sequence_in_dependency_order() {
    let mut kernel = Kernel::new();
    kernel.register_reducer("step.a", |_ctx, _packets| {
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(json!({"step":"a"}), None)],
            metadata: Value::Null,
        })
    });
    kernel.register_reducer("step.b", |_ctx, _packets| {
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(json!({"step":"b"}), None)],
            metadata: Value::Null,
        })
    });

    let response = kernel
        .execute_sequence(KernelSequenceRequest {
            budget: ExecutionBudget {
                token_cap: Some(100),
                byte_cap: None,
                runtime_ms_cap: None,
            },
            reactive: ReactiveSequenceConfig::default(),
            steps: vec![
                KernelStepRequest {
                    id: "b".to_string(),
                    target: "step.b".to_string(),
                    depends_on: vec!["a".to_string()],
                    input_packets: vec![],
                    ..KernelStepRequest::default()
                },
                KernelStepRequest {
                    id: "a".to_string(),
                    target: "step.a".to_string(),
                    depends_on: vec![],
                    input_packets: vec![],
                    ..KernelStepRequest::default()
                },
            ],
        })
        .unwrap();

    assert_eq!(response.scheduled, vec!["a".to_string(), "b".to_string()]);
    assert!(response.skipped.is_empty());
}

#[test]
fn sequence_autofills_missing_step_ids_and_resolves_dependencies() {
    let mut kernel = Kernel::new();
    kernel.register_reducer("step.a", |_ctx, _packets| Ok(ReducerResult::default()));
    kernel.register_reducer("step.b", |_ctx, _packets| Ok(ReducerResult::default()));

    let response = kernel
        .execute_sequence(KernelSequenceRequest {
            budget: ExecutionBudget::default(),
            reactive: ReactiveSequenceConfig::default(),
            steps: vec![
                KernelStepRequest {
                    target: "step.a".to_string(),
                    ..KernelStepRequest::default()
                },
                KernelStepRequest {
                    id: " custom ".to_string(),
                    target: "step.b".to_string(),
                    depends_on: vec!["step-a-0".to_string()],
                    ..KernelStepRequest::default()
                },
            ],
        })
        .unwrap();

    assert_eq!(
        response.scheduled,
        vec!["step-a-0".to_string(), "custom".to_string()]
    );
    assert_eq!(response.step_results[0].id, "step-a-0");
    assert_eq!(response.step_results[1].id, "custom");
}

#[test]
fn sequence_rejects_empty_targets() {
    let kernel = Kernel::new();

    let err = kernel
        .execute_sequence(KernelSequenceRequest {
            budget: ExecutionBudget::default(),
            reactive: ReactiveSequenceConfig::default(),
            steps: vec![KernelStepRequest::default()],
        })
        .unwrap_err();

    assert!(
        matches!(err, KernelError::InvalidRequest { detail } if detail == "sequence step 0 target cannot be empty")
    );
}

#[test]
fn sequence_rejects_duplicate_resolved_ids() {
    let kernel = Kernel::new();

    let err = kernel
        .execute_sequence(KernelSequenceRequest {
            budget: ExecutionBudget::default(),
            reactive: ReactiveSequenceConfig::default(),
            steps: vec![
                KernelStepRequest {
                    id: "step-a-1".to_string(),
                    target: "step.a".to_string(),
                    ..KernelStepRequest::default()
                },
                KernelStepRequest {
                    target: "step.a".to_string(),
                    ..KernelStepRequest::default()
                },
            ],
        })
        .unwrap_err();

    assert!(
        matches!(err, KernelError::InvalidRequest { detail } if detail == "sequence step id 'step-a-1' must be unique")
    );
}

#[test]
fn sequence_respects_scheduler_budget_cutoff() {
    let mut kernel = Kernel::new();
    kernel.register_reducer("step.a", |_ctx, _packets| Ok(ReducerResult::default()));
    kernel.register_reducer("step.b", |_ctx, _packets| Ok(ReducerResult::default()));

    let packet = KernelPacket {
        body: json!({"size":"large"}),
        token_usage: Some(90),
        ..KernelPacket::default()
    };
    let response = kernel
        .execute_sequence(KernelSequenceRequest {
            budget: ExecutionBudget {
                token_cap: Some(100),
                byte_cap: None,
                runtime_ms_cap: None,
            },
            reactive: ReactiveSequenceConfig::default(),
            steps: vec![
                KernelStepRequest {
                    id: "a".to_string(),
                    target: "step.a".to_string(),
                    input_packets: vec![packet.clone()],
                    ..KernelStepRequest::default()
                },
                KernelStepRequest {
                    id: "b".to_string(),
                    target: "step.b".to_string(),
                    input_packets: vec![packet],
                    ..KernelStepRequest::default()
                },
            ],
        })
        .unwrap();

    assert!(response.budget_exhausted);
    assert_eq!(response.scheduled, vec!["a".to_string()]);
    assert_eq!(response.skipped, vec!["b".to_string()]);
}

#[test]
fn sequence_skips_dependent_step_after_failure() {
    let mut kernel = Kernel::new();
    kernel.register_reducer("step.fail", |_ctx, _packets| {
        Err(KernelError::ReducerFailed {
            target: "step.fail".to_string(),
            detail: "boom".to_string(),
        })
    });
    kernel.register_reducer("step.after", |_ctx, _packets| Ok(ReducerResult::default()));

    let response = kernel
        .execute_sequence(KernelSequenceRequest {
            budget: ExecutionBudget::default(),
            reactive: ReactiveSequenceConfig::default(),
            steps: vec![
                KernelStepRequest {
                    id: "fail".to_string(),
                    target: "step.fail".to_string(),
                    ..KernelStepRequest::default()
                },
                KernelStepRequest {
                    id: "after".to_string(),
                    target: "step.after".to_string(),
                    depends_on: vec!["fail".to_string()],
                    ..KernelStepRequest::default()
                },
            ],
        })
        .unwrap();

    let after = response
        .step_results
        .iter()
        .find(|step| step.id == "after")
        .unwrap();
    assert_eq!(after.status, "skipped");
}

#[test]
fn reactive_sequence_cancels_completed_steps_and_releases_dependencies() {
    let kernel = Kernel::with_v1_reducers();
    kernel
        .execute(KernelRequest {
            target: "agenty.state.write".to_string(),
            reducer_input: json!({
                "task_id": "task-reactive",
                "event_id": "evt-1",
                "occurred_at_unix": 1,
                "actor": "agent",
                "kind": "step_completed",
                "data": {
                    "type": "step_completed",
                    "step_id": "map-step"
                }
            }),
            ..KernelRequest::default()
        })
        .unwrap();

    let mut kernel = kernel;
    kernel.register_reducer("step.noop", |_ctx, _packets| Ok(ReducerResult::default()));

    let response = kernel
        .execute_sequence(KernelSequenceRequest {
            budget: ExecutionBudget::default(),
            reactive: ReactiveSequenceConfig {
                enabled: true,
                task_id: Some("task-reactive".to_string()),
                append_focused_map: false,
                mode: ReactiveReplanMode::Basic,
            },
            steps: vec![
                KernelStepRequest {
                    id: "map-step".to_string(),
                    target: "step.noop".to_string(),
                    ..KernelStepRequest::default()
                },
                KernelStepRequest {
                    id: "final-step".to_string(),
                    target: "step.noop".to_string(),
                    depends_on: vec!["map-step".to_string()],
                    ..KernelStepRequest::default()
                },
            ],
        })
        .unwrap();

    assert_eq!(response.scheduled, vec!["final-step".to_string()]);
    assert!(response.skipped.contains(&"map-step".to_string()));
    assert!(response
        .metadata
        .get("reactive")
        .and_then(|reactive| reactive.get("replans"))
        .and_then(Value::as_array)
        .is_some_and(|replans| !replans.is_empty()));
}

#[test]
fn reactive_sequence_replaces_map_steps_after_focus_update() {
    let dir = tempdir().unwrap();
    setup_diff_repo(dir.path());

    let mut kernel = Kernel::with_v1_reducers();
    kernel.register_reducer("custom.focus", |ctx, _packets| {
        let event = suite_packet_core::AgentStateEventPayload {
            task_id: "task-focus".to_string(),
            event_id: "focus-1".to_string(),
            occurred_at_unix: 2,
            actor: "agent".to_string(),
            kind: suite_packet_core::AgentStateEventKind::FocusSet,
            paths: vec!["src/alpha.rs".to_string()],
            symbols: Vec::new(),
            data: suite_packet_core::AgentStateEventData::FocusSet { note: None },
        };
        let (_, packet) = build_agent_state_packet(&ctx.target, &event, "custom.focus")?;
        Ok(ReducerResult {
            output_packets: vec![packet],
            metadata: json!({"source":"custom.focus"}),
        })
    });

    let response = kernel
        .execute_sequence(KernelSequenceRequest {
            budget: ExecutionBudget::default(),
            reactive: ReactiveSequenceConfig {
                enabled: true,
                task_id: Some("task-focus".to_string()),
                append_focused_map: false,
                mode: ReactiveReplanMode::Basic,
            },
            steps: vec![
                KernelStepRequest {
                    id: "focus".to_string(),
                    target: "custom.focus".to_string(),
                    ..KernelStepRequest::default()
                },
                KernelStepRequest {
                    id: "map".to_string(),
                    target: "mapy.repo".to_string(),
                    reducer_input: serde_json::to_value(mapy_core::RepoMapRequest {
                        repo_root: dir.path().to_string_lossy().to_string(),
                        focus_paths: Vec::new(),
                        focus_symbols: Vec::new(),
                        max_files: 10,
                        max_symbols: 20,
                        include_tests: false,
                    })
                    .unwrap(),
                    ..KernelStepRequest::default()
                },
            ],
        })
        .unwrap();

    let map_response = response
        .step_results
        .iter()
        .find(|step| step.id == "map")
        .and_then(|step| step.response.as_ref())
        .unwrap();
    assert_eq!(
        map_response
            .metadata
            .get("focus_paths")
            .and_then(Value::as_array)
            .and_then(|paths| paths.first())
            .and_then(Value::as_str),
        Some("src/alpha.rs")
    );
}

#[test]
fn reactive_mutations_can_append_focused_map_followup() {
    let original = vec![KernelStepRequest {
        id: "map".to_string(),
        target: "mapy.repo".to_string(),
        reducer_input: serde_json::to_value(mapy_core::RepoMapRequest {
            repo_root: ".".to_string(),
            focus_paths: Vec::new(),
            focus_symbols: Vec::new(),
            max_files: 10,
            max_symbols: 20,
            include_tests: false,
        })
        .unwrap(),
        ..KernelStepRequest::default()
    }];
    let remaining = vec![KernelStepRequest {
        id: "other".to_string(),
        target: "step.noop".to_string(),
        ..KernelStepRequest::default()
    }];
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        task_id: "task-focus".to_string(),
        focus_paths: vec!["src/alpha.rs".to_string()],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };

    let mutations = build_reactive_kernel_mutations(
        &remaining,
        &original,
        &snapshot,
        &BTreeSet::new(),
        ReactiveReplanMode::Basic,
        true,
        Some("other"),
    );

    assert!(mutations.iter().any(|mutation| matches!(
        mutation,
        KernelPlanMutation::Append { step, .. }
            if step.id == "map__reactive_focus"
            && step.depends_on == vec!["other".to_string()]
    )));
}
