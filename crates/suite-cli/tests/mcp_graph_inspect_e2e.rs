mod support;

use serde_json::json;
use std::io::{BufReader, Write};
use std::process::{ChildStdin, ChildStdout, Stdio};
use support::mcp::{
    initialize_mcp_session, packet28_cmd, packet28_process, read_mcp_message_for_id,
    write_mcp_message,
};
use tempfile::TempDir;

fn call_graph_tool(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    id: u64,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    write_mcp_message(
        stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":"tools/call",
            "params":{"name":name, "arguments":arguments}
        }),
    );
    read_mcp_message_for_id(stdout, id)
}

fn seed_graph(stdin: &mut ChildStdin, stdout: &mut BufReader<ChildStdout>) {
    call_graph_tool(
        stdin,
        stdout,
        2,
        "packet28.graph_create",
        json!({"name":"McpMemoir", "description":"MCP graph container"}),
    );
    call_graph_tool(
        stdin,
        stdout,
        3,
        "packet28.graph_add_concept",
        json!({"name":"Packet28", "description":"local context runtime", "memoir":"McpMemoir"}),
    );
    call_graph_tool(
        stdin,
        stdout,
        4,
        "packet28.graph_refine",
        json!({"name":"Packet28", "description":"local context runtime with reducers"}),
    );
    call_graph_tool(
        stdin,
        stdout,
        5,
        "packet28.graph_add_concept",
        json!({"name":"Reducers", "memoir":"McpMemoir"}),
    );
    call_graph_tool(
        stdin,
        stdout,
        6,
        "packet28.graph_link",
        json!({"source":"Packet28", "target":"Reducers", "relation":"uses"}),
    );
}

#[test]
fn test_mcp_graph_inspect_tools_round_trip() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let mut child = packet28_process()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["mcp", "serve", "--root", root.path().to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    initialize_mcp_session(&mut stdin, &mut stdout);
    seed_graph(&mut stdin, &mut stdout);

    let graph_show = call_graph_tool(
        &mut stdin,
        &mut stdout,
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
        &mut stdin,
        &mut stdout,
        8,
        "packet28.graph_inspect",
        json!({"limit": 5}),
    );
    assert!(graph["result"]["structuredContent"]["concepts"].is_array());

    let graph_concept_inspect = call_graph_tool(
        &mut stdin,
        &mut stdout,
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

    let _ = stdin.flush();
    let _ = child.kill();
    let _ = child.wait();
    packet28_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
