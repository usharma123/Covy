#[path = "support/run_raw_artifact.rs"]
mod run_raw_artifact;

use predicates::prelude::*;
use run_raw_artifact::{init_repo, suite_cmd};
use serde_json::{self, Value};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_run_raw_artifact_reduced_command_is_fetchable() {
    let root = TempDir::new().unwrap();
    init_repo(root.path());
    fs::write(root.path().join("raw-visible.txt"), "changed\n").unwrap();

    let output = suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "git",
            "status",
            "--short",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let handle = value["raw_artifact"]["handle"].as_str().unwrap();
    assert!(value["raw_artifact"]["available"].as_bool().unwrap());

    suite_cmd()
        .current_dir(root.path())
        .args([
            "compact",
            "fetch-raw",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            "run-raw",
            "--handle",
            handle,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("raw-visible.txt"))
        .stdout(predicate::str::contains("--- stdout ---"));
}

#[test]
fn test_run_raw_artifact_fallback_command_is_fetchable() {
    let root = TempDir::new().unwrap();

    let output = suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "sh",
            "-c",
            "echo fallback-stdout; echo fallback-stderr >&2",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["fallback_reason"], "unsupported");
    assert_eq!(value["command"]["exit_code"], 0);
    assert!(value["raw_artifact"]["available"].as_bool().unwrap());
    let handle = value["raw_artifact"]["handle"].as_str().unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "compact",
            "fetch-raw",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            "run-raw",
            "--handle",
            handle,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("fallback-stdout"))
        .stdout(predicate::str::contains("fallback-stderr"));
}

#[test]
fn test_run_raw_artifact_failing_reduced_command_preserves_exit_and_stderr() {
    let root = TempDir::new().unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"packet28-broken-run-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/lib.rs"), "pub fn broken( {}\n").unwrap();

    let output = suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "cargo",
            "check",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(101));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["fallback_reason"], Value::Null);
    assert_eq!(value["command"]["exit_code"], 101);
    assert_eq!(value["reduction"]["exit_code"], 101);
    assert!(value["raw_artifact"]["available"].as_bool().unwrap());
    let handle = value["raw_artifact"]["handle"].as_str().unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "compact",
            "fetch-raw",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            "run-raw",
            "--handle",
            handle,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("exit_code: 101"))
        .stdout(predicate::str::contains("unclosed delimiter"))
        .stdout(predicate::str::contains("cargo check"));
}
