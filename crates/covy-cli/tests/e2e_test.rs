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
fn test_help() {
    covy_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Universal code coverage tool"));
}

#[test]
fn test_report_no_data() {
    covy_cmd()
        .args(["report", "--input", "/nonexistent/path.bin"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No coverage data found"));
}

#[test]
fn test_report_min_coverage_fail() {
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
            "--min-coverage",
            "95.0",
            "--color",
            "never",
        ])
        .assert()
        .code(1);
}

#[test]
fn test_diff_returns_failure_exit_code_when_gate_fails() {
    covy_cmd()
        .args([
            "diff",
            "--coverage",
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

#[test]
fn test_init_defaults_to_cwd_not_git_root() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("README.md"), "init\n").unwrap();
    setup_git_repo(dir.path());

    let sub = dir.path().join("subproject");
    std::fs::create_dir_all(&sub).unwrap();

    covy_cmd()
        .current_dir(&sub)
        .args(["init"])
        .assert()
        .success();

    assert!(sub.join("covy.toml").exists());
    assert!(sub.join(".covy/state").exists());
    assert!(sub.join(".covy/cache").exists());
    assert!(!dir.path().join("covy.toml").exists());
}
