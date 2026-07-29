mod support;

use serde_json::json;
use support::mcp::{
    initialize_mcp_session, packet28_cmd, packet28_process, read_mcp_message_for_id, spawn_mcp,
    stop_mcp_server, write_mcp_message,
};
use support::process_harness::McpHarness;
use tempfile::TempDir;

fn call_graph_tool(
    server: &mut McpHarness,
    id: u64,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    write_mcp_message(
        server,
        &json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":"tools/call",
            "params":{"name":name, "arguments":arguments}
        }),
    );
    read_mcp_message_for_id(server, id)
}

fn seed_graph(server: &mut McpHarness) {
    call_graph_tool(
        server,
        2,
        "packet28.graph_create",
        json!({"name":"McpMemoir", "description":"MCP graph container"}),
    );
    call_graph_tool(
        server,
        3,
        "packet28.graph_add_concept",
        json!({"name":"Packet28", "description":"local context runtime", "memoir":"McpMemoir"}),
    );
    call_graph_tool(
        server,
        4,
        "packet28.graph_refine",
        json!({"name":"Packet28", "description":"local context runtime with reducers"}),
    );
    call_graph_tool(
        server,
        5,
        "packet28.graph_add_concept",
        json!({"name":"Reducers", "memoir":"McpMemoir"}),
    );
    call_graph_tool(
        server,
        6,
        "packet28.graph_link",
        json!({"source":"Packet28", "target":"Reducers", "relation":"uses"}),
    );
}

#[test]
fn test_mcp_graph_inspect_tools_round_trip() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let mut command = packet28_process();
    command
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["mcp", "serve", "--root", root.path().to_str().unwrap()]);
    let mut server = spawn_mcp(&mut command);
    initialize_mcp_session(&mut server);
    seed_graph(&mut server);

    let graph_show = call_graph_tool(
        &mut server,
        7,
        "packet28.graph_show",
        json!({"name":"McpMemoir", "limit": 5}),
    );
    assert_eq!(
        graph_show["result"]["structuredContent"]["memoir"]["name"].as_str(),
        Some("McpMemoir")
    );
    assert_eq!(
        graph_show["result"]["structuredContent"]["concepts"][0]["revision"].as_i64(),
        Some(2)
    );

    let graph = call_graph_tool(
        &mut server,
        8,
        "packet28.graph_inspect",
        json!({"limit": 5}),
    );
    assert!(graph["result"]["structuredContent"]["concepts"].is_array());

    let graph_concept_inspect = call_graph_tool(
        &mut server,
        9,
        "packet28.graph_inspect_concept",
        json!({"name":"Packet28", "memoir":"McpMemoir", "depth": 1}),
    );
    assert_eq!(
        graph_concept_inspect["result"]["structuredContent"]["concept"]["name"].as_str(),
        Some("Packet28")
    );
    assert!(
        graph_concept_inspect["result"]["structuredContent"]["neighbors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|concept| concept["name"] == "Reducers")
    );

    stop_mcp_server(server);
    packet28_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
