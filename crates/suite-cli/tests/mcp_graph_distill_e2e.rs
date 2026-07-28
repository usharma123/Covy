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
fn test_mcp_graph_distill_and_extract_patterns() {
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
                "name":"packet28.memory_store",
                "arguments":{
                    "content":"Distill MCP memory into a graph concept",
                    "topic":"mcp-distill",
                    "keywords":"McpDistill,graph",
                    "importance":"critical"
                }
            }
        }),
    );
    let mcp_distill_memory = read_mcp_message_for_id(&mut stdout, 3);
    assert_eq!(
        mcp_distill_memory["result"]["structuredContent"]["topic"].as_str(),
        Some("mcp-distill")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_distill",
                "arguments":{"from_topic":"mcp-distill", "into":"McpMemoir", "limit": 5}
            }
        }),
    );
    let graph_distill = read_mcp_message_for_id(&mut stdout, 4);
    assert_eq!(
        graph_distill["result"]["structuredContent"]["created_count"].as_u64(),
        Some(2)
    );
    assert_eq!(
        graph_distill["result"]["structuredContent"]["concepts"][0]["name"].as_str(),
        Some("McpDistill")
    );
    assert_eq!(
        graph_distill["result"]["structuredContent"]["concepts"][1]["name"].as_str(),
        Some("graph")
    );

    for (id, content) in [
        (5, "Pattern extraction should group adapter memories"),
        (6, "Adapter pattern extraction should create graph concepts"),
    ] {
        write_mcp_message(
            &mut stdin,
            &json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"tools/call",
                "params":{
                    "name":"packet28.memory_store",
                    "arguments":{
                        "content":content,
                        "topic":"mcp-patterns",
                        "keywords":"adapter,pattern",
                        "importance":"critical"
                    }
                }
            }),
        );
        let stored_pattern_memory = read_mcp_message_for_id(&mut stdout, id);
        assert_eq!(
            stored_pattern_memory["result"]["structuredContent"]["topic"].as_str(),
            Some("mcp-patterns")
        );
    }

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":7,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_extract_patterns",
                "arguments":{"topic":"mcp-patterns", "memoir":"McpMemoir", "min_cluster_size":2}
            }
        }),
    );
    let memory_patterns = read_mcp_message_for_id(&mut stdout, 7);
    assert!(
        memory_patterns["result"]["structuredContent"]["pattern_count"]
            .as_u64()
            .unwrap()
            >= 2
    );
    assert!(memory_patterns["result"]["structuredContent"]["patterns"]
        .as_array()
        .unwrap()
        .iter()
        .any(|pattern| pattern["key"] == "adapter" && pattern["memory_count"].as_u64() == Some(2)));
    assert!(
        memory_patterns["result"]["structuredContent"]["created_concepts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|concept| concept["name"] == "adapter" && concept["memoir_name"] == "McpMemoir")
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
