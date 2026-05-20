use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_discover_reports_run_missed_savings() {
    let root = TempDir::new().unwrap();
    let missing_sessions = root.path().join("missing-sessions");
    suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "printf",
            "hello",
        ])
        .assert()
        .success();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "discover",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            missing_sessions.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"missed_savings\""))
        .stdout(predicate::str::contains("\"command\":\"printf hello\""));
}

#[test]
fn test_discover_splits_chained_session_commands() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root.path().join("claude-projects").join("project");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_file = sessions_dir.join("session-b.jsonl");
    let line = json!({
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": {
                        "command": "git status --short && echo raw"
                    }
                },
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": {
                        "command": "pytest -q"
                    }
                }
            ]
        }
    });
    fs::write(&session_file, format!("{line}\n")).unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "discover",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"commands_found\":3"))
        .stdout(predicate::str::contains("\"supported_commands\":2"))
        .stdout(predicate::str::contains("\"unsupported_commands\":1"))
        .stdout(predicate::str::contains("\"command\":\"echo\""));
}

#[test]
fn test_discover_uses_tool_result_output_size_for_token_estimates() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root.path().join("claude-projects").join("project");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_file = sessions_dir.join("session-output.jsonl");
    let use_line = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "id": "tool-large-output",
                "name": "Bash",
                "input": { "command": "git status --short" }
            }]
        }
    });
    let result_line = json!({
        "type": "user",
        "message": {
            "content": [{
                "type": "tool_result",
                "tool_use_id": "tool-large-output",
                "content": "x".repeat(400)
            }]
        }
    });
    fs::write(&session_file, format!("{use_line}\n{result_line}\n")).unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "discover",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"commands_found\":1"))
        .stdout(predicate::str::contains("\"supported_commands\":1"))
        .stdout(predicate::str::contains("\"estimated_tokens\":100"));
}

#[test]
fn test_discover_reports_rtk_style_missed_packet28_opportunities() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root.path().join("claude-projects").join("project");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_file = sessions_dir.join("session-opportunities.jsonl");
    let use_line = json!({
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "tool_use",
                    "id": "tool-git",
                    "name": "Bash",
                    "input": { "command": "git status --short" }
                },
                {
                    "type": "tool_use",
                    "id": "tool-p28",
                    "name": "Bash",
                    "input": { "command": "Packet28 run cargo check" }
                }
            ]
        }
    });
    let result_line = json!({
        "type": "user",
        "message": {
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "tool-git",
                    "content": "x".repeat(400)
                },
                {
                    "type": "tool_result",
                    "tool_use_id": "tool-p28",
                    "content": "y".repeat(400)
                }
            ]
        }
    });
    fs::write(&session_file, format!("{use_line}\n{result_line}\n")).unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "discover",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"missed_opportunities\""))
        .stdout(predicate::str::contains("\"command\":\"git status\""))
        .stdout(predicate::str::contains(
            "\"packet28_equivalent\":\"Packet28 run\"",
        ))
        .stdout(predicate::str::contains("\"raw_est_tokens\":100"))
        .stdout(predicate::str::contains("\"estimated_savings_tokens\":70"))
        .stdout(predicate::str::contains("Packet28 run cargo").not());

    suite_cmd()
        .current_dir(root.path())
        .args([
            "discover",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Missed Packet28 opportunities"))
        .stdout(predicate::str::contains("git status: 1x -> Packet28 run"));
}

#[test]
fn test_discover_reports_disabled_bypasses_like_rtk() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root.path().join("claude-projects").join("project");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_file = sessions_dir.join("session-disabled.jsonl");
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
                    "input": { "command": "env RTK_DISABLED=true cargo test" }
                },
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": "PACKET28_DISABLED=0 pytest -q" }
                }
            ]
        }
    });
    fs::write(&session_file, format!("{line}\n")).unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "discover",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"commands_found\":3"))
        .stdout(predicate::str::contains("\"supported_commands\":1"))
        .stdout(predicate::str::contains("\"unsupported_commands\":0"))
        .stdout(predicate::str::contains("\"disabled_bypass_count\":2"))
        .stdout(predicate::str::contains("git status (1x)"))
        .stdout(predicate::str::contains("cargo test (1x)"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "discover",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Disabled bypasses: 2 commands"))
        .stdout(predicate::str::contains(
            "Remove PACKET28_DISABLED/RTK_DISABLED",
        ));
}
