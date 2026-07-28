use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn write_permissive_context(path: &Path) {
    fs::write(
        path,
        r#"
version: 1
policy:
  paths:
    include: ["**"]
    exclude: []
  redaction:
    forbidden_patterns: []
"#,
    )
    .unwrap();
}

fn write_repo_fixture(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("alpha.rs"),
        r#"
use crate::beta::Beta;

fn alpha() {}
struct Alpha;
"#,
    )
    .unwrap();
    fs::write(
        src.join("beta.rs"),
        r#"
fn beta() {}
enum Beta {
  A,
}
"#,
    )
    .unwrap();
}

#[test]
fn test_map_proxy_governed_map_repo_rich_section_body_uses_rich_payload() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    let context = dir.path().join("context.yaml");
    write_permissive_context(&context);

    let output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json",
            "full",
            "--packet-detail",
            "rich",
            "--context-config",
            context.to_str().unwrap(),
            "--context-budget-tokens",
            "5000",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    let body = value
        .get("packet")
        .and_then(|v| v.get("payload"))
        .and_then(|v| v.get("debug"))
        .and_then(|v| v.get("governed_packet"))
        .and_then(|v| v.get("payload"))
        .and_then(|v| v.get("sections"))
        .and_then(Value::as_array)
        .and_then(|sections| sections.first())
        .and_then(|section| section.get("body"))
        .and_then(Value::as_str)
        .unwrap();
    assert!(body.contains("\"path\""));
    assert!(!body.contains("file_idx"));
}

#[test]
fn test_map_proxy_governed_proxy_run_rich_section_body_uses_rich_payload() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    write_permissive_context(&context);

    let output = suite_cmd()
        .args([
            "proxy",
            "run",
            "--json",
            "full",
            "--packet-detail",
            "rich",
            "--context-config",
            context.to_str().unwrap(),
            "--context-budget-tokens",
            "5000",
            "--",
            "ls",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    let body = value
        .get("packet")
        .and_then(|v| v.get("payload"))
        .and_then(|v| v.get("debug"))
        .and_then(|v| v.get("governed_packet"))
        .and_then(|v| v.get("payload"))
        .and_then(|v| v.get("sections"))
        .and_then(Value::as_array)
        .and_then(|sections| sections.first())
        .and_then(|section| section.get("body"))
        .and_then(Value::as_str)
        .unwrap();
    assert!(body.contains("\"output_lines\""));
}
