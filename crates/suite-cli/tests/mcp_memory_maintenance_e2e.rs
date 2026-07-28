#[path = "support/mcp_memory.rs"]
mod mcp_memory;

mod support;

use mcp_memory::McpMemoryServer;
use serde_json::json;

#[test]
fn test_mcp_memory_maintenance_consolidates_forgets_decays_and_prunes() {
    let mut server = McpMemoryServer::start("mcp-memory-maintenance-fixture");

    let _first_memory = server.call_tool(
        2,
        "packet28.memory_store",
        json!({"content":"MCP memory updated locally", "topic":"mcp-updated", "project":"mcp-project-b", "source":"mcp-update"}),
    );
    let _second_memory = server.call_tool(
        3,
        "packet28.memory_store",
        json!({"content":"Second MCP memory before consolidation", "topic":"mcp-updated"}),
    );

    let consolidated = server.call_tool(
        4,
        "packet28.memory_consolidate",
        json!({"topic":"mcp-updated"}),
    );
    assert_eq!(
        consolidated["result"]["structuredContent"]["status"].as_str(),
        Some("consolidated")
    );
    assert_eq!(
        consolidated["result"]["structuredContent"]["source_count"].as_u64(),
        Some(2)
    );

    let health = server.call_tool(
        5,
        "packet28.memory_health",
        json!({"topic":"mcp-updated", "consolidation_threshold": 1}),
    );
    assert_eq!(
        health["result"]["structuredContent"]["topic_filter"].as_str(),
        Some("mcp-updated")
    );
    assert_eq!(
        health["result"]["structuredContent"]["topics_needing_consolidation"].as_i64(),
        Some(1)
    );

    let forgotten = server.call_tool(6, "packet28.memory_forget", json!({"topic":"mcp-updated"}));
    assert_eq!(
        forgotten["result"]["structuredContent"]["deleted"].as_u64(),
        Some(1)
    );

    let _prunable = server.call_tool(
        7,
        "packet28.memory_store",
        json!({"content":"MCP prunable memory", "topic":"mcp-prune", "importance":"low"}),
    );

    let decayed = server.call_tool(8, "packet28.memory_decay", json!({"factor":0.1}));
    assert_eq!(
        decayed["result"]["structuredContent"]["decayed_count"].as_u64(),
        Some(1)
    );

    let prune_preview = server.call_tool(
        9,
        "packet28.memory_prune",
        json!({"threshold":0.5, "dry_run":true}),
    );
    assert_eq!(
        prune_preview["result"]["structuredContent"]["candidate_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        prune_preview["result"]["structuredContent"]["deleted_count"].as_u64(),
        Some(0)
    );

    let pruned = server.call_tool(10, "packet28.memory_prune", json!({"threshold":0.5}));
    assert_eq!(
        pruned["result"]["structuredContent"]["deleted_count"].as_u64(),
        Some(1)
    );

    server.stop();
}
