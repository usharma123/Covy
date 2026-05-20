mod support;

use serde_json::json;
use std::io::BufReader;
use std::process::Stdio;
use support::mcp::{
    initialize_mcp_session, packet28_cmd, packet28_process, read_mcp_message_for_id,
    write_mcp_message,
};
use tempfile::TempDir;

#[test]
fn test_mcp_graph_tools_round_trip() {
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

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_create",
                "arguments":{"name":"McpMemoir", "description":"MCP graph container"}
            }
        }),
    );
    let graph_memoir = read_mcp_message_for_id(&mut stdout, 2);
    assert_eq!(
        graph_memoir["result"]["structuredContent"]["name"].as_str(),
        Some("McpMemoir")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_add_concept",
                "arguments":{
                    "name":"Packet28",
                    "description":"local context runtime",
                    "memoir":"McpMemoir",
                    "labels":["domain:context"],
                    "confidence":0.91,
                    "source_ids":["memory:mcp"]
                }
            }
        }),
    );
    let graph_concept = read_mcp_message_for_id(&mut stdout, 3);
    assert_eq!(
        graph_concept["result"]["structuredContent"]["name"].as_str(),
        Some("Packet28")
    );
    assert_eq!(
        graph_concept["result"]["structuredContent"]["memoir_name"].as_str(),
        Some("McpMemoir")
    );
    assert_eq!(
        graph_concept["result"]["structuredContent"]["confidence"].as_f64(),
        Some(0.91)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_refine",
                "arguments":{"name":"Packet28", "description":"local context runtime with reducers"}
            }
        }),
    );
    let refined = read_mcp_message_for_id(&mut stdout, 4);
    assert_eq!(
        refined["result"]["structuredContent"]["description"].as_str(),
        Some("local context runtime with reducers")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_add_concept",
                "arguments":{"name":"Reducers", "memoir":"McpMemoir"}
            }
        }),
    );
    let reducer_concept = read_mcp_message_for_id(&mut stdout, 5);
    assert_eq!(
        reducer_concept["result"]["structuredContent"]["memoir_name"].as_str(),
        Some("McpMemoir")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_link",
                "arguments":{"source":"Packet28", "target":"Reducers", "relation":"uses"}
            }
        }),
    );
    let relation = read_mcp_message_for_id(&mut stdout, 6);
    assert_eq!(
        relation["result"]["structuredContent"]["relation"].as_str(),
        Some("uses")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":7,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_search",
                "arguments":{"query":"context", "memoir":"McpMemoir", "label":"domain:context", "limit": 5}
            }
        }),
    );
    let graph_search = read_mcp_message_for_id(&mut stdout, 7);
    assert!(!graph_search["result"]["structuredContent"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        graph_search["result"]["structuredContent"][0]["memoir_name"].as_str(),
        Some("McpMemoir")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":8,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_export",
                "arguments":{"format":"dot", "limit": 5}
            }
        }),
    );
    let graph_export = read_mcp_message_for_id(&mut stdout, 8);
    assert_eq!(
        graph_export["result"]["structuredContent"]["format"].as_str(),
        Some("dot")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":9,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_stats",
                "arguments":{}
            }
        }),
    );
    let graph_stats = read_mcp_message_for_id(&mut stdout, 9);
    assert!(
        graph_stats["result"]["structuredContent"]["relation_count"]
            .as_i64()
            .unwrap()
            >= 1
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":10,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_show",
                "arguments":{"name":"McpMemoir", "limit": 5}
            }
        }),
    );
    let graph_show = read_mcp_message_for_id(&mut stdout, 10);
    assert_eq!(
        graph_show["result"]["structuredContent"]["memoir"]["name"].as_str(),
        Some("McpMemoir")
    );
    assert_eq!(
        graph_show["result"]["structuredContent"]["concepts"][0]["revision"].as_i64(),
        Some(2)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":11,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_inspect",
                "arguments":{"limit": 5}
            }
        }),
    );
    let graph = read_mcp_message_for_id(&mut stdout, 11);
    assert!(graph["result"]["structuredContent"]["concepts"].is_array());

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":12,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_inspect_concept",
                "arguments":{"name":"Packet28", "memoir":"McpMemoir", "depth": 1}
            }
        }),
    );
    let graph_concept_inspect = read_mcp_message_for_id(&mut stdout, 12);
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

    let _ = child.kill();
    let _ = child.wait();
    packet28_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
