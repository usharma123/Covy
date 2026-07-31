#[path = "support/mcp_graph.rs"]
mod mcp_graph;
#[expect(
    dead_code,
    reason = "shared integration harness APIs are exercised by sibling test binaries"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use mcp_graph::McpGraphServer;
use serde_json::json;

#[test]
fn test_mcp_graph_tools_round_trip() {
    let mut server = McpGraphServer::start();

    let graph_memoir = server.call_tool(
        2,
        "packet28.graph_create",
        json!({"name":"McpMemoir", "description":"MCP graph container"}),
    );
    assert_eq!(
        graph_memoir["result"]["structuredContent"]["name"].as_str(),
        Some("McpMemoir")
    );

    let graph_concept = server.call_tool(
        3,
        "packet28.graph_add_concept",
        json!({
            "name":"Packet28",
            "description":"local context runtime",
            "memoir":"McpMemoir",
            "labels":["domain:context"],
            "confidence":0.91,
            "source_ids":["memory:mcp"]
        }),
    );
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

    let refined = server.call_tool(
        4,
        "packet28.graph_refine",
        json!({"name":"Packet28", "description":"local context runtime with reducers"}),
    );
    assert_eq!(
        refined["result"]["structuredContent"]["description"].as_str(),
        Some("local context runtime with reducers")
    );

    let reducer_concept = server.call_tool(
        5,
        "packet28.graph_add_concept",
        json!({"name":"Reducers", "memoir":"McpMemoir"}),
    );
    assert_eq!(
        reducer_concept["result"]["structuredContent"]["memoir_name"].as_str(),
        Some("McpMemoir")
    );

    let relation = server.call_tool(
        6,
        "packet28.graph_link",
        json!({"source":"Packet28", "target":"Reducers", "relation":"uses"}),
    );
    assert_eq!(
        relation["result"]["structuredContent"]["relation"].as_str(),
        Some("uses")
    );

    let graph_search = server.call_tool(
        7,
        "packet28.graph_search",
        json!({"query":"context", "memoir":"McpMemoir", "label":"domain:context", "limit": 5}),
    );
    assert!(!graph_search["result"]["structuredContent"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        graph_search["result"]["structuredContent"][0]["memoir_name"].as_str(),
        Some("McpMemoir")
    );

    let graph_export = server.call_tool(
        8,
        "packet28.graph_export",
        json!({"format":"dot", "limit": 5}),
    );
    assert_eq!(
        graph_export["result"]["structuredContent"]["format"].as_str(),
        Some("dot")
    );

    let graph_stats = server.call_tool(9, "packet28.graph_stats", json!({}));
    assert!(
        graph_stats["result"]["structuredContent"]["relation_count"]
            .as_i64()
            .unwrap()
            >= 1
    );

    server.stop();
}
