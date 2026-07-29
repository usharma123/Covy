#[path = "support/hypothesis.rs"]
mod hypothesis;
#[expect(
    dead_code,
    reason = "this integration binary exercises a focused subset of the shared harness"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use hypothesis::{ensure_packet28d_built, init_repo, suite_cmd, write_repo_fixture};
use process_harness::{HarnessLimits, McpHarness};
use serde_json::{json, Value};
use std::time::Duration;
use tempfile::TempDir;

const MCP_IO_TIMEOUT: Duration = Duration::from_secs(10);
const MCP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

fn call_tool(server: &mut McpHarness, id: u64, name: &str, arguments: Value) -> Value {
    server
        .request_with_id(
            json!(id),
            "tools/call",
            json!({"name": name, "arguments": arguments}),
            MCP_IO_TIMEOUT,
        )
        .unwrap_or_else(|error| panic!("MCP tool {name} failed: {error}"))
}

#[test]
#[cfg(unix)]
fn test_hypothesis_mcp_tools_track_active_assumptions() {
    ensure_packet28d_built();
    let root = TempDir::new().unwrap();
    init_repo(root.path());
    write_repo_fixture(root.path());
    let task_id = "task-mcp-hypothesis";

    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    command.current_dir(root.path()).args([
        "mcp",
        "serve",
        "--root",
        root.path().to_str().unwrap(),
    ]);
    let mut server = McpHarness::spawn(&mut command, HarnessLimits::default())
        .unwrap_or_else(|error| panic!("failed to start MCP server: {error}"));
    server
        .request_with_id(
            json!(1),
            "initialize",
            json!({
                "protocolVersion":"2024-11-05",
                "capabilities":{},
                "clientInfo":{"name":"test","version":"1"}
            }),
            MCP_IO_TIMEOUT,
        )
        .unwrap_or_else(|error| panic!("failed to initialize MCP server: {error}"));

    let added = call_tool(
        &mut server,
        2,
        "packet28.hypothesis_add",
        json!({
                    "task_id":task_id,
                    "id":"auth-cache",
                    "text":"Auth cache invalidation is the regression source",
                    "paths":["src/auth.rs"],
                    "symbols":["AuthCache"],
                    "artifact_id":"artifact-auth-cache"
        }),
    );
    let added_payload = &added["result"]["structuredContent"];
    assert_eq!(added_payload["id"], "auth-cache");
    assert_eq!(added_payload["status"], "active");
    assert_eq!(added_payload["decision_id"], "hypothesis:auth-cache");

    let listed = call_tool(
        &mut server,
        3,
        "packet28.hypothesis_list",
        json!({
                    "task_id":task_id
        }),
    );
    let listed_payload = listed["result"]["structuredContent"].as_array().unwrap();
    assert_eq!(listed_payload.len(), 1);
    assert_eq!(listed_payload[0]["id"], "auth-cache");
    assert_eq!(
        listed_payload[0]["text"],
        "Auth cache invalidation is the regression source"
    );
    assert_eq!(listed_payload[0]["related_paths"][0], "src/auth.rs");
    assert_eq!(listed_payload[0]["related_symbols"][0], "AuthCache");
    assert_eq!(
        listed_payload[0]["related_artifact_ids"][0],
        "artifact-auth-cache"
    );

    let rejected = call_tool(
        &mut server,
        4,
        "packet28.hypothesis_resolve",
        json!({
                    "task_id":task_id,
                    "id":"auth-cache",
                    "status":"rejected"
        }),
    );
    assert_eq!(
        rejected["result"]["structuredContent"]["status"],
        "rejected"
    );

    let listed_after_reject = call_tool(
        &mut server,
        5,
        "packet28.hypothesis_list",
        json!({
                    "task_id":task_id
        }),
    );
    assert!(listed_after_reject["result"]["structuredContent"]
        .as_array()
        .unwrap()
        .is_empty());

    server
        .finish(MCP_SHUTDOWN_TIMEOUT)
        .unwrap_or_else(|error| panic!("failed to stop MCP server: {error}"));

    suite_cmd()
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
