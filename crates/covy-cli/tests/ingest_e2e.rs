use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
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

#[test]
fn test_ingest_lcov() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("coverage.bin");

    covy_cmd()
        .args([
            "ingest",
            &fixture("lcov/basic.info"),
            "--output",
            output.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(output.exists());
}

#[test]
fn test_ingest_then_report() {
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

    covy_cmd()
        .args([
            "ingest",
            &fixture("cobertura/basic.xml"),
            "--output",
            output.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(output.exists());
}

#[test]
fn test_ingest_jacoco() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("coverage.bin");

    covy_cmd()
        .args([
            "ingest",
            &fixture("jacoco/basic.xml"),
            "--output",
            output.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(output.exists());
}

#[test]
fn test_ingest_gocov() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("coverage.bin");

    covy_cmd()
        .args([
            "ingest",
            &fixture("gocov/basic.out"),
            "--output",
            output.to_str().unwrap(),
        ])
        .assert()
        .success();

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

#[test]
fn test_ingest_accepts_legacy_out_alias_without_warning_noise() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("legacy.bin");
    covy_cmd()
        .args([
            "ingest",
            &fixture("lcov/basic.info"),
            "--out",
            output.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("deprecated").not());
    assert!(output.exists());
}

#[test]
fn test_ingest_legacy_out_alias_suppresses_warning_in_json_mode() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("legacy-json.bin");
    covy_cmd()
        .args([
            "ingest",
            &fixture("lcov/basic.info"),
            "--out",
            output.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("deprecated").not())
        .stdout(predicate::str::contains("\"output_coverage_path\""));
}

#[test]
fn test_ingest_deprecation_warning_can_be_enabled_via_env() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("legacy-env.bin");
    covy_cmd()
        .env("COVY_DEPRECATION_WARNINGS", "1")
        .args([
            "ingest",
            &fixture("lcov/basic.info"),
            "--out",
            output.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("deprecated"))
        .stderr(predicate::str::contains("--output"));
}
