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
