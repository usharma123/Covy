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
fn broker_evidence_confidence_distinguishes_stale_paths_from_changed_symbols() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_paths_since_checkpoint: vec!["src/auth.rs".to_string()],
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("stale_paths=1"));
    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("confidence: medium"));
    assert!(confidence.contains("risk=freshness_mixed"));
    assert!(confidence.contains("payoff=refresh stale_paths+changed_symbols"));
}

#[test]
fn broker_evidence_confidence_payoff_priority_orders_repair_actions() {
    assert_eq!(confidence_payoff(100, 1, 1, 1, 1, 1), "evidence usable");
    assert_eq!(
        confidence_payoff(55, 1, 1, 1, 1, 1),
        "rerun failing evidence"
    );
    assert_eq!(
        confidence_payoff(60, 1, 1, 1, 0, 1),
        "refresh stale_paths+changed_symbols"
    );
    assert_eq!(
        confidence_payoff(75, 0, 1, 0, 0, 1),
        "capture artifact-backed symbol evidence"
    );
    assert_eq!(
        confidence_payoff(80, 0, 1, 0, 0, 0),
        "refresh changed_symbols"
    );
    assert_eq!(
        confidence_payoff(80, 0, 0, 1, 0, 0),
        "replace fallback_records"
    );
    assert_eq!(
        confidence_payoff(70, 0, 0, 1, 0, 1),
        "replace fallback_records"
    );
    assert_eq!(
        confidence_payoff(80, 0, 0, 0, 0, 1),
        "capture artifact-backed evidence"
    );
}

#[test]
fn broker_evidence_confidence_risk_priority_matches_repair_actions() {
    assert_eq!(confidence_risk(100, 1, 1, 1, 1, 1), "none");
    assert_eq!(confidence_risk(55, 1, 1, 1, 1, 1), "failures");
    assert_eq!(confidence_risk(60, 1, 1, 1, 0, 1), "freshness_mixed");
    assert_eq!(confidence_risk(75, 1, 0, 1, 0, 1), "stale_paths");
    assert_eq!(confidence_risk(75, 0, 1, 0, 0, 1), "missing_backing");
    assert_eq!(confidence_risk(80, 0, 1, 0, 0, 0), "changed_symbols");
    assert_eq!(confidence_risk(80, 0, 0, 1, 0, 0), "fallback_records");
    assert_eq!(confidence_risk(70, 0, 0, 1, 0, 1), "fallback_records");
}

#[test]
fn broker_evidence_confidence_reason_lines_stay_stable() {
    let state = daemon_test_state();
    let backed_success = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            evidence_artifact_ids: vec!["artifact-test".to_string()],
            recent_tool_invocations: vec![tool_invocation(
                "test-backed",
                1,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests passed"),
                Some("artifact-test"),
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let unbacked_symbol = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![tool_invocation(
                "test-unbacked-symbol",
                2,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests passed"),
                None,
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let mixed_freshness = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_paths_since_checkpoint: vec!["src/auth.rs".to_string()],
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert_eq!(
        broker_evidence_confidence_reason_line(&backed_success),
        "- confidence_reason: source=local_tool_state verification=fresh artifacts=1 backing=artifact risk=none payoff=evidence usable"
    );
    assert_eq!(
        broker_evidence_confidence_reason_line(&unbacked_symbol),
        "- confidence_reason: source=local_tool_state verification=fresh artifacts=0 backing=missing risk=missing_backing payoff=capture artifact-backed symbol evidence"
    );
    assert_eq!(
        broker_evidence_confidence_reason_line(&mixed_freshness),
        "- confidence_reason: source=local_tool_state verification=missing artifacts=0 backing=missing risk=freshness_mixed payoff=refresh stale_paths+changed_symbols"
    );
    for reason_line in [
        broker_evidence_confidence_reason_line(&backed_success),
        broker_evidence_confidence_reason_line(&unbacked_symbol),
        broker_evidence_confidence_reason_line(&mixed_freshness),
    ] {
        assert!(
            reason_line.len() <= 180,
            "reason line too wide: {reason_line}"
        );
    }
    for body in [&backed_success, &unbacked_symbol, &mixed_freshness] {
        assert!(body.len() <= 512, "confidence body too large: {body}");
    }
}

#[test]
fn broker_evidence_confidence_scores_stale_or_fallback_below_fresh_success() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    std::fs::create_dir_all(root.join(".packet28")).unwrap();
    std::fs::write(
        root.join(".packet28/run-savings.jsonl"),
        serde_json::json!({
            "command": "Packet28 run -- rg stale",
            "cwd": root.display().to_string(),
            "family": "search",
            "canonical_kind": "rg",
            "exit_code": 0,
            "raw_est_tokens": 500,
            "reduced_est_tokens": 100,
            "savings_percent": 80.0,
            "fallback_reason": "fff auto preferred backend failed: launch error",
            "failure_fingerprint": null,
            "changed_paths": [],
            "timestamp_unix_ms": 10
        })
        .to_string(),
    )
    .unwrap();
    let stale_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_paths_since_checkpoint: vec!["src/stale.rs".to_string()],
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "search-1".to_string(),
            sequence: 1,
            tool_name: "rg".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Search,
            result_summary: Some("fallback search result".to_string()),
            ..suite_packet_core::ToolInvocationSummary::default()
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let fresh_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_paths_since_checkpoint: vec!["src/stale.rs".to_string()],
        files_read: vec!["src/stale.rs".to_string()],
        evidence_artifact_ids: vec!["artifact-test".to_string()],
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "test-1".to_string(),
            sequence: 2,
            tool_name: "cargo test".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Test,
            result_summary: Some("tests passed".to_string()),
            artifact_id: Some("artifact-test".to_string()),
            ..suite_packet_core::ToolInvocationSummary::default()
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let stale = broker_evidence_confidence_body(&state, stale_snapshot);
    let fresh = broker_evidence_confidence_body(&state, fresh_snapshot);

    assert!(stale.contains("stale_paths=1"));
    assert!(stale.contains("fallback_records=1"));
    assert!(stale.contains("confidence: low"));
    assert!(stale.contains("risk=stale_paths"));
    assert!(stale.contains("payoff=refresh stale_paths"));
    assert!(fresh.contains("confidence: high"));
    assert!(fresh.contains("verification=fresh"));
    assert!(fresh.contains("risk=none"));
    assert!(fresh.contains("payoff=evidence usable"));
}

#[test]
fn broker_evidence_confidence_penalizes_symbol_only_staleness() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("stale_paths=0"));
    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("confidence: medium"));
    assert!(confidence.contains("risk=changed_symbols"));
    assert!(confidence.contains("payoff=refresh changed_symbols"));
}

#[test]
fn broker_evidence_confidence_keeps_symbol_staleness_visible_after_verification() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            evidence_artifact_ids: vec!["artifact-test".to_string()],
            recent_tool_invocations: vec![tool_invocation(
                "test-1",
                2,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests passed"),
                Some("artifact-test"),
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("confidence: high"));
    assert!(confidence.contains("verification=fresh"));
}

#[test]
fn broker_evidence_confidence_scores_failed_symbol_verification_low() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![tool_invocation(
                "test-1",
                2,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests failed"),
                None,
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("failures=1"));
    assert!(confidence.contains("confidence: low"));
    assert!(confidence.contains("verification=missing"));
    assert!(confidence.contains("risk=failures"));
    assert!(confidence.contains("payoff=rerun failing evidence"));
}

#[test]
fn broker_evidence_confidence_scores_unbacked_symbol_verification_medium() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![tool_invocation(
                "test-1",
                2,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests passed"),
                None,
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("artifact_gaps=1"));
    assert!(confidence.contains("backing=missing"));
    assert!(confidence.contains("confidence: medium"));
    assert!(confidence.contains("verification=fresh"));
    assert!(confidence.contains("artifacts=0 backing=missing risk=missing_backing"));
    assert!(confidence.contains("payoff=capture artifact-backed symbol evidence"));
}

#[test]
fn broker_evidence_confidence_scores_repeated_artifact_gaps_medium() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            recent_tool_invocations: vec![
                tool_invocation(
                    "search-1",
                    1,
                    "rg AuthCache",
                    suite_packet_core::ToolOperationKind::Search,
                    Some("matched AuthCache"),
                    None,
                ),
                tool_invocation(
                    "read-1",
                    2,
                    "read src/auth.rs",
                    suite_packet_core::ToolOperationKind::Read,
                    Some("read auth cache code"),
                    None,
                ),
            ],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("artifact_gaps=2"));
    assert!(confidence.contains("backing=missing"));
    assert!(confidence.contains("confidence: medium"));
    assert!(confidence.contains("risk=missing_backing"));
    assert!(confidence.contains("payoff=capture artifact-backed evidence"));
}

#[test]
fn broker_evidence_confidence_caps_missing_backing_below_high() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            recent_tool_invocations: vec![tool_invocation(
                "test-1",
                1,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests passed"),
                None,
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("artifact_gaps=1"));
    assert!(confidence.contains("backing=missing"));
    assert!(confidence.contains("score=84"));
    assert!(confidence.contains("confidence: medium"));
    assert!(confidence.contains("risk=missing_backing"));
    assert!(confidence.contains("payoff=capture artifact-backed evidence"));
}

#[test]
fn broker_evidence_confidence_missing_backing_keeps_score_spread() {
    let state = daemon_test_state();
    let symbol_only = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let unbacked_success = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            recent_tool_invocations: vec![tool_invocation(
                "test-1",
                1,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests passed"),
                None,
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let failed_symbol = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-2".to_string(),
                sequence: 2,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests failed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(symbol_only.contains("score=80"));
    assert!(symbol_only.contains("confidence: medium"));
    assert!(unbacked_success.contains("score=84"));
    assert!(unbacked_success.contains("confidence: medium"));
    assert!(failed_symbol.contains("score=35"));
    assert!(failed_symbol.contains("confidence: low"));
}

#[test]
fn broker_evidence_confidence_orders_symbol_evidence_tiers() {
    let state = daemon_test_state();
    let backed_verified = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            evidence_artifact_ids: vec!["artifact-test".to_string()],
            recent_tool_invocations: vec![tool_invocation(
                "test-backed",
                1,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests passed"),
                Some("artifact-test"),
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let unbacked_verified = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![tool_invocation(
                "test-unbacked",
                2,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests passed"),
                None,
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let failed = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![tool_invocation(
                "test-failed",
                3,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests failed"),
                None,
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(backed_verified.contains("confidence: high"));
    assert!(backed_verified.contains("artifact_gaps=0"));
    assert!(backed_verified.contains("backing=artifact"));
    assert!(backed_verified.contains("artifacts=1 backing=artifact"));
    assert!(unbacked_verified.contains("confidence: medium"));
    assert!(unbacked_verified.contains("artifact_gaps=1"));
    assert!(unbacked_verified.contains("backing=missing"));
    assert!(unbacked_verified.contains("artifacts=0 backing=missing"));
    assert!(failed.contains("confidence: low"));
    assert!(failed.contains("failures=1"));
    assert!(failed.contains("backing=missing"));
    assert!(failed.contains("artifacts=0 backing=missing"));
}

#[test]
fn broker_evidence_confidence_backing_labels_stay_compact() {
    let state = daemon_test_state();
    let backed_verified = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            evidence_artifact_ids: vec!["artifact-test".to_string()],
            recent_tool_invocations: vec![tool_invocation(
                "test-backed",
                1,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests passed"),
                Some("artifact-test"),
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let unbacked_verified = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![tool_invocation(
                "test-unbacked",
                2,
                "cargo test auth_cache",
                suite_packet_core::ToolOperationKind::Test,
                Some("tests passed"),
                None,
            )],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    for body in [backed_verified, unbacked_verified] {
        let lines = body.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.len() <= 180));
        assert!(body.contains("backing="));
    }
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
    let version_path = task_version_json_path(&root, "task-resume-guard", "ctx-1");
    std::fs::create_dir_all(version_path.parent().unwrap()).unwrap();
    std::fs::write(&version_path, serde_json::to_vec_pretty(&context).unwrap()).unwrap();

    {
        let mut guard = state.lock().unwrap();
        let task = ensure_task_record_mut(&mut guard.tasks, "task-resume-guard");
        task.task_id = "task-resume-guard".to_string();
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
