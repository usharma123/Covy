use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn write_guard_context(path: &Path) {
    fs::write(
        path,
        r#"
version: 1
policy:
  tools:
    allowlist: ["covy"]
  reducers:
    allowlist: ["merge"]
  paths:
    include: ["src/**"]
    exclude: ["src/private/**"]
  token_budget:
    cap: 200
  runtime_budget:
    cap_ms: 1000
  redaction:
    forbidden_patterns: ["(?i)password"]
"#,
    )
    .unwrap();
}

fn write_invalid_guard_context(path: &Path) {
    fs::write(
        path,
        r#"
version: 2
policy:
  tools:
    allowlist: [""]
  reducers:
    allowlist: [""]
  paths:
    include: ["["]
    exclude: []
  token_budget:
    cap: 0
  runtime_budget:
    cap_ms: 0
  redaction:
    forbidden_patterns: ["("]
"#,
    )
    .unwrap();
}

#[test]
fn test_guard_cli_validate_smoke() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    write_guard_context(&context);

    suite_cmd()
        .args(["guard", "validate", "--config", context.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"valid\": true"));
}

#[test]
fn test_guard_cli_validate_with_context_config_flag() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    write_guard_context(&context);

    suite_cmd()
        .args([
            "guard",
            "validate",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"valid\": true"));
}

#[test]
fn test_guard_cli_validate_exit_code_stable_for_invalid_config() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    write_invalid_guard_context(&context);

    suite_cmd()
        .args(["guard", "validate", "--config", context.to_str().unwrap()])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"valid\": false"));
}
