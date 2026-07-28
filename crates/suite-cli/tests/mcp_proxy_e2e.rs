#[path = "support/mcp_proxy.rs"]
mod mcp_proxy;
#[path = "support/mcp_proxy_fake.rs"]
mod mcp_proxy_fake;
#[expect(
    dead_code,
    reason = "shared integration harness APIs are exercised by sibling test binaries"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use mcp_proxy_fake::{
    write_colliding_tool_server, write_compact_read_server, write_concurrent_tool_server,
};
use process_harness::McpHarness;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

use mcp_proxy::{
    close_mcp_stdin, ensure_packet28d_built, init_repo, initialize_mcp_session, read_mcp_message,
    read_mcp_message_for_id, start_mcp_proxy_server, start_mcp_proxy_server_with_tool,
    stop_mcp_server, suite_cmd, write_mcp_message, write_repo_fixture,
};

fn read_next_mcp_response(server: &mut McpHarness) -> serde_json::Value {
    loop {
        let value = read_mcp_message(server);
        if value.get("id").is_some() {
            return value;
        }
    }
}
#[test]
#[cfg(unix)]
fn test_mcp_proxy_cli_namespaces_colliding_tools() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_alpha = dir.path().join("alpha_mcp.py");
    write_colliding_tool_server(&script_alpha, "alpha");

    let script_beta = dir.path().join("beta_mcp.py");
    write_colliding_tool_server(&script_beta, "beta");

    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "alpha": {
                    "command": "python3",
                    "args": ["-u", script_alpha.to_str().unwrap()]
                },
                "beta": {
                    "command": "python3",
                    "args": ["-u", script_beta.to_str().unwrap()]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let mut server = start_mcp_proxy_server(dir.path(), &config_path, "task-proxy-collision");

    initialize_mcp_session(&mut server);

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/list"
        }),
    );
    let tools = read_mcp_message_for_id(&mut server, 2);
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "alpha.shared.read"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "beta.shared.read"));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"beta.shared.read",
                "arguments":{}
            }
        }),
    );
    let response = read_mcp_message_for_id(&mut server, 3);
    assert_eq!(
        response["result"]["structuredContent"]["owner"]
            .as_str()
            .unwrap(),
        "beta"
    );

    stop_mcp_server(server);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_cli_compacts_allowlisted_read_tool_results() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("compact_mcp.py");
    write_compact_read_server(&script_path);

    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "compact": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "compact_tools": ["compact.read"]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut server, tools) = start_mcp_proxy_server_with_tool(
        dir.path(),
        &config_path,
        "task-proxy-compact",
        "compact.read",
    );
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "compact.read"));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"compact.read",
                "arguments":{}
            }
        }),
    );
    let compact = read_mcp_message_for_id(&mut server, 2);
    let compact_payload = &compact["result"]["structuredContent"];
    assert_eq!(compact_payload["response_mode"], "slim");
    assert_eq!(compact_payload["original_tool"], "compact.read");
    assert!(compact_payload["artifact_id"].as_str().is_some());
    let artifact_id = compact_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.fetch_tool_result",
                "arguments":{
                    "task_id":"task-proxy-compact",
                    "artifact_id": artifact_id
                }
            }
        }),
    );
    let fetched = read_mcp_message_for_id(&mut server, 3);
    let fetched_payload = &fetched["result"]["structuredContent"];
    assert_eq!(fetched_payload["structuredContent"]["path"], "src/alpha.rs");
    assert!(fetched_payload["structuredContent"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line == "pub struct Alpha;"));

    stop_mcp_server(server);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_routes_concurrent_and_late_responses_by_id() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("concurrent_mcp.py");
    write_concurrent_tool_server(&script_path);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "concurrent": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "timeout_ms": 500
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut server, _) = start_mcp_proxy_server_with_tool(
        dir.path(),
        &config_path,
        "task-proxy-concurrent",
        "concurrent.echo",
    );
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"slow",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"barrier":true,"delay_ms":100,"value":"slow"}
            }
        }),
    );
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"fast",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"barrier":true,"delay_ms":5,"value":"fast"}
            }
        }),
    );

    let first = read_next_mcp_response(&mut server);
    let second = read_next_mcp_response(&mut server);
    assert_eq!(first["id"], "fast");
    assert_eq!(first["result"]["structuredContent"]["value"], "fast");
    assert_eq!(second["id"], "slow");
    assert_eq!(second["result"]["structuredContent"]["value"], "slow");

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"will-time-out",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"delay_ms":800,"value":"late"}
            }
        }),
    );
    let timeout = read_next_mcp_response(&mut server);
    assert_eq!(timeout["id"], "will-time-out");
    assert!(timeout["error"]["message"]
        .as_str()
        .unwrap()
        .contains("500ms"));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"after-timeout",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"delay_ms":5,"value":"not-poisoned"}
            }
        }),
    );
    let after_timeout = read_next_mcp_response(&mut server);
    assert_eq!(after_timeout["id"], "after-timeout");
    assert_eq!(
        after_timeout["result"]["structuredContent"]["value"],
        "not-poisoned"
    );

    std::thread::sleep(std::time::Duration::from_millis(350));
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"after-late",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"delay_ms":5,"value":"still-correct"}
            }
        }),
    );
    let after_late = read_next_mcp_response(&mut server);
    assert_eq!(after_late["id"], "after-late");
    assert_eq!(
        after_late["result"]["structuredContent"]["value"],
        "still-correct"
    );

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"half-close",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"delay_ms":50,"value":"drained-before-shutdown"}
            }
        }),
    );
    close_mcp_stdin(&mut server);
    let drained = read_next_mcp_response(&mut server);
    assert_eq!(drained["id"], "half-close");
    assert_eq!(
        drained["result"]["structuredContent"]["value"],
        "drained-before-shutdown"
    );
    stop_mcp_server(server);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
