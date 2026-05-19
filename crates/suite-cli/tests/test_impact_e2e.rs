use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn fixture(rel: &str) -> String {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    workspace
        .join("tests")
        .join("fixtures")
        .join(rel)
        .to_string_lossy()
        .to_string()
}

fn write_manifest(path: &Path) {
    let line = format!(
        "{{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    fs::write(path, line).unwrap();
}

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

fn packet_debug(wrapper: &Value) -> Option<&Value> {
    packet_payload(wrapper).get("debug")
}

fn write_testmap(manifest: &Path, testmap: &Path) {
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
}

#[test]
fn test_test_impact_cli_smoke() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    write_manifest(&manifest);
    write_testmap(&manifest, &testmap);

    let output = suite_cmd()
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
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.test.impact.v1");
    assert!(packet_payload(&value)
        .get("result")
        .and_then(|v| v.get("selected_tests"))
        .is_some());
}

#[test]
fn test_test_impact_cli_governed_smoke() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    let context = dir.path().join("context.yaml");
    write_manifest(&manifest);
    write_governed_context(&context);
    write_testmap(&manifest, &testmap);

    let output = suite_cmd()
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
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.test.impact.v1");
    assert!(packet_debug(&value)
        .and_then(|v| v.get("governed_packet"))
        .and_then(|v| v.get("tool"))
        .and_then(Value::as_str)
        .is_some());
}

#[test]
fn test_test_impact_cli_governed_json_metadata_shape() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    let context = dir.path().join("context.yaml");
    write_manifest(&manifest);
    write_governed_context(&context);
    write_testmap(&manifest, &testmap);

    let output = suite_cmd()
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
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.test.impact.v1");
    assert!(packet_debug(&value)
        .and_then(|v| v.get("kernel_metadata"))
        .and_then(|meta| meta.get("impact"))
        .is_some());
    assert!(packet_debug(&value)
        .and_then(|v| v.get("kernel_metadata"))
        .and_then(|meta| meta.get("governed"))
        .and_then(|governed| governed.get("budget_trim"))
        .is_some());
}
