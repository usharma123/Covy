use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_gain_warns_about_disabled_bypass_sessions_like_rtk() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root.path().join("claude-projects").join("project");
    fs::create_dir_all(&sessions_dir).unwrap();
    let line = json!({
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": "PACKET28_DISABLED=1 git status --short" }
                },
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": "RTK_DISABLED=true cargo test" }
                },
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": "git status --short" }
                }
            ]
        }
    });
    fs::write(
        sessions_dir.join("session-gain-disabled.jsonl"),
        format!("{line}\n"),
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "used PACKET28_DISABLED/RTK_DISABLED unnecessarily",
        ))
        .stderr(predicate::str::contains("Packet28 discover"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
}

#[test]
fn test_gain_cc_economics_merges_ccusage_and_packet28_savings() {
    let root = TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(root.path())
        .status()
        .unwrap();
    fs::write(root.path().join("tracked.txt"), "changed\n").unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "git",
            "status",
            "--short",
        ])
        .assert()
        .success();

    let ccusage = root.path().join("ccusage.json");
    fs::write(
        &ccusage,
        r#"{
  "monthly": [{
    "month": "2026-05",
    "inputTokens": 1000,
    "outputTokens": 200,
    "cacheCreationTokens": 80,
    "cacheReadTokens": 500,
    "totalTokens": 1780,
    "totalCost": 3.5
  }]
}"#,
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "cc-economics",
            "--root",
            root.path().to_str().unwrap(),
            "--ccusage-json",
            ccusage.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"source\""))
        .stdout(predicate::str::contains("\"cc_total_tokens\":1780"))
        .stdout(predicate::str::contains("\"packet28_commands\":1"))
        .stdout(predicate::str::contains("\"packet28_saved_tokens\""))
        .stdout(predicate::str::contains(
            "\"weighted_input_cost_per_token\"",
        ));
}
