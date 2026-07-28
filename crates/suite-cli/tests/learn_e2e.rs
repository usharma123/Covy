use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_learn_cli_detects_correction_from_session_history() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root.path().join("claude-projects").join("project");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_file = sessions_dir.join("session-learn.jsonl");
    let bad_use = json!({
        "type": "assistant",
        "message": {"content": [{
            "type": "tool_use",
            "name": "Bash",
            "input": {"command": "git status --porcelain=v9"}
        }]}
    });
    let bad_result = json!({
        "type": "user",
        "message": {"content": [{
            "type": "tool_result",
            "is_error": true,
            "content": "error: unknown option `porcelain=v9`"
        }]}
    });
    let good_use = json!({
        "type": "assistant",
        "message": {"content": [{
            "type": "tool_use",
            "name": "Bash",
            "input": {"command": "git status --short"}
        }]}
    });
    let unrelated_use = json!({
        "type": "assistant",
        "message": {"content": [{
            "type": "tool_use",
            "name": "Bash",
            "input": {"command": "pwd"}
        }]}
    });
    let unrelated_result = json!({
        "type": "user",
        "message": {"content": [{
            "type": "tool_result",
            "is_error": false,
            "content": root.path().display().to_string()
        }]}
    });
    let good_result = json!({
        "type": "user",
        "message": {"content": [{
            "type": "tool_result",
            "is_error": false,
            "content": " M src/main.rs"
        }]}
    });
    fs::write(
        &session_file,
        format!(
            "{bad_use}\n{bad_result}\n{unrelated_use}\n{unrelated_result}\n{good_use}\n{good_result}\n"
        ),
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "learn",
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--min-frequency",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sessions_scanned\":1"))
        .stdout(predicate::str::contains("\"corrections_found\":1"))
        .stdout(predicate::str::contains("git status --porcelain=v9"))
        .stdout(predicate::str::contains("git status --short"))
        .stdout(predicate::str::contains("\"error_type\":\"unknown_flag\""));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "learn",
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--min-occurrences",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"corrections_found\":1"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "learn",
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--min-frequency",
            "1",
            "--write-rules",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Corrections found: 1"))
        .stdout(predicate::str::contains("unknown_flag"))
        .stdout(predicate::str::contains("Corrections written"));
    assert!(root
        .path()
        .join(".claude")
        .join("rules")
        .join("cli-corrections.md")
        .exists());
}
