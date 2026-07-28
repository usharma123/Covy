#[path = "support/daemon_task_submit.rs"]
mod daemon_task_submit;

use daemon_task_submit::{
    ensure_packet28d_built, setup_changed_repo, suite_cmd, task_spec_with_file_watch,
    write_task_spec,
};
use serde_json::{json, Value};
use tempfile::TempDir;

#[test]
#[cfg(unix)]
fn test_daemon_task_submit_failed_submit_cleans_up_task_and_watches() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let spec_path = dir.path().join("bad-task-spec.json");
    write_task_spec(
        &spec_path,
        task_spec_with_file_watch(
            dir.path(),
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

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    suite_cmd()
        .args([
            "daemon",
            "task",
            "submit",
            "--root",
            dir.path().to_str().unwrap(),
            "--spec",
            spec_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .code(2);

    let task_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
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
            dir.path().to_str().unwrap(),
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

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
