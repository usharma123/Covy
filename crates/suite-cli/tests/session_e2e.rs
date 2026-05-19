use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_session_cli_reports_adoption_from_session_jsonl() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root
        .path()
        .join("claude-projects")
        .join("project")
        .join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_file = sessions_dir.join("session-a.jsonl");
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
                        "command": "Packet28 run cargo check"
                    }
                }
            ]
        }
    });
    fs::write(&session_file, format!("{line}\n")).unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "session",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sessions_scanned\":1"))
        .stdout(predicate::str::contains("\"total_commands\":3"))
        .stdout(predicate::str::contains("\"packet28_commands\":2"))
        .stdout(predicate::str::contains("\"adoption_pct\":66.666"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "session",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28 Session Overview"))
        .stdout(predicate::str::contains("Session"))
        .stdout(predicate::str::contains("Packet28"))
        .stdout(predicate::str::contains("@@@.."))
        .stdout(predicate::str::contains("Average adoption: 67%"))
        .stdout(predicate::str::contains("Packet28 discover --sessions-dir"));
}

#[test]
fn test_session_cli_adoption_all_and_since_scan_multiple_session_files() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root
        .path()
        .join("claude-projects")
        .join("project")
        .join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    for (name, command) in [
        ("session-a.jsonl", "git status --short"),
        ("session-b.jsonl", "Packet28 run cargo check"),
    ] {
        let line = json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": command }
                }]
            }
        });
        fs::write(sessions_dir.join(name), format!("{line}\n")).unwrap();
    }

    suite_cmd()
        .current_dir(root.path())
        .args([
            "session",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--limit",
            "1",
            "--all",
            "--since",
            "7",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sessions_scanned\":2"))
        .stdout(predicate::str::contains("\"total_commands\":2"))
        .stdout(predicate::str::contains("\"packet28_commands\":2"));
}

#[test]
fn test_session_cli_skips_subagent_session_files_like_rtk() {
    let root = TempDir::new().unwrap();
    let project_dir = root.path().join("claude-projects").join("project");
    let subagent_sessions_dir = root
        .path()
        .join("claude-projects")
        .join("project")
        .join("subagents");
    fs::create_dir_all(&project_dir).unwrap();
    fs::create_dir_all(&subagent_sessions_dir).unwrap();

    for (path, command) in [
        (
            project_dir.join("session-top.jsonl"),
            "Packet28 run cargo check",
        ),
        (
            subagent_sessions_dir.join("session-subagent.jsonl"),
            "echo subagent raw",
        ),
    ] {
        let line = json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": command }
                }]
            }
        });
        fs::write(path, format!("{line}\n")).unwrap();
    }

    suite_cmd()
        .current_dir(root.path())
        .args([
            "session",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--all",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sessions_scanned\":1"))
        .stdout(predicate::str::contains("\"total_commands\":1"))
        .stdout(predicate::str::contains("\"packet28_commands\":1"));
}
