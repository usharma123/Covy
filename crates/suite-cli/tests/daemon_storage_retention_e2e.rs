use std::fs;

use assert_cmd::Command;
use packet28_daemon_core::task_store_lease::acquire_daemon_task_store_lease;
use serde_json::Value;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn write_task_artifact(root: &TempDir, task_id: &str, contents: &[u8]) {
    let directory = root.path().join(".packet28/task").join(task_id);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("payload.bin"), contents).unwrap();
}

#[test]
fn storage_inspect_reports_current_timestamped_metrics_as_json() {
    let root = TempDir::new().unwrap();
    write_task_artifact(&root, "task", b"payload");

    let output = suite_cmd()
        .args([
            "daemon",
            "storage",
            "inspect",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["mode"], "inspect");
    assert!(report["observed_at_unix"].as_u64().unwrap() > 0);
    assert_eq!(
        report["metrics_before"]["allocated_bytes_supported"],
        cfg!(unix)
    );
    assert!(report["metrics_before"]["state_allocated_bytes"]
        .as_u64()
        .is_some());
    assert_eq!(report["metrics_before"]["task_artifact_logical_bytes"], 7);
    assert_eq!(report["metrics_before"]["task_artifact_files"], 1);
    assert_eq!(report["metrics_before"]["managed_task_logical_bytes"], 7);
}

#[test]
fn storage_cleanup_is_a_non_mutating_dry_run_without_apply() {
    let root = TempDir::new().unwrap();
    write_task_artifact(&root, "stale", b"payload");
    let artifact = root.path().join(".packet28/task/stale/payload.bin");

    let output = suite_cmd()
        .args([
            "daemon",
            "storage",
            "cleanup",
            "--root",
            root.path().to_str().unwrap(),
            "--max-bytes",
            "0",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(report["mode"], "dry_run");
    assert_eq!(report["retention"]["planned_tasks"], 1);
    assert_eq!(report["retention"]["removed_tasks"], 0);
    assert_eq!(report["actions"][0]["outcome"], "would_remove");
    assert!(artifact.exists());
}

#[cfg(unix)]
#[test]
fn storage_cleanup_only_removes_data_after_explicit_apply() {
    let root = TempDir::new().unwrap();
    write_task_artifact(&root, "stale", b"payload");
    let task_directory = root.path().join(".packet28/task/stale");

    let output = suite_cmd()
        .args([
            "daemon",
            "storage",
            "cleanup",
            "--root",
            root.path().to_str().unwrap(),
            "--max-bytes",
            "0",
            "--apply",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(report["mode"], "apply");
    assert_eq!(report["retention"]["removed_tasks"], 1);
    assert_eq!(report["retention"]["removed_logical_bytes"], 7);
    assert_eq!(report["metrics_after"]["managed_task_logical_bytes"], 0);
    assert!(!task_directory.exists());
}

#[cfg(unix)]
#[test]
fn storage_cleanup_refuses_apply_while_daemon_owns_task_store() {
    let root = TempDir::new().unwrap();
    write_task_artifact(&root, "stale", b"payload");
    let artifact = root.path().join(".packet28/task/stale/payload.bin");
    let daemon_lease = acquire_daemon_task_store_lease(root.path()).unwrap();

    suite_cmd()
        .args([
            "daemon",
            "storage",
            "cleanup",
            "--root",
            root.path().to_str().unwrap(),
            "--max-bytes",
            "0",
            "--apply",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "task retention cannot apply while daemon owns task storage",
        ));

    assert!(artifact.exists());
    drop(daemon_lease);
}

#[test]
fn storage_cleanup_requires_an_explicit_bound() {
    let root = TempDir::new().unwrap();

    suite_cmd()
        .args([
            "daemon",
            "storage",
            "cleanup",
            "--root",
            root.path().to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "cleanup requires --max-age-seconds, --max-bytes, or both",
        ));
}
