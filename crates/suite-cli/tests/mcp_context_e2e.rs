#[path = "support/mcp_context.rs"]
mod mcp_context;

mod support;

use mcp_context::McpContextServer;
use serde_json::json;

#[test]
fn test_mcp_context_feedback_tools_project_scoping() {
    let mut server = McpContextServer::start("mcp-learn-fixture");

    let memory = server.call_tool(
        2,
        "packet28.memory_store",
        json!({
            "content":"MCP wakeup memory stays project scoped",
            "topic":"mcp-context",
            "project":"mcp-project-b",
            "importance":"high"
        }),
    );
    assert_eq!(
        memory["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    let feedback = server.call_tool(
        3,
        "packet28.feedback_record",
        json!({
            "subject":"mcp",
            "correction":"store feedback locally",
            "topic":"mcp-feedback",
            "context":"MCP feedback context",
            "predicted":"ignore feedback",
            "reason":"user correction",
            "source":"mcp-test",
            "project":"mcp-project-b"
        }),
    );
    assert_eq!(
        feedback["result"]["structuredContent"]["correction"].as_str(),
        Some("store feedback locally")
    );
    assert_eq!(
        feedback["result"]["structuredContent"]["topic"].as_str(),
        Some("mcp-feedback")
    );
    assert_eq!(
        feedback["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    let feedback_search = server.call_tool(
        4,
        "packet28.feedback_search",
        json!({"query":"feedback", "project":"mcp-project-b", "limit": 3}),
    );
    assert_eq!(
        feedback_search["result"]["structuredContent"][0]["correction"].as_str(),
        Some("store feedback locally")
    );
    assert_eq!(
        feedback_search["result"]["structuredContent"][0]["project"].as_str(),
        Some("mcp-project-b")
    );

    let feedback_list = server.call_tool(
        5,
        "packet28.feedback_list",
        json!({"topic":"mcp-feedback", "limit": 3}),
    );
    assert_eq!(
        feedback_list["result"]["structuredContent"][0]["topic"].as_str(),
        Some("mcp-feedback")
    );

    let feedback_apply = server.call_tool(6, "packet28.feedback_apply", json!({"id":1}));
    assert_eq!(
        feedback_apply["result"]["structuredContent"]["applied_count"].as_i64(),
        Some(1)
    );

    let feedback_stats = server.call_tool(7, "packet28.feedback_stats", json!({}));
    assert_eq!(
        feedback_stats["result"]["structuredContent"]["feedback_count"].as_i64(),
        Some(1)
    );
    assert_eq!(
        feedback_stats["result"]["structuredContent"]["applied_count"].as_i64(),
        Some(1)
    );

    let feedback_delete = server.call_tool(8, "packet28.feedback_delete", json!({"id":1}));
    assert_eq!(
        feedback_delete["result"]["structuredContent"]["deleted"].as_u64(),
        Some(1)
    );

    server.stop();
}
