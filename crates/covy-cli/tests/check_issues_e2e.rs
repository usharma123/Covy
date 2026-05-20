#[path = "support/check.rs"]
mod check;

use check::{covy_cmd, fixture};
use predicates::prelude::*;

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
