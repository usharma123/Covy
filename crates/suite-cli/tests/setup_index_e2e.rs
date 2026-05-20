mod support;

use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
use std::io::BufReader;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use support::mcp::{
    initialize_mcp_session, packet28_cmd, packet28_process, read_mcp_message_for_id,
    write_mcp_message,
};
use tempfile::TempDir;

#[cfg(unix)]
fn write_repo_fixture(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub struct AlphaUniqueToken;\npub fn alpha_unique_token() -> &'static str { \"AlphaUniqueToken\" }\n",
    )
    .unwrap();
}

#[cfg(unix)]
fn start_mcp_server(root: &Path) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = packet28_process()
        .current_dir(root)
        .args(["mcp", "serve", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (child, stdin, stdout)
}

#[test]
#[cfg(unix)]
fn test_setup_index_builds_regex_index_and_search_uses_indexed_backend() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    write_repo_fixture(root.path());

    packet28_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "setup",
            "--root",
            root.path().to_str().unwrap(),
            "--runtime",
            "cursor",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("index ready"));

    assert!(root
        .path()
        .join(".packet28")
        .join("index")
        .join("regex-v1")
        .join("manifest.json")
        .exists());

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
            .and_then(|manifest| manifest.get("regex_status"))
            .and_then(Value::as_str),
        Some("ready")
    );

    let (mut child, mut stdin, mut stdout) = start_mcp_server(root.path());
    initialize_mcp_session(&mut stdin, &mut stdout);
    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"packet28.search",
                "arguments":{
                    "task_id":"task-setup-regex-index",
                    "query":"AlphaUniqueToken",
                    "fixed_string":true,
                    "response_mode":"full"
                }
            }
        }),
    );
    let search = read_mcp_message_for_id(&mut stdout, 2);
    assert_eq!(
        search["result"]["structuredContent"]["engine"]["engine"].as_str(),
        Some("indexed_regex")
    );
    assert!(
        search["result"]["structuredContent"]["match_count"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    );

    let _ = child.kill();
    let _ = child.wait();
    packet28_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
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
