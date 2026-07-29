mod support;

#[path = "support/setup_index.rs"]
mod setup_index;

use predicates::prelude::*;
use serde_json::{json, Value};
use setup_index::write_repo_fixture;
use support::mcp::{
    initialize_mcp_session, packet28_cmd, packet28_process, read_mcp_message_for_id, spawn_mcp,
    stop_mcp_server, write_mcp_message,
};
use tempfile::TempDir;

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

    let mut command = packet28_process();
    command.current_dir(root.path()).args([
        "mcp",
        "serve",
        "--root",
        root.path().to_str().unwrap(),
    ]);
    let mut server = spawn_mcp(&mut command);
    initialize_mcp_session(&mut server);
    write_mcp_message(
        &mut server,
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
    let search = read_mcp_message_for_id(&mut server, 2);
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

    stop_mcp_server(server);
    packet28_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
