#[path = "support/governed_workflow.rs"]
mod governed_workflow;

use governed_workflow::{
    fixture, packet_payload, parse_packet_wrapper, suite_cmd, write_context_packet,
    write_governed_context, write_manifest,
};
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn test_governed_workflow_cli_runs_local_packet_flow() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    let context = dir.path().join("context.yaml");
    let packet_a = dir.path().join("a.json");
    let packet_b = dir.path().join("b.json");

    write_manifest(&manifest);
    write_governed_context(&context);
    write_context_packet(
        &packet_a,
        "diffy",
        "Diff gate",
        "critical regression in coverage",
        "src/lib.rs",
    );
    write_context_packet(
        &packet_b,
        "impact",
        "Impact plan",
        "selected tests for src/lib.rs",
        "src/lib.rs",
    );

    suite_cmd()
        .args(["guard", "validate", "--config", context.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"valid\": true"));

    suite_cmd()
        .args([
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"governed_packet\""))
        .stdout(predicate::str::contains("\"kernel_audit\""));

    suite_cmd()
        .args([
            "test",
            "map",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success();

    suite_cmd()
        .args([
            "test",
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"governed_packet\""))
        .stdout(predicate::str::contains("\"kernel_audit\""));

    let output = suite_cmd()
        .args([
            "context",
            "assemble",
            "--packet",
            packet_a.to_str().unwrap(),
            "--packet",
            packet_b.to_str().unwrap(),
            "--budget-tokens",
            "1200",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.context.assemble.v1");
    assert_eq!(
        value
            .get("packet")
            .and_then(|packet| packet.get("tool"))
            .and_then(Value::as_str),
        Some("contextq")
    );
    assert!(packet_payload(&value).get("assembly").is_some());
}
