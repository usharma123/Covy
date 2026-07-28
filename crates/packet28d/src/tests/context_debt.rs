use super::support::*;

#[test]
fn broker_context_debt_clears_after_reads_questions_and_verification() {
    let state = daemon_test_state();
    let debt_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_paths_since_checkpoint: vec!["src/stale.rs".to_string()],
        files_edited: vec!["src/stale.rs".to_string()],
        open_questions: vec![suite_packet_core::AgentQuestion {
            id: "q-auth".to_string(),
            text: "Which auth path owns this?".to_string(),
        }],
        active_decisions: vec![suite_packet_core::AgentDecision {
            id: "hypothesis:auth-cache".to_string(),
            text: "hypothesis active: Auth cache owns stale reads".to_string(),
            related_paths: vec!["src/stale.rs".to_string()],
            related_symbols: Vec::new(),
            related_artifact_ids: Vec::new(),
        }],
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "tool-refute".to_string(),
            sequence: 8,
            tool_name: "cargo test".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Generic,
            result_summary: Some("refuted auth-cache after reading src/stale.rs".to_string()),
            ..suite_packet_core::ToolInvocationSummary::default()
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let debt = broker_context_debt_body(&state, debt_snapshot)
        .expect("debt section should render when debts exist");
    assert!(debt.contains(
        "debt_summary: stale_paths=1 open_questions=1 unverified_edits=1 contradictions=1"
    ));
    assert!(debt.contains("payoff stale_path"));
    assert!(serde_json::to_string(&debt).unwrap().len() < 1024);

    let clear_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_paths_since_checkpoint: vec!["src/stale.rs".to_string()],
        files_read: vec!["src/stale.rs".to_string()],
        files_edited: vec!["src/stale.rs".to_string()],
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "tool-test".to_string(),
            sequence: 9,
            tool_name: "cargo test".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Test,
            result_summary: Some("tests passed".to_string()),
            ..suite_packet_core::ToolInvocationSummary::default()
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    assert!(broker_context_debt_body(&state, clear_snapshot).is_none());
}

#[test]
fn broker_context_debt_surfaces_symbol_payoff_without_stale_path() {
    let state = daemon_test_state();
    let debt = broker_context_debt_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    )
    .expect("symbol-only stale evidence should render debt");

    assert!(debt.contains(
        "debt_summary: stale_paths=0 open_questions=0 unverified_edits=1 contradictions=0"
    ));
    assert!(debt.contains(
        "payoff stale_symbol: inspect/search AuthCache before relying on cached evidence"
    ));
    assert!(debt.lines().count() <= 3);
    assert!(debt.lines().all(|line| line.len() <= 140));
    assert!(serde_json::to_string(&debt).unwrap().len() < 512);
}

#[test]
fn broker_context_debt_orders_symbol_payoff_after_path_payoff() {
    let state = daemon_test_state();
    let debt = broker_context_debt_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_paths_since_checkpoint: vec!["src/auth.rs".to_string()],
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            open_questions: vec![suite_packet_core::AgentQuestion {
                id: "q-auth".to_string(),
                text: "Which auth cache path owns this?".to_string(),
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    )
    .expect("mixed stale evidence should render debt");
    let stale_path_index = debt
        .find("payoff stale_path")
        .expect("stale path payoff should render");
    let stale_symbol_index = debt
        .find("payoff stale_symbol")
        .expect("stale symbol payoff should render");
    let open_questions_index = debt
        .find("payoff open_questions")
        .expect("open question payoff should render");
    let unverified_edits_index = debt
        .find("payoff unverified_edits")
        .expect("unverified edit payoff should render");

    assert!(stale_path_index < stale_symbol_index);
    assert!(stale_symbol_index < open_questions_index);
    assert!(open_questions_index < unverified_edits_index);
    assert!(debt.lines().count() <= 5);
    assert!(debt.lines().all(|line| line.len() <= 140));
    assert!(serde_json::to_string(&debt).unwrap().len() < 768);
}

#[test]
fn broker_context_debt_clears_symbol_only_after_verification() {
    let state = daemon_test_state();
    let debt = broker_context_debt_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "tool-test".to_string(),
                sequence: 9,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(debt.is_none());
}

#[test]
fn broker_symbol_verification_clears_debt_but_preserves_confidence_staleness() {
    let state = daemon_test_state();
    let verified_symbol_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
        evidence_artifact_ids: vec!["artifact-test".to_string()],
        recent_tool_invocations: vec![tool_invocation(
            "tool-test",
            9,
            "cargo test auth_cache",
            suite_packet_core::ToolOperationKind::Test,
            Some("tests passed"),
            Some("artifact-test"),
        )],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let debt = broker_context_debt_body(&state, verified_symbol_snapshot.clone());
    let confidence = broker_evidence_confidence_body(&state, verified_symbol_snapshot);

    assert!(debt.is_none());
    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("confidence: high"));
    assert!(confidence.contains("backing=artifact"));
}

#[test]
fn broker_symbol_labels_distinguish_confidence_from_debt_payoff() {
    let state = daemon_test_state();
    let symbol_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };

    let debt = broker_context_debt_body(&state, symbol_snapshot.clone())
        .expect("expected context debt for unverified changed symbol");
    let confidence = broker_evidence_confidence_body(&state, symbol_snapshot);

    assert!(confidence.contains("changed_symbols=1"));
    assert!(!confidence.contains("stale_symbols"));
    assert!(debt.contains("payoff stale_symbol"));
    assert!(!debt.contains("changed_symbols="));
}
