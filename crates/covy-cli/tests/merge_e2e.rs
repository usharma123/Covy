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
fn test_merge_non_strict_skips_corrupt_artifacts() {
    let dir = TempDir::new().unwrap();
    let bad = dir.path().join("bad.bin");
    std::fs::write(&bad, b"broken").unwrap();

    covy_cmd()
        .args([
            "merge",
            "--coverage",
            bad.to_str().unwrap(),
            "--strict",
            "false",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"skipped_inputs\": 1"))
        .stdout(predicate::str::contains("\"strict_mode\": false"))
        .stdout(predicate::str::contains("\"output_coverage_path\""));
}

#[test]
fn test_merge_strict_fails_on_corrupt_artifacts() {
    let dir = TempDir::new().unwrap();
    let bad = dir.path().join("bad.bin");
    std::fs::write(&bad, b"broken").unwrap();

    covy_cmd()
        .args([
            "merge",
            "--coverage",
            bad.to_str().unwrap(),
            "--strict",
            "true",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Failed to merge coverage input"));
}

#[test]
fn test_merge_writes_output_coverage_state() {
    let dir = TempDir::new().unwrap();
    let shard = dir.path().join("shard.bin");
    let merged = dir.path().join("merged.bin");

    covy_cmd()
        .args([
            "ingest",
            &fixture("lcov/basic.info"),
            "--output",
            shard.to_str().unwrap(),
        ])
        .assert()
        .success();

    covy_cmd()
        .args([
            "merge",
            "--coverage",
            shard.to_str().unwrap(),
            "--output-coverage",
            merged.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    assert!(merged.exists());
}

#[test]
fn test_merge_writes_output_issues_state() {
    let dir = TempDir::new().unwrap();
    let shard = dir.path().join("issues-shard.bin");
    let merged = dir.path().join("issues-merged.bin");

    covy_cmd()
        .current_dir(dir.path())
        .args(["ingest", "--issues", &fixture("sarif/basic.sarif")])
        .assert()
        .success();

    std::fs::copy(dir.path().join(".covy/state/issues.bin"), &shard).unwrap();

    covy_cmd()
        .args([
            "merge",
            "--issues",
            shard.to_str().unwrap(),
            "--output-issues",
            merged.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    assert!(merged.exists());
}
