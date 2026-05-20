#[path = "support/stack_build.rs"]
mod stack_build;

use serde_json::Value;
use stack_build::{
    packet_payload, parse_packet_wrapper, suite_cmd, write_build_log, write_stack_log,
};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn write_governed_context(path: &Path) {
    fs::write(
        path,
        r#"
version: 1
policy:
  tools:
    allowlist: ["diffy", "testy", "stacky", "buildy", "contextq"]
  reducers:
    allowlist: ["analyze", "impact", "slice", "reduce", "assemble", "contextq.assemble", "diffy.analyze", "testy.impact", "stacky.slice", "buildy.reduce", "governed.assemble"]
  paths:
    include: ["**"]
    exclude: []
  token_budget:
    cap: 5000
  runtime_budget:
    cap_ms: 5000
  tool_call_budget:
    cap: 10
  redaction:
    forbidden_patterns: []
  human_review:
    required: false
    on_policy_violation: true
    on_budget_violation: true
    on_redaction_violation: true
    paths: []
"#,
    )
    .unwrap();
}

fn packet_debug(wrapper: &Value) -> Option<&Value> {
    packet_payload(wrapper).get("debug")
}

#[test]
fn test_stack_build_cli_stack_slice_governed_smoke() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("stack.log");
    let context = dir.path().join("context.yaml");
    write_stack_log(&input);
    write_governed_context(&context);

    let output = suite_cmd()
        .args([
            "stack",
            "slice",
            "--input",
            input.to_str().unwrap(),
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.stack.slice.v1");
    assert!(packet_debug(&value)
        .and_then(|debug| debug.get("kernel_audit"))
        .and_then(|v| v.get("stack"))
        .is_some());
    assert!(packet_debug(&value)
        .and_then(|debug| debug.get("kernel_audit"))
        .and_then(|v| v.get("governed"))
        .is_some());
}

#[test]
fn test_stack_build_cli_build_reduce_governed_smoke() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("build.log");
    let context = dir.path().join("context.yaml");
    write_build_log(&input);
    write_governed_context(&context);

    let output = suite_cmd()
        .args([
            "build",
            "reduce",
            "--input",
            input.to_str().unwrap(),
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.build.reduce.v1");
    assert!(packet_debug(&value)
        .and_then(|debug| debug.get("kernel_audit"))
        .and_then(|v| v.get("build"))
        .is_some());
    assert!(packet_debug(&value)
        .and_then(|debug| debug.get("kernel_audit"))
        .and_then(|v| v.get("governed"))
        .is_some());
}
