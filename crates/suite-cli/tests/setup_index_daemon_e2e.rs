#![cfg(unix)]

#[path = "support/setup_index.rs"]
mod setup_index;

use assert_cmd::Command;
use serde_json::Value;
use setup_index::write_repo_fixture;
use tempfile::TempDir;

fn packet28_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_setup_index_daemon_start_and_manual_rebuild_coalesce_full_index() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    write_repo_fixture(root.path());

    packet28_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args(["daemon", "start", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();

    packet28_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "daemon",
            "index",
            "rebuild",
            "--root",
            root.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let started = std::time::Instant::now();
    loop {
        let status_output = packet28_cmd()
            .current_dir(root.path())
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .args([
                "daemon",
                "index",
                "status",
                "--root",
                root.path().to_str().unwrap(),
                "--json",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let status: Value = serde_json::from_slice(&status_output).unwrap();
        if status.get("ready").and_then(Value::as_bool) == Some(true) {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "index did not become ready after duplicate rebuild request: {status}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    std::thread::sleep(std::time::Duration::from_millis(500));
    let status_output = packet28_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "daemon",
            "index",
            "status",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: Value = serde_json::from_slice(&status_output).unwrap();
    assert_eq!(status.get("ready").and_then(Value::as_bool), Some(true));
    assert_eq!(
        status
            .get("manifest")
            .and_then(|manifest| manifest.get("status"))
            .and_then(Value::as_str),
        Some("ready")
    );

    packet28_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
