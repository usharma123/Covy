#[path = "support/map_proxy.rs"]
mod map_proxy;
#[path = "support/map_proxy_repo.rs"]
mod map_proxy_repo;

use map_proxy::suite_cmd;
use map_proxy_repo::write_repo_fixture;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

fn kernel_cache_file(root: &Path) -> PathBuf {
    root.join(".packet28").join("packet-cache-v3.bin")
}

fn assert_persistence_failure(output: &Output, target: &str, message_fragment: &str) {
    assert!(output.stderr.is_empty());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        error.get("schema_version").and_then(Value::as_str),
        Some("suite.error.v1")
    );
    assert_eq!(error.get("target").and_then(Value::as_str), Some(target));
    assert!(error
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| message.contains(message_fragment)));
    assert!(error
        .get("causes")
        .and_then(Value::as_array)
        .and_then(|causes| causes.first())
        .and_then(Value::as_str)
        .is_some_and(|cause| cause.contains("cache persistence")));
}

#[test]
fn test_map_proxy_cache_map_repo_flag_writes_kernel_cache_file() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    let cache_file = kernel_cache_file(dir.path());
    assert!(!cache_file.exists());

    suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--cache",
            "--json",
        ])
        .assert()
        .success();

    assert!(cache_file.exists());
    assert!(fs::metadata(cache_file).unwrap().len() > 0);
}

#[test]
fn test_map_proxy_cache_proxy_run_flag_writes_kernel_cache_file() {
    let dir = TempDir::new().unwrap();
    let cache_file = kernel_cache_file(dir.path());
    assert!(!cache_file.exists());

    suite_cmd()
        .args([
            "proxy",
            "run",
            "--cache",
            "--cwd",
            dir.path().to_str().unwrap(),
            "--json",
            "--",
            "ls",
        ])
        .assert()
        .success();

    assert!(cache_file.exists());
    assert!(fs::metadata(cache_file).unwrap().len() > 0);
}

#[test]
fn test_map_repo_cache_reports_persistence_shutdown_failure() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    fs::write(dir.path().join(".packet28"), "not a directory").unwrap();

    let output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--cache",
            "--json",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert_persistence_failure(
        &output,
        "mapy.repo",
        "failed to durably finish repository map",
    );
}

#[test]
fn test_proxy_run_cache_reports_persistence_shutdown_failure() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(".packet28"), "not a directory").unwrap();

    let output = suite_cmd()
        .args([
            "proxy",
            "run",
            "--cache",
            "--cwd",
            dir.path().to_str().unwrap(),
            "--json",
            "--",
            "ls",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert_persistence_failure(&output, "proxy.run", "failed to durably finish proxy run");
}

#[test]
fn test_map_proxy_cache_map_repo_terminal_shows_hit_and_miss() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());

    let first = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--cache",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first_out = String::from_utf8(first).unwrap();
    assert!(first_out.contains("cache: miss"));

    let second = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--cache",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second_out = String::from_utf8(second).unwrap();
    assert!(second_out.contains("cache: hit"));
}
