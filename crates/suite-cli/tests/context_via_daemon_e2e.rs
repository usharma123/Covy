#[path = "support/context_daemon.rs"]
mod context_daemon;

use context_daemon::{
    ensure_packet28d_built, init_repo, suite_cmd, write_context_packet, write_packet_value,
};
use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn write_state_event(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
}

fn parse_packet_wrapper(output: &[u8], packet_type: &str) -> Value {
    let value: Value = serde_json::from_slice(output).unwrap();
    assert_eq!(
        value.get("schema_version").and_then(Value::as_str),
        Some("suite.packet.v1")
    );
    assert_eq!(
        value.get("packet_type").and_then(Value::as_str),
        Some(packet_type)
    );
    assert!(value.get("packet").is_some());
    value
}

fn packet_payload(wrapper: &Value) -> &Value {
    wrapper
        .get("packet")
        .and_then(|packet| packet.get("payload"))
        .expect("packet.payload should exist")
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
    let packet_a = dir.path().join("a.json");
    let packet_b = dir.path().join("b.json");

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
    write_context_packet(
        &packet_a,
        "diffy",
        "Diff gate",
        "critical regression in coverage",
        "src/lib.rs",
    );
    write_context_packet(
        &packet_b,
        "testy",
        "Impact plan",
        "selected tests for src/lib.rs",
        "src/lib.rs",
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
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "assemble",
            "--packet",
            packet_a.to_str().unwrap(),
            "--packet",
            packet_b.to_str().unwrap(),
        ])
        .assert()
        .success();

    let store_list_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "list",
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let store_list_value: Value = serde_json::from_slice(&store_list_output).unwrap();
    let entries = store_list_value
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap();
    assert!(!entries.is_empty());

    let key = entries[0]
        .get("cache_key")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "get",
            "--root",
            ".",
            "--key",
            &key,
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&key));

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "stats",
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"stats\""));

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "recall",
            "--root",
            ".",
            "--query",
            "critical regression",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"query\":\"critical regression\"",
        ));

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "prune",
            "--root",
            ".",
            "--all",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"report\""));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
