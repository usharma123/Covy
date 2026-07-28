#[path = "support/diff_analyze.rs"]
mod diff_analyze;

use diff_analyze::{fixture, suite_cmd};
use predicates::prelude::*;
use serde_json::Value;

#[test]
fn test_diff_analyze_cli_smoke() {
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
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"passed\""));
}

#[test]
fn test_diff_analyze_cli_json_includes_cache_block() {
    let output = suite_cmd()
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
            "--cache",
            "--json",
            "full",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert!(value
        .get("packet")
        .and_then(|packet| packet.get("payload"))
        .and_then(|payload| payload.get("debug"))
        .and_then(|debug| debug.get("cache"))
        .and_then(|v| v.get("diff"))
        .and_then(|v| v.get("hit"))
        .and_then(Value::as_bool)
        .is_some());
}
