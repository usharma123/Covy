#[path = "support/daemon_task_submit.rs"]
mod daemon_task_submit;
#[expect(
    dead_code,
    reason = "this integration binary exercises a focused subset of the shared harness"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use daemon_task_submit::{
    ensure_packet28d_built, setup_changed_repo, suite_cmd, task_spec_with_file_watch,
    write_task_spec,
};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn invalid_task_spec(root: &Path) -> PathBuf {
    let spec_path = root.join("bad-task-spec.json");
    write_task_spec(
        &spec_path,
        task_spec_with_file_watch(
            root,
            "task-invalid",
            vec![json!({
                "id": "",
                "target": "nope.reducer",
                "depends_on": [],
                "input_packets": [],
                "policy_context": {},
                "reducer_input": {},
                "budget": {}
            })],
            "file",
        ),
    );
    spec_path
}

fn start_daemon(root: &Path) {
    suite_cmd()
        .args(["daemon", "start", "--root", root.to_str().unwrap()])
        .assert()
        .success();
}

fn submit_invalid_task(root: &Path, spec_path: &Path) {
    suite_cmd()
        .args([
            "daemon",
            "task",
            "submit",
            "--root",
            root.to_str().unwrap(),
            "--spec",
            spec_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .code(2);
}

fn assert_task_and_watches_absent(root: &Path) {
    let task_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            root.to_str().unwrap(),
            "--task-id",
            "task-invalid",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let task_status: Value = serde_json::from_slice(&task_output).unwrap();
    assert!(task_status.is_null());

    let watches_output = suite_cmd()
        .args([
            "daemon",
            "watch",
            "list",
            "--root",
            root.to_str().unwrap(),
            "--task-id",
            "task-invalid",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let watches: Value = serde_json::from_slice(&watches_output).unwrap();
    assert!(watches.as_array().unwrap().is_empty());
}

fn stop_daemon(root: &Path) {
    let pid = fs::read_to_string(root.join(".packet28/daemon/pid"))
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    suite_cmd()
        .args(["daemon", "stop", "--root", root.to_str().unwrap()])
        .assert()
        .success();
    let started = Instant::now();
    while daemon_process_exists(pid) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "packet28d did not finish shutting down"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn daemon_process_exists(pid: i32) -> bool {
    // SAFETY: signal 0 only probes the daemon PID read from this fixture's runtime file.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[test]
#[cfg(unix)]
fn test_daemon_task_submit_failed_submit_cleans_up_task_and_watches() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let spec_path = invalid_task_spec(dir.path());

    start_daemon(dir.path());
    submit_invalid_task(dir.path(), &spec_path);
    assert_task_and_watches_absent(dir.path());
    stop_daemon(dir.path());
}

#[test]
#[cfg(unix)]
fn test_daemon_task_submit_failed_submit_does_not_resurrect_after_restart() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let spec_path = invalid_task_spec(dir.path());

    start_daemon(dir.path());
    submit_invalid_task(dir.path(), &spec_path);
    stop_daemon(dir.path());

    start_daemon(dir.path());
    assert_task_and_watches_absent(dir.path());
    stop_daemon(dir.path());
}
