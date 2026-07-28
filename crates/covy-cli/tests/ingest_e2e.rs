#[path = "support/ingest.rs"]
mod ingest;

use ingest::{covy_cmd, fixture, ingest_fixture};
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn test_ingest_lcov() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("coverage.bin");

    ingest_fixture("lcov/basic.info", &output);

    assert!(output.exists());
}

#[test]
fn test_ingest_then_report() {
    let dir = TempDir::new().unwrap();
    let state_file = dir.path().join("state.bin");

    ingest_fixture("lcov/basic.info", &state_file);

    covy_cmd()
        .args([
            "report",
            "--input",
            state_file.to_str().unwrap(),
            "--color",
            "never",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/main.rs"));

    covy_cmd()
        .args([
            "report",
            "--input",
            state_file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("total_coverage_pct"));
}

#[test]
fn test_ingest_cobertura() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("coverage.bin");

    ingest_fixture("cobertura/basic.xml", &output);

    assert!(output.exists());
}

#[test]
fn test_ingest_jacoco() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("coverage.bin");

    ingest_fixture("jacoco/basic.xml", &output);

    assert!(output.exists());
}

#[test]
fn test_ingest_gocov() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("coverage.bin");

    ingest_fixture("gocov/basic.out", &output);

    assert!(output.exists());
}

#[test]
fn test_ingest_with_strip_prefix() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("coverage.bin");

    covy_cmd()
        .args([
            "ingest",
            &fixture("lcov/basic.info"),
            "--output",
            output.to_str().unwrap(),
            "--strip-prefix",
            "src/",
        ])
        .assert()
        .success();

    covy_cmd()
        .args([
            "report",
            "--input",
            output.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("main.rs"))
        .stdout(predicate::str::contains("lib.rs"));
}

#[test]
fn test_ingest_issues_creates_state_file() {
    let dir = TempDir::new().unwrap();

    covy_cmd()
        .current_dir(dir.path())
        .args(["ingest", "--issues", &fixture("sarif/basic.sarif")])
        .assert()
        .success();

    assert!(dir.path().join(".covy/state/issues.bin").exists());
}

#[test]
fn test_ingest_quiet_json_emits_machine_summary() {
    covy_cmd()
        .args(["ingest", &fixture("lcov/basic.info"), "--json", "-q"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"coverage_inputs\""))
        .stdout(predicate::str::contains("\"output_coverage_path\""));
}

#[test]
fn test_ingest_quiet_without_json_emits_machine_summary() {
    covy_cmd()
        .args(["ingest", &fixture("lcov/basic.info"), "-q"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"coverage_inputs\""))
        .stdout(predicate::str::contains("\"output_coverage_path\""));
}
