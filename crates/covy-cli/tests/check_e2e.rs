#[path = "support/check.rs"]
mod check;

use check::{covy_cmd, fixture};
use predicates::prelude::*;
use std::path::Path;
use tempfile::TempDir;

fn setup_git_repo(dir: &Path) {
    let init_status = std::process::Command::new("git")
        .current_dir(dir)
        .args(["init"])
        .status()
        .expect("failed to execute `git init`");
    assert!(
        init_status.success(),
        "`git init` exited with {init_status}"
    );

    let add_status = std::process::Command::new("git")
        .current_dir(dir)
        .args(["add", "README.md"])
        .status()
        .expect("failed to execute `git add README.md`");
    assert!(
        add_status.success(),
        "`git add README.md` exited with {add_status}"
    );

    let commit_status = std::process::Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "init",
        ])
        .status()
        .expect("failed to execute initial git commit");
    assert!(
        commit_status.success(),
        "`git commit -m init` exited with {commit_status}"
    );
}

#[test]
fn test_check_without_issues_still_works() {
    covy_cmd()
        .args([
            "check",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--report",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("passed"));
}

#[test]
fn test_check_loads_coverage_from_state_by_default() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    setup_git_repo(dir.path());

    covy_cmd()
        .current_dir(dir.path())
        .args(["ingest", &fixture("lcov/basic.info")])
        .assert()
        .success();

    covy_cmd()
        .current_dir(dir.path())
        .args([
            "check", "--base", "HEAD", "--head", "HEAD", "--report", "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("passed"));
}

#[test]
fn test_check_still_returns_failure_exit_code_when_gate_fails() {
    covy_cmd()
        .args([
            "check",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--fail-under-total",
            "101",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--report",
            "json",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"passed\": false"));
}

#[test]
fn test_check_without_coverage_and_state_fails() {
    let dir = TempDir::new().unwrap();

    covy_cmd()
        .current_dir(dir.path())
        .args([
            "check", "--base", "HEAD", "--head", "HEAD", "--report", "json",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "No coverage files specified and no cached coverage state found",
        ));
}

#[test]
fn test_check_json_output_stays_on_stdout() {
    covy_cmd()
        .args([
            "check",
            &fixture("lcov/basic.info"),
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"passed\""))
        .stderr(predicate::str::contains("\"passed\"").not());
}

#[test]
fn test_check_rejects_malformed_config_instead_of_running_with_defaults() {
    let dir = TempDir::new().unwrap();
    let config = dir.path().join("covy.toml");
    std::fs::write(&config, "[gate\nfail_under_total = 101").unwrap();

    covy_cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "check",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--report",
            "json",
        ])
        .assert()
        .code(2)
        .stderr(
            predicate::str::contains("failed to parse config at")
                .and(predicate::str::contains(config.to_str().unwrap())),
        );
}

#[test]
fn test_check_keeps_default_behavior_for_missing_config() {
    let dir = TempDir::new().unwrap();
    let missing_config = dir.path().join("missing.toml");

    covy_cmd()
        .args([
            "--config",
            missing_config.to_str().unwrap(),
            "check",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--report",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"passed\": true"));
}
