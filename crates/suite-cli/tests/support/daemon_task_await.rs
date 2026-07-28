use crate::daemon_task_core::{git, suite_cmd, write_repo_fixture};
use crate::daemon_task_seed::seed_checkpointed_handoff_task;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

pub fn repo_with_checkpointed_handoff(task_id: &str, intention_text: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init"]);
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(dir.path(), task_id, intention_text);
    dir
}

pub fn await_handoff(root: &Path, task_id: &str, timeout_ms: &str, poll_ms: &str) -> Value {
    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            root.to_str().unwrap(),
            "--task-id",
            task_id,
            "--timeout-ms",
            timeout_ms,
            "--poll-ms",
            poll_ms,
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&output).unwrap()
}

pub fn await_newer_handoff(root: &Path, task_id: &str, previous_context_version: &str) -> Value {
    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            root.to_str().unwrap(),
            "--task-id",
            task_id,
            "--after-context-version",
            previous_context_version,
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
    serde_json::from_slice(&output).unwrap()
}

pub fn launch_agent_for_bootstrap_mode(root: &Path, task_id: &str) -> Value {
    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "launch-agent",
            "--root",
            root.to_str().unwrap(),
            "--task-id",
            task_id,
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
    serde_json::from_slice(&output).unwrap()
}

pub fn task_status(root: &Path, task_id: &str) -> Value {
    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            root.to_str().unwrap(),
            "--task-id",
            task_id,
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&output).unwrap()
}

pub fn stop_daemon(root: &Path) {
    suite_cmd()
        .args(["daemon", "stop", "--root", root.to_str().unwrap()])
        .assert()
        .success();
}
