use assert_cmd::Command;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn ensure_packet28d_built() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let status = std::process::Command::new("cargo")
            .args(["build", "-p", "packet28d"])
            .status()
            .unwrap();
        assert!(status.success(), "failed to build packet28d");
    });
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed with {status}", args);
}

fn init_repo(root: &Path) {
    git(root, &["init"]);
}

fn write_context_packet(path: &Path, packet_id: &str, title: &str, body: &str, path_ref: &str) {
    fs::write(
        path,
        format!(
            r#"{{
  "packet_id": "{packet_id}",
  "tool": "{packet_id}",
  "reducer": "reduce",
  "paths": ["{path_ref}"],
  "sections": [
    {{
      "title": "{title}",
      "body": "{body}",
      "refs": [{{ "kind": "file", "value": "{path_ref}" }}],
      "relevance": 0.9
    }}
  ]
}}"#
        ),
    )
    .unwrap();
}

fn write_packet_value(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

#[test]
fn test_context_recall_daemon_cli_prefers_summary_snippet_over_target_name() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let packet_a = dir.path().join("a.json");
    let packet_b = dir.path().join("b.json");
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
            "--json",
        ])
        .assert()
        .success();

    let output = suite_cmd()
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
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    let snippet = value
        .get("hits")
        .and_then(Value::as_array)
        .and_then(|hits| hits.first())
        .and_then(|hit| hit.get("snippet"))
        .and_then(Value::as_str)
        .unwrap();
    assert!(snippet.contains("critical regression"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn test_context_recall_daemon_cli_json_surfaces_summary_field() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let packet = dir.path().join("stack.json");

    write_packet_value(
        &packet,
        &json!({
            "version": "1",
            "tool": "stacky",
            "kind": "stack_slice",
            "hash": "stack-hash",
            "summary": "stack failures total=2 unique=2",
            "files": [{"path": "src/main.rs", "relevance": 1.0}],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["stack.log"], "generated_at_unix": 1},
            "payload": {
                "total_failures": 2,
                "unique_failures": 2,
                "duplicates_removed": 0
            }
        }),
    );

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "assemble",
            "--packet",
            packet.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "recall",
            "--root",
            ".",
            "--query",
            "stack failures src/main.rs",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    let first_hit = value
        .get("hits")
        .and_then(Value::as_array)
        .and_then(|hits| hits.first())
        .unwrap();
    assert_eq!(
        first_hit.get("summary").and_then(Value::as_str),
        Some("stack failures total=2 unique=2")
    );
    assert_eq!(
        first_hit.get("snippet").and_then(Value::as_str),
        Some("stack failures total=2 unique=2")
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
