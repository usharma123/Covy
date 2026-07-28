use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_discover_sessions_all_and_since_scan_multiple_session_files() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root.path().join("claude-projects").join("project");
    fs::create_dir_all(&sessions_dir).unwrap();
    for (name, command) in [
        ("session-a.jsonl", "git status --short"),
        ("session-b.jsonl", "pytest -q"),
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
            "discover",
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
        .stdout(predicate::str::contains("\"commands_found\":2"))
        .stdout(predicate::str::contains("\"supported_commands\":2"));
}

#[test]
fn test_discover_sessions_project_filter_limits_session_scan_like_rtk() {
    let root = TempDir::new().unwrap();
    let projects_dir = root.path().join("claude-projects");
    for (project, session, command) in [
        ("matching-project", "session-a.jsonl", "git status --short"),
        ("other-project", "session-b.jsonl", "pytest -q"),
    ] {
        let sessions_dir = projects_dir.join(project).join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
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
        fs::write(sessions_dir.join(session), format!("{line}\n")).unwrap();
    }

    suite_cmd()
        .current_dir(root.path())
        .args([
            "discover",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            projects_dir.to_str().unwrap(),
            "--project",
            "matching",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sessions_scanned\":1"))
        .stdout(predicate::str::contains("\"commands_found\":1"))
        .stdout(predicate::str::contains("\"supported_commands\":1"));
}

#[test]
fn test_discover_sessions_recurses_project_session_dirs_like_rtk() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root
        .path()
        .join("claude-projects")
        .join("project")
        .join("nested");
    fs::create_dir_all(&sessions_dir).unwrap();
    let line = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "name": "Bash",
                "input": { "command": "git status --short" }
            }]
        }
    });
    fs::write(
        sessions_dir.join("session-nested.jsonl"),
        format!("{line}\n"),
    )
    .unwrap();

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
        .stdout(predicate::str::contains("\"sessions_scanned\":1"))
        .stdout(predicate::str::contains("\"commands_found\":1"))
        .stdout(predicate::str::contains("\"supported_commands\":1"));
}
