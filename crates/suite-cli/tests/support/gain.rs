use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn init_git_status_fixture(root: &TempDir) {
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(root.path())
        .status()
        .unwrap();
    fs::write(root.path().join("tracked.txt"), "changed\n").unwrap();
}

pub fn record_git_status_run(root: &TempDir) {
    suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "git",
            "status",
            "--short",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"git\""))
        .stdout(predicate::str::contains("\"raw_est_tokens\""))
        .stdout(predicate::str::contains("\"savings_percent\""))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));
}
