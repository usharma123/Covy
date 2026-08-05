#![cfg(unix)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_setup_cursor_writes_rules_hooks_and_mcp_without_legacy_cursorrules() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "setup",
            "--root",
            root.path().to_str().unwrap(),
            "--runtime",
            "cursor",
            "--yes",
        ])
        .assert()
        .success();

    assert!(root.path().join(".cursor").join("mcp.json").exists());
    assert!(root.path().join(".cursor").join("hooks.json").exists());
    assert!(root
        .path()
        .join(".cursor")
        .join("rules")
        .join("packet28.mdc")
        .exists());
    assert!(!root.path().join(".cursorrules").exists());
    let mcp: Value = serde_json::from_str(
        &fs::read_to_string(root.path().join(".cursor").join("mcp.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(mcp["mcpServers"]["packet28"]["args"][1], ".");
    let rules = fs::read_to_string(
        root.path()
            .join(".cursor")
            .join("rules")
            .join("packet28.mdc"),
    )
    .unwrap();
    assert!(!rules.contains(root.path().to_str().unwrap()));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "doctor",
            "--root",
            root.path().to_str().unwrap(),
            "--agent",
            "cursor",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("cursor_hook_config"));
}

#[test]
fn test_setup_cursor_is_idempotent() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    for _ in 0..2 {
        suite_cmd()
            .current_dir(root.path())
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .args([
                "setup",
                "--root",
                root.path().to_str().unwrap(),
                "--runtime",
                "cursor",
                "--yes",
            ])
            .assert()
            .success();
    }

    let hooks: Value = serde_json::from_str(
        &fs::read_to_string(root.path().join(".cursor").join("hooks.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        hooks["hooks"]["beforeSubmitPrompt"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        hooks["hooks"]["beforeShellExecution"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        hooks["hooks"]["afterShellExecution"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(hooks["hooks"]["stop"].as_array().unwrap().len(), 1);

    let mcp: Value = serde_json::from_str(
        &fs::read_to_string(root.path().join(".cursor").join("mcp.json")).unwrap(),
    )
    .unwrap();
    assert!(mcp["mcpServers"]["packet28"].is_object());
    assert_eq!(mcp["mcpServers"].as_object().unwrap().len(), 1);
}
