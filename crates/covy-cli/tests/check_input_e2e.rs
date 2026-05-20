#[path = "support/check.rs"]
mod check;

use check::{covy_cmd, fixture};
use predicates::prelude::*;
use tempfile::TempDir;

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
