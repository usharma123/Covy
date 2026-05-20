#[path = "support/mcp_memory.rs"]
mod mcp_memory;
mod support;

use mcp_memory::McpMemoryServer;
use serde_json::json;

#[test]
fn test_mcp_memory_store_recall_uses_sqlite_home_db() {
    let mut server = McpMemoryServer::start("mcp-learn-fixture");

    let stored = server.call_tool(
        2,
        "packet28.memory_store",
        json!({
            "content":"MCP memory survives locally",
            "tags":"mcp",
            "topic":"mcp-topic",
            "importance":"high",
            "keywords":"survives,locally",
            "project":"mcp-project-a",
            "source":"mcp-test",
            "raw_excerpt":"verbatim mcp memory"
        }),
    );
    assert_eq!(
        stored["result"]["structuredContent"]["content"].as_str(),
        Some("MCP memory survives locally")
    );
    assert_eq!(
        stored["result"]["structuredContent"]["topic"].as_str(),
        Some("mcp-topic")
    );
    assert_eq!(
        stored["result"]["structuredContent"]["source"].as_str(),
        Some("mcp-test")
    );
    assert_eq!(
        stored["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-a")
    );

    let recalled = server.call_tool(
        3,
        "packet28.memory_recall",
        json!({"query":"survives", "limit": 3}),
    );
    assert_eq!(
        recalled["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP memory survives locally")
    );

    let listed = server.call_tool(4, "packet28.memory_list", json!({"limit": 3}));
    assert_eq!(
        listed["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP memory survives locally")
    );

    let updated = server.call_tool(
        41,
        "packet28.memory_update",
        json!({"id":1, "content":"MCP memory updated locally", "topic":"mcp-updated", "project":"mcp-project-b", "source":"mcp-update"}),
    );
    assert_eq!(
        updated["result"]["structuredContent"]["content"].as_str(),
        Some("MCP memory updated locally")
    );
    assert_eq!(
        updated["result"]["structuredContent"]["topic"].as_str(),
        Some("mcp-updated")
    );
    assert_eq!(
        updated["result"]["structuredContent"]["source"].as_str(),
        Some("mcp-update")
    );
    assert_eq!(
        updated["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    let topics = server.call_tool(42, "packet28.memory_topics", json!({}));
    assert_eq!(
        topics["result"]["structuredContent"][0]["topic"].as_str(),
        Some("mcp-updated")
    );

    let memory_stats = server.call_tool(43, "packet28.memory_stats", json!({}));
    assert_eq!(
        memory_stats["result"]["structuredContent"]["memory_count"].as_i64(),
        Some(1)
    );

    let filtered_recall = server.call_tool(
        66,
        "packet28.memory_recall",
        json!({
            "query":"updated",
            "topic":"mcp-updated",
            "project":"mcp-project-b",
            "keyword":"survives",
            "limit":3
        }),
    );
    assert_eq!(
        filtered_recall["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP memory updated locally")
    );

    let filtered_list = server.call_tool(
        67,
        "packet28.memory_list",
        json!({"topic":"mcp-updated", "project":"mcp-project-b", "all":true, "sort":"importance"}),
    );
    assert_eq!(
        filtered_list["result"]["structuredContent"][0]["topic"].as_str(),
        Some("mcp-updated")
    );

    let memory_embed = server.call_tool(
        65,
        "packet28.memory_embed",
        json!({"all":true, "dimensions":16}),
    );
    assert_eq!(
        memory_embed["result"]["structuredContent"]["embedded_count"].as_u64(),
        Some(1)
    );

    server.stop();
}
