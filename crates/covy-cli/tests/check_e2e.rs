use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn covy_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("covy")
}

fn fixture(rel: &str) -> String {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    workspace
        .join("tests")
        .join("fixtures")
        .join(rel)
        .to_string_lossy()
        .to_string()
}

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
fn test_check_with_issues_flag() {
    covy_cmd()
        .args([
            "check",
            &fixture("lcov/basic.info"),
            "--issues",
            &fixture("sarif/basic.sarif"),
            "--max-new-errors",
            "0",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--report",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("issue_counts"));
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
fn test_check_loads_issues_from_state_by_default() {
    covy_cmd()
        .args(["ingest", "--issues", &fixture("sarif/basic.sarif")])
        .assert()
        .success();

    covy_cmd()
        .args([
            "check",
            &fixture("lcov/basic.info"),
            "--max-new-errors",
            "0",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--report",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("issue_counts"));
}

#[test]
fn test_check_can_disable_state_issues_loading() {
    covy_cmd()
        .args(["ingest", "--issues", &fixture("sarif/basic.sarif")])
        .assert()
        .success();

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
        .stdout(predicate::str::contains("issue_counts").not());
}

#[test]
fn test_check_accepts_packed_issues_input() {
    covy_cmd()
        .args(["ingest", "--issues", &fixture("sarif/basic.sarif")])
        .assert()
        .success();

    covy_cmd()
        .args([
            "check",
            &fixture("lcov/basic.info"),
            "--issues",
            ".covy/state/issues.bin",
            "--max-new-errors",
            "0",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--report",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("issue_counts"));
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
fn test_check_input_works_for_state_file() {
    let dir = TempDir::new().unwrap();
    let state_file = dir.path().join("state.bin");

    covy_cmd()
        .args([
            "ingest",
            &fixture("lcov/basic.info"),
            "--output",
            state_file.to_str().unwrap(),
        ])
        .assert()
        .success();

    covy_cmd()
        .args([
            "check",
            "--input",
            state_file.to_str().unwrap(),
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"passed\""));
}

#[test]
fn test_check_rejects_conflicting_input_and_stdin() {
    let dir = TempDir::new().unwrap();
    let state_file = dir.path().join("state.bin");
    std::fs::write(&state_file, "not-real-state").unwrap();

    covy_cmd()
        .args(["check", "--stdin", "--input", state_file.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Cannot combine --input with --stdin",
        ));
}

#[test]
fn test_check_missing_input_file_fails_with_usage_code() {
    covy_cmd()
        .args([
            "check",
            "--input",
            "/definitely/missing/state.bin",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("No coverage data found"));
}
