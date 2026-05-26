use super::*;
use crate::instruction_files::resolve_context;
use packet28_daemon_core::{
    ContextBackendKind, ContextResolveOutcome, ContextResolveRequest, ContextSourceKind,
    InstructionFileResolveOutcome, InstructionFileResolveRequest,
};

mod action_critic;
mod budget;
mod code_evidence;
mod context_debt;
mod evidence_confidence;
mod handoff;
mod plan_validation;
mod search;
mod support;
use support::*;

#[test]
fn explicit_limits_override_verbosity_alias() {
    let mut section_limits = BTreeMap::new();
    section_limits.insert("relevant_context".to_string(), 2);
    let limits = resolve_effective_limits(
        BrokerAction::Plan,
        Some(BrokerVerbosity::Rich),
        Some(3),
        Some(5),
        &section_limits,
    );
    assert_eq!(limits.max_sections, 3);
    assert_eq!(limits.default_max_items_per_section, 5);
    assert_eq!(limits.section_item_limits["relevant_context"], 2);
}

#[test]
fn omitted_explicit_limits_use_deterministic_action_defaults() {
    let plan_limits =
        resolve_effective_limits(BrokerAction::Plan, None, None, None, &BTreeMap::new());
    let choose_tool_limits =
        resolve_effective_limits(BrokerAction::ChooseTool, None, None, None, &BTreeMap::new());
    assert_eq!(plan_limits.max_sections, 8);
    assert_eq!(plan_limits.default_max_items_per_section, 8);
    assert_eq!(plan_limits.section_item_limits["code_evidence"], 6);
    assert_eq!(choose_tool_limits.max_sections, 6);
    assert_eq!(choose_tool_limits.default_max_items_per_section, 5);
}

#[test]
fn brief_always_starts_with_supersession_header() {
    let brief = render_brief(
        "task-123",
        "7",
        &[BrokerSection {
            id: "task_objective".to_string(),
            title: "Task Objective".to_string(),
            body: "Investigate auth flow".to_string(),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        }],
    );
    assert!(brief.starts_with("[Packet28 Context v7"));
    assert!(brief.contains("supersedes all prior Packet28 context"));
}

#[test]
fn relevant_context_renders_human_summaries_without_debug_ids() {
    let request = BrokerGetContextRequest {
        task_id: "task-summary".to_string(),
        include_sections: vec!["relevant_context".to_string()],
        ..BrokerGetContextRequest::default()
    };
    let snapshot = suite_packet_core::AgentSnapshotPayload::default();
    let manage = suite_packet_core::ContextManagePayload {
        working_set: vec![
            suite_packet_core::ContextManagePacketRef {
                cache_key: "cache-1".to_string(),
                target: "packet28.broker_memory.write".to_string(),
                score: 9.0,
                summary: Some(
                    "Checkpoint handoff for task-summary: inspect Alpha before editing it"
                        .to_string(),
                ),
                reason: Some("curated_memory".to_string()),
                source_tier: Some(suite_packet_core::MemorySourceTier::CuratedMemory),
                memory_kind: Some(suite_packet_core::MemoryKind::Handoff),
                packet_types: vec!["suite.packet28.broker_memory.v1".to_string()],
                est_tokens: 24,
                est_bytes: 96,
                runtime_ms: 1,
            },
            suite_packet_core::ContextManagePacketRef {
                cache_key: "cache-2".to_string(),
                target: "contextq.manage".to_string(),
                score: 7.0,
                summary: Some(
                    "task memory for task-summary: 2 relevant packet(s), 1 recommended action(s)"
                        .to_string(),
                ),
                reason: Some("curated_memory".to_string()),
                source_tier: Some(suite_packet_core::MemorySourceTier::CuratedMemory),
                memory_kind: Some(suite_packet_core::MemoryKind::Brief),
                packet_types: vec!["suite.context.manage.v1".to_string()],
                est_tokens: 18,
                est_bytes: 72,
                runtime_ms: 1,
            },
        ],
        ..suite_packet_core::ContextManagePayload::default()
    };

    let sections = build_broker_sections(
        Path::new("."),
        &daemon_test_state(),
        &request,
        &snapshot,
        Some(&manage),
        None,
    );
    let relevant_context = sections
        .iter()
        .find(|section| section.id == "relevant_context")
        .expect("relevant_context section should exist");
    assert!(relevant_context
        .body
        .contains("Checkpoint handoff for task-summary"));
    assert!(!relevant_context.body.contains("cache-1"));
    assert!(!relevant_context
        .body
        .contains("packet28.broker_memory.write"));
}

#[test]
fn relevant_context_marks_and_downranks_stale_changed_path_context() {
    let request = BrokerGetContextRequest {
        task_id: "task-stale-context".to_string(),
        include_sections: vec!["relevant_context".to_string()],
        ..BrokerGetContextRequest::default()
    };
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_paths_since_checkpoint: vec!["src/stale.rs".to_string()],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let manage = suite_packet_core::ContextManagePayload {
        working_set: vec![
            suite_packet_core::ContextManagePacketRef {
                cache_key: "cache-stale".to_string(),
                target: "contextq.manage".to_string(),
                score: 9.0,
                summary: Some("cached notes for src/stale.rs before the edit".to_string()),
                reason: Some("curated_memory".to_string()),
                source_tier: Some(suite_packet_core::MemorySourceTier::CuratedMemory),
                memory_kind: Some(suite_packet_core::MemoryKind::Evidence),
                packet_types: vec!["suite.context.manage.v1".to_string()],
                est_tokens: 18,
                est_bytes: 72,
                runtime_ms: 1,
            },
            suite_packet_core::ContextManagePacketRef {
                cache_key: "cache-fresh".to_string(),
                target: "contextq.manage".to_string(),
                score: 7.0,
                summary: Some("general implementation notes".to_string()),
                reason: Some("curated_memory".to_string()),
                source_tier: Some(suite_packet_core::MemorySourceTier::CuratedMemory),
                memory_kind: Some(suite_packet_core::MemoryKind::Brief),
                packet_types: vec!["suite.context.manage.v1".to_string()],
                est_tokens: 18,
                est_bytes: 72,
                runtime_ms: 1,
            },
        ],
        ..suite_packet_core::ContextManagePayload::default()
    };

    let sections = build_broker_sections(
        Path::new("."),
        &daemon_test_state(),
        &request,
        &snapshot,
        Some(&manage),
        None,
    );
    let relevant_context = sections
        .iter()
        .find(|section| section.id == "relevant_context")
        .expect("relevant_context section should exist");
    assert!(relevant_context
        .body
        .contains("[stale_after_change: refresh src/stale.rs]"));
    let fresh_idx = relevant_context
        .body
        .find("general implementation notes")
        .expect("fresh context should render");
    let stale_idx = relevant_context
        .body
        .find("cached notes for src/stale.rs")
        .expect("stale context should render");
    assert!(
        fresh_idx < stale_idx,
        "stale changed-path context should render after fresh context"
    );
}

#[test]
fn active_decisions_render_related_paths_and_symbols() {
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        active_decisions: vec![suite_packet_core::AgentDecision {
            id: "hypothesis:auth-cache".to_string(),
            text: "hypothesis active: Auth cache invalidation is suspect".to_string(),
            related_paths: vec!["src/auth.rs".to_string()],
            related_symbols: vec!["AuthCache".to_string()],
            related_artifact_ids: vec!["artifact-auth-cache".to_string()],
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let request = BrokerGetContextRequest {
        task_id: "task-hypothesis-evidence".to_string(),
        action: Some(BrokerAction::Inspect),
        ..BrokerGetContextRequest::default()
    };
    let sections = build_broker_sections(
        Path::new("."),
        &daemon_test_state(),
        &request,
        &snapshot,
        None,
        None,
    );
    let active_decisions = sections
        .iter()
        .find(|section| section.id == "active_decisions")
        .expect("active_decisions section should exist");
    assert!(active_decisions.body.contains("paths=src/auth.rs"));
    assert!(active_decisions.body.contains("symbols=AuthCache"));
    assert!(active_decisions
        .body
        .contains("artifacts=artifact-auth-cache"));
}

#[test]
fn broker_context_surfaces_failure_advice_from_run_savings() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    std::fs::create_dir_all(root.join(".packet28")).unwrap();
    std::fs::write(
        root.join(".packet28").join("run-savings.jsonl"),
        [
            serde_json::json!({
                "command": "cargo test failing_case",
                "cwd": root.display().to_string(),
                "family": "rust",
                "canonical_kind": "cargo_test",
                "exit_code": 101,
                "raw_est_tokens": 100,
                "reduced_est_tokens": 20,
                "savings_percent": 80.0,
                "fallback_reason": null,
                "failure_fingerprint": "failure:v1:abc",
                "changed_paths": [],
                "timestamp_unix_ms": 1
            })
            .to_string(),
            serde_json::json!({
                "command": "cargo test fixed_case",
                "cwd": root.display().to_string(),
                "family": "rust",
                "canonical_kind": "cargo_test",
                "exit_code": 0,
                "raw_est_tokens": 100,
                "reduced_est_tokens": 20,
                "savings_percent": 80.0,
                "fallback_reason": null,
                "failure_fingerprint": null,
                "changed_paths": ["src/fix.rs"],
                "timestamp_unix_ms": 2
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    let sections = build_broker_sections(
        &root,
        &state,
        &BrokerGetContextRequest {
            task_id: "task-failure-advice".to_string(),
            action: Some(BrokerAction::Plan),
            include_sections: vec!["failure_advice".to_string()],
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        None,
        None,
    );

    let advice = sections
        .iter()
        .find(|section| section.id == "failure_advice")
        .expect("failure advice section should render from run savings");
    assert!(advice.body.contains("failure:v1:abc"));
    assert!(advice.body.contains("cargo test fixed_case"));
    assert!(advice.body.contains("paths=src/fix.rs"));
}

#[test]
fn inherit_broker_request_defaults_reuses_previous_follow_up_shape() {
    let previous = BrokerGetContextRequest {
        task_id: "task-a".to_string(),
        action: Some(BrokerAction::Inspect),
        budget_tokens: Some(700),
        budget_bytes: Some(2800),
        focus_paths: vec!["src/alpha.rs".to_string()],
        focus_symbols: vec!["Alpha".to_string()],
        query: Some("Where is Alpha defined?".to_string()),
        include_sections: vec!["task_objective".to_string(), "code_evidence".to_string()],
        verbosity: Some(BrokerVerbosity::Rich),
        response_mode: Some(BrokerResponseMode::Delta),
        max_sections: Some(5),
        default_max_items_per_section: Some(3),
        section_item_limits: BTreeMap::from([("code_evidence".to_string(), 2)]),
        persist_artifacts: Some(true),
        ..BrokerGetContextRequest::default()
    };
    let mut current = BrokerGetContextRequest {
        task_id: "task-a".to_string(),
        ..BrokerGetContextRequest::default()
    };

    inherit_broker_request_defaults(&mut current, Some(&previous));

    assert_eq!(current.action, Some(BrokerAction::Inspect));
    assert_eq!(current.query.as_deref(), Some("Where is Alpha defined?"));
    assert_eq!(current.focus_paths, vec!["src/alpha.rs"]);
    assert_eq!(current.focus_symbols, vec!["Alpha"]);
    assert_eq!(
        current.include_sections,
        vec!["task_objective".to_string(), "code_evidence".to_string()]
    );
    assert_eq!(current.response_mode, Some(BrokerResponseMode::Delta));
    assert_eq!(current.section_item_limits["code_evidence"], 2);
}

#[test]
fn reducer_search_only_runs_when_evidence_sections_are_allowed() {
    let only_summary = HashSet::from(["task_objective".to_string(), "progress".to_string()]);
    assert!(!should_run_reducer_search(&only_summary));

    let with_search = HashSet::from(["search_evidence".to_string()]);
    assert!(should_run_reducer_search(&with_search));

    let with_code = HashSet::from(["code_evidence".to_string()]);
    assert!(should_run_reducer_search(&with_code));
}

#[test]
fn broker_edit_context_surfaces_evidence_freshness_for_changed_paths() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        files_read: vec!["src/fresh.rs".to_string()],
        changed_paths_since_checkpoint: vec![
            "src/fresh.rs".to_string(),
            "src/stale.rs".to_string(),
        ],
        changed_symbols_since_checkpoint: vec!["StaleSymbol".to_string()],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let sections = build_broker_sections(
        &root,
        &state,
        &BrokerGetContextRequest {
            task_id: "task-freshness".to_string(),
            action: Some(BrokerAction::Edit),
            include_sections: vec!["evidence_freshness".to_string()],
            ..BrokerGetContextRequest::default()
        },
        &snapshot,
        None,
        None,
    );

    let freshness = sections
        .iter()
        .find(|section| section.id == "evidence_freshness")
        .expect("changed paths should produce evidence freshness section");
    assert!(freshness.body.contains(
        "freshness_score: 1/2 changed path(s) have fresh reads; 1 path(s) and 1 symbol(s) need refresh"
    ));
    assert!(freshness
        .body
        .contains("src/fresh.rs (fresh read recorded)"));
    assert!(freshness
        .body
        .contains("src/stale.rs (refresh read/search before relying on cached evidence)"));
    assert!(freshness.body.contains("StaleSymbol"));
}

#[test]
fn render_task_memory_lines_surfaces_recent_state() {
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        files_read: vec!["src/alpha.rs".to_string()],
        latest_intention: Some(suite_packet_core::AgentIntention {
            text: "Inspect Alpha before editing".to_string(),
            note: Some("Need a clean handoff breadcrumb".to_string()),
            step_id: Some("investigating".to_string()),
            paths: vec!["src/alpha.rs".to_string()],
            occurred_at_unix: 1,
            ..suite_packet_core::AgentIntention::default()
        }),
        latest_checkpoint_id: Some("cp-1".to_string()),
        checkpoint_note: Some("Validated shuffle scope".to_string()),
        checkpoint_focus_paths: vec!["src/alpha.rs".to_string()],
        checkpoint_focus_symbols: vec!["Alpha".to_string()],
        changed_paths_since_checkpoint: vec!["src/beta.rs".to_string()],
        changed_symbols_since_checkpoint: vec!["Beta".to_string()],
        evidence_artifact_ids: vec!["artifact-1".to_string()],
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "tool-1".to_string(),
            sequence: 7,
            tool_name: "manual.read".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Read,
            request_summary: Some("Read alpha".to_string()),
            result_summary: Some("Found Alpha".to_string()),
            paths: vec!["src/alpha.rs".to_string()],
            symbols: vec!["Alpha".to_string()],
            occurred_at_unix: 1,
            ..suite_packet_core::ToolInvocationSummary::default()
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };

    let rendered = render_task_memory_lines(&snapshot);

    assert!(rendered.iter().any(
        |line| line.contains("latest intention [investigating]: Inspect Alpha before editing")
    ));
    assert!(rendered
        .iter()
        .any(|line| line.contains("latest intention note: Need a clean handoff breadcrumb")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("latest tool: manual.read")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("recently read: src/alpha.rs")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("latest checkpoint: cp-1")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("checkpoint note: Validated shuffle scope")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("checkpoint focus path: src/alpha.rs")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("checkpoint focus symbol: Alpha")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("changed since checkpoint: src/beta.rs")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("changed symbol since checkpoint: Beta")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("evidence artifact: artifact-1")));
}

#[test]
fn instruction_file_resolution_rewrites_larger_markdown() {
    let state = daemon_test_state();
    let workspace_root = state.lock().unwrap().root.display().to_string();
    let response = resolve_instruction_file(
        state,
        InstructionFileResolveRequest {
            workspace_root,
            path: "AGENTS.md".to_string(),
            content_sha256: String::new(),
            content: "# Coverage\n\n- Prefer reducer outputs that stay bounded.\n- Avoid redundant tool chatter.\n\n## Auth\nTouch src/auth.rs carefully and preserve task state.\n\n## Testing\nRun targeted checks before widening scope.\n".to_string(),
            task_id: Some("task-virtualize".to_string()),
            budget_tokens: Some(128),
            schema_version: 1,
        },
    )
    .unwrap();

    match response.outcome {
        InstructionFileResolveOutcome::Rewrite {
            content,
            task_label,
            original_bytes,
            rewritten_bytes,
            ..
        } => {
            assert!(content.starts_with("# [p28:virtual] sha256:"));
            assert_eq!(task_label, "task-virtualize");
            assert!(rewritten_bytes < original_bytes);
        }
        other => panic!("expected rewrite response, got {other:?}"),
    }
}

#[test]
fn instruction_file_resolution_fails_open_when_summary_is_not_smaller() {
    let state = daemon_test_state();
    let response = resolve_instruction_file(
        state,
        InstructionFileResolveRequest {
            workspace_root: ".".to_string(),
            path: "AGENTS.md".to_string(),
            content_sha256: String::new(),
            content: "short".to_string(),
            task_id: None,
            budget_tokens: Some(128),
            schema_version: 1,
        },
    )
    .unwrap();

    match response.outcome {
        InstructionFileResolveOutcome::Passthrough { reason, .. } => {
            assert_eq!(reason, "not_smaller_than_original");
        }
        other => panic!("expected passthrough response, got {other:?}"),
    }
}

#[test]
fn context_resolve_rewrites_instruction_file_and_preserves_metadata() {
    let state = daemon_test_state();
    let workspace_root = state.lock().unwrap().root.display().to_string();
    let response = resolve_context(
        state,
        ContextResolveRequest {
            workspace_root,
            source_kind: ContextSourceKind::InstructionFile,
            source_path: Some("AGENTS.md".to_string()),
            source_sha256: String::new(),
            source_content: "# Coverage\n\n- Prefer reducer outputs that stay bounded.\n- Avoid redundant tool chatter.\n\n## Auth\nTouch src/auth.rs carefully and preserve task state.\n\n## Testing\nRun targeted checks before widening scope.\n".to_string(),
            task_id: Some("task-virtualize".to_string()),
            task_label: None,
            budget_tokens: Some(128),
            schema_version: 7,
            agent_family: Some("claude".to_string()),
            backend_kind: ContextBackendKind::LinuxPreload,
        },
    )
    .unwrap();

    assert_eq!(response.source_kind, ContextSourceKind::InstructionFile);
    assert_eq!(response.source_path.as_deref(), Some("AGENTS.md"));
    match response.outcome {
        ContextResolveOutcome::Rewrite {
            content,
            task_label,
            original_bytes,
            rewritten_bytes,
            schema_version,
            ..
        } => {
            assert!(content.starts_with("# [p28:virtual] sha256:"));
            assert_eq!(task_label, "task-virtualize");
            assert!(rewritten_bytes < original_bytes);
            assert_eq!(schema_version, 7);
        }
        other => panic!("expected rewrite response, got {other:?}"),
    }
}

#[test]
fn instruction_file_resolution_compatibility_matches_context_resolve_decision() {
    let state = daemon_test_state();
    let workspace_root = state.lock().unwrap().root.display().to_string();
    let legacy = resolve_instruction_file(
        state.clone(),
        InstructionFileResolveRequest {
            workspace_root: workspace_root.clone(),
            path: "AGENTS.md".to_string(),
            content_sha256: String::new(),
            content: "# Coverage\n\n- Prefer reducer outputs that stay bounded.\n- Avoid redundant tool chatter.\n\n## Auth\nTouch src/auth.rs carefully and preserve task state.\n".to_string(),
            task_id: Some("task-compat".to_string()),
            budget_tokens: Some(128),
            schema_version: 3,
        },
    )
    .unwrap();
    let generic = resolve_context(
        state,
        ContextResolveRequest {
            workspace_root,
            source_kind: ContextSourceKind::InstructionFile,
            source_path: Some("AGENTS.md".to_string()),
            source_sha256: String::new(),
            source_content: "# Coverage\n\n- Prefer reducer outputs that stay bounded.\n- Avoid redundant tool chatter.\n\n## Auth\nTouch src/auth.rs carefully and preserve task state.\n".to_string(),
            task_id: Some("task-compat".to_string()),
            task_label: None,
            budget_tokens: Some(128),
            schema_version: 3,
            agent_family: Some("generic".to_string()),
            backend_kind: ContextBackendKind::Unknown,
        },
    )
    .unwrap();

    match (legacy.outcome, generic.outcome) {
        (
            InstructionFileResolveOutcome::Rewrite {
                task_label: legacy_task,
                original_bytes: legacy_original,
                rewritten_bytes: legacy_rewritten,
                ..
            },
            ContextResolveOutcome::Rewrite {
                task_label: generic_task,
                original_bytes: generic_original,
                rewritten_bytes: generic_rewritten,
                ..
            },
        ) => {
            assert_eq!(legacy_task, generic_task);
            assert_eq!(legacy_original, generic_original);
            assert_eq!(legacy_rewritten, generic_rewritten);
        }
        other => panic!("expected matching rewrite decisions, got {other:?}"),
    }
}
