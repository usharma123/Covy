use super::support::*;
use super::*;

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
