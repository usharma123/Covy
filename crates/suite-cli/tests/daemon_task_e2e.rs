#[path = "support/daemon_task.rs"]
mod daemon_task;

use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
use std::time::Duration;
use tempfile::TempDir;

use daemon_task::{
    ensure_packet28d_built, git, initialize_mcp_session, run_claude_hook,
    seed_checkpointed_handoff_task, start_mcp_server, suite_cmd, write_intention_via_mcp,
    write_repo_fixture,
};
#[test]
#[cfg(unix)]
fn test_daemon_task_cli_await_handoff_reports_ready_status() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init"]);
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-await",
        "Prepare daemon-owned handoff wait",
    );

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-await",
            "--timeout-ms",
            "1000",
            "--poll-ms",
            "50",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["task_status"]["handoff_ready"], true);
    assert!(value["waited_ms"].as_u64().unwrap() <= 1_000);
    assert!(value["polls"].as_u64().unwrap() >= 1);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_daemon_task_cli_launch_agent_spawns_child_from_handoff() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init"]);
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-launch",
        "Launch fresh worker from daemon",
    );

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "launch-agent",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-launch",
            "--json",
            "--",
            "sh",
            "-c",
            "printf '%s\\n%s\\n' \"$PACKET28_BOOTSTRAP_MODE\" \"$PACKET28_TASK_ID\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launch_value: Value = serde_json::from_slice(&output).unwrap();
    let log_path = launch_value["log_path"].as_str().unwrap();
    assert_eq!(launch_value["bootstrap_mode"], "handoff");
    assert!(launch_value["pid"].as_u64().unwrap() > 0);

    let mut log_contents = String::new();
    for _ in 0..40 {
        if let Ok(raw) = fs::read_to_string(log_path) {
            log_contents = raw;
            if log_contents.contains("handoff") && log_contents.contains("task-daemon-launch") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(log_contents.contains("handoff"));
    assert!(log_contents.contains("task-daemon-launch"));

    let status_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-launch",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_value: Value = serde_json::from_slice(&status_output).unwrap();
    assert_eq!(status_value["latest_agent_bootstrap_mode"], "handoff");
    assert_eq!(
        status_value["latest_agent_pid"].as_u64().unwrap(),
        launch_value["pid"].as_u64().unwrap()
    );
    assert_eq!(status_value["latest_agent_log_path"], log_path);
    assert_eq!(
        status_value["latest_agent_handoff_artifact_id"],
        launch_value["handoff_artifact_id"]
    );
    assert_eq!(
        status_value["latest_agent_handoff_checkpoint_id"],
        launch_value["handoff_checkpoint_id"]
    );
    assert!(status_value["latest_agent_context_version"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_daemon_task_cli_await_handoff_can_require_newer_context_version() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init"]);
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-newer-handoff",
        "Prepare initial handoff",
    );
    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    let launch_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "launch-agent",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--json",
            "--",
            "sh",
            "-c",
            "printf '%s\\n' \"$PACKET28_BOOTSTRAP_MODE\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launch_value: Value = serde_json::from_slice(&launch_output).unwrap();
    let launched_context_version = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launched_status: Value = serde_json::from_slice(&launched_context_version).unwrap();
    let previous_context_version = launched_status["latest_agent_context_version"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(launch_value["bootstrap_mode"], "handoff");

    suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--after-context-version",
            &previous_context_version,
            "--timeout-ms",
            "100",
            "--poll-ms",
            "20",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "newer handoff than context version",
        ));

    let _ = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        4,
        "task-daemon-newer-handoff",
        "Resume from a newer handoff",
        "editing",
        &["src/beta.rs"],
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreCompact",
            "task_id":"task-daemon-newer-handoff",
            "session_id":"session-daemon-newer-handoff",
        }),
    );
    assert_eq!(status, 0);

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--after-context-version",
            &previous_context_version,
            "--timeout-ms",
            "1000",
            "--poll-ms",
            "50",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["task_status"]["handoff_ready"], true);
    assert_ne!(
        value["task_status"]["latest_context_version"]
            .as_str()
            .unwrap(),
        previous_context_version
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
