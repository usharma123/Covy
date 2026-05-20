#[path = "support/discover.rs"]
mod discover;

use discover::suite_cmd;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

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
