use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn testy_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("testy")
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

fn write_manifest(path: &Path) {
    let line = format!(
        "{{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    std::fs::write(path, line).unwrap();
}

#[test]
fn test_testy_testmap_smoke() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    write_manifest(&manifest);

    testy_cmd()
        .args([
            "testmap",
            "build",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Built testmap"));

    assert!(testmap.exists());
}

#[test]
fn test_testy_impact_smoke() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    write_manifest(&manifest);

    testy_cmd()
        .args([
            "testmap",
            "build",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success();

    testy_cmd()
        .args([
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"selected_tests\""));
}

#[test]
fn test_testy_shard_smoke() {
    let dir = TempDir::new().unwrap();
    let tests_file = dir.path().join("tests.txt");
    std::fs::write(&tests_file, "com.foo.A\ncom.foo.B\n").unwrap();

    testy_cmd()
        .args([
            "shard",
            "plan",
            "--shards",
            "2",
            "--tests-file",
            tests_file.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"shards\""));
}

#[test]
fn test_testy_shard_rejects_malformed_config() {
    let dir = TempDir::new().unwrap();
    let config = dir.path().join("covy.toml");
    let tests_file = dir.path().join("tests.txt");
    std::fs::write(&config, "[shard\nalgorithm = \"lpt\"").unwrap();
    std::fs::write(&tests_file, "com.foo.A\n").unwrap();

    testy_cmd()
        .args([
            "--config",
            config.to_str().unwrap(),
            "shard",
            "plan",
            "--shards",
            "1",
            "--tests-file",
            tests_file.to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(
            predicate::str::contains("failed to parse config at")
                .and(predicate::str::contains(config.to_str().unwrap())),
        );
}

#[test]
fn test_testy_missing_impact_plan_preserves_error_output_and_exit_code() {
    testy_cmd()
        .args(["impact", "run", "--", "cargo", "test"])
        .assert()
        .code(2)
        .stdout("")
        .stderr("error: Missing --plan. Use: testy impact run --plan plan.json -- <command>\n");
}

#[test]
fn test_testy_missing_impact_plan_file_preserves_source_reporting() {
    let directory = TempDir::new().unwrap();
    let missing_plan = directory.path().join("missing-plan.json");

    testy_cmd()
        .args([
            "impact",
            "run",
            "--plan",
            missing_plan.to_str().unwrap(),
            "--",
            "true",
        ])
        .assert()
        .code(2)
        .stdout("")
        .stderr(
            predicate::str::contains(format!(
                "error: Failed to read plan at {}",
                missing_plan.display()
            ))
            .and(predicate::str::contains("caused by:"))
            .and(predicate::str::contains("No such file or directory")),
        );
}
