#[path = "support/context_daemon_core.rs"]
mod context_daemon_core;
#[path = "support/context_packet.rs"]
mod context_packet;
#[expect(
    dead_code,
    reason = "this integration binary exercises a focused subset of the shared harness"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use context_daemon_core::{ensure_packet28d_built, init_repo, suite_cmd};
use context_packet::{packet_payload, parse_packet_wrapper, write_state_event};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn write_packet_value(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

#[test]
#[cfg(unix)]
fn test_context_via_daemon_cli_non_assemble_smoke() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let diff = dir.path().join("diff.json");
    let impact = dir.path().join("impact.json");
    let event = dir.path().join("event.json");

    write_packet_value(
        &diff,
        &json!({
            "version": "1",
            "tool": "diffy",
            "kind": "diff_analyze",
            "hash": "diff-hash",
            "summary": "changed StopWatch",
            "files": [{"path": "src/StopWatch.java", "relevance": 1.0}],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["diff"], "generated_at_unix": 1},
            "payload": {
                "gate_result": {"passed": true, "violations": []},
                "diffs": [{"path": "src/StopWatch.java", "old_path": null, "status": "Modified", "changed_lines": [10, 11]}]
            }
        }),
    );
    write_packet_value(
        &impact,
        &json!({
            "version": "1",
            "tool": "testy",
            "kind": "test_impact",
            "hash": "impact-hash",
            "summary": "impact",
            "files": [],
            "symbols": [{"name": "StopWatchTest#testSplit", "kind": "test_id", "relevance": 1.0}],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["testmap.bin"], "generated_at_unix": 1},
            "payload": {
                "result": {
                    "selected_tests": ["StopWatchTest#testSplit"],
                    "smoke_tests": [],
                    "missing_mappings": [],
                    "confidence": 0.9,
                    "stale": false,
                    "escalate_full_suite": false
                },
                "known_tests": 1,
                "print_command": null
            }
        }),
    );
    write_state_event(
        &event,
        r#"{
  "event_id": "evt-1",
  "occurred_at_unix": 1,
  "actor": "tester",
  "kind": "focus_set",
  "paths": ["src/lib.rs"],
  "symbols": [],
  "data": {"type": "focus_set"}
}"#,
    );

    let correlate_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "correlate",
            "--packet",
            diff.to_str().unwrap(),
            "--packet",
            impact.to_str().unwrap(),
            "--task-id",
            "task-correlation",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let correlate_value = parse_packet_wrapper(&correlate_output, "suite.context.correlate.v1");
    assert!(packet_payload(&correlate_value)
        .get("findings")
        .and_then(Value::as_array)
        .is_some());

    let state_append_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "state",
            "append",
            "--task-id",
            "task-state",
            "--input",
            event.to_str().unwrap(),
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let state_append_value = parse_packet_wrapper(&state_append_output, "suite.agent.state.v1");
    assert_eq!(
        packet_payload(&state_append_value)
            .get("task_id")
            .and_then(Value::as_str),
        Some("task-state")
    );

    let state_snapshot_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "state",
            "snapshot",
            "--task-id",
            "task-state",
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let state_snapshot_value =
        parse_packet_wrapper(&state_snapshot_output, "suite.agent.snapshot.v1");
    assert_eq!(
        packet_payload(&state_snapshot_value)
            .get("task_id")
            .and_then(Value::as_str),
        Some("task-state")
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
