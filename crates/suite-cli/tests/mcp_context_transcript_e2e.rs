#[path = "support/mcp_context_transcript.rs"]
mod mcp_context_transcript;
mod support;

use mcp_context_transcript::McpContextTranscriptServer;
use serde_json::json;

#[test]
fn test_mcp_context_transcript_wakeup_and_learn_project() {
    let mut server = McpContextTranscriptServer::start();

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

    let wakeup_feedback = server.call_tool(
        3,
        "packet28.feedback_record",
        json!({
            "subject":"mcp wakeup",
            "correction":"wake-up feedback stays project scoped",
            "topic":"mcp-feedback",
            "project":"mcp-project-b"
        }),
    );
    assert_eq!(
        wakeup_feedback["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    let transcript = server.call_tool(
        4,
        "packet28.transcript_append",
        json!({
            "content":"MCP transcript recall should find reducer notes",
            "session":"mcp-session",
            "agent":"codex",
            "role":"assistant",
            "source":"mcp-test",
            "project":"mcp-project-b"
        }),
    );
    assert_eq!(
        transcript["result"]["structuredContent"]["session_key"].as_str(),
        Some("mcp-session")
    );
    assert_eq!(
        transcript["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    let transcript_search = server.call_tool(
        5,
        "packet28.transcript_search",
        json!({"query":"reducer", "project":"mcp-project-b", "limit": 3}),
    );
    assert_eq!(
        transcript_search["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP transcript recall should find reducer notes")
    );
    assert_eq!(
        transcript_search["result"]["structuredContent"][0]["project"].as_str(),
        Some("mcp-project-b")
    );

    let transcript_stats = server.call_tool(6, "packet28.transcript_stats", json!({}));
    assert_eq!(
        transcript_stats["result"]["structuredContent"]["message_count"].as_i64(),
        Some(1)
    );

    let transcript_export = server.call_tool(
        7,
        "packet28.transcript_export",
        json!({"session":"mcp-session"}),
    );
    assert_eq!(
        transcript_export["result"]["structuredContent"]["format"].as_str(),
        Some("packet28.transcript.export")
    );
    assert_eq!(
        transcript_export["result"]["structuredContent"]["messages"][0]["project"].as_str(),
        Some("mcp-project-b")
    );
    let exported_transcript =
        serde_json::to_string(&transcript_export["result"]["structuredContent"]).unwrap();

    let transcript_import = server.call_tool(
        8,
        "packet28.transcript_import",
        json!({"content": exported_transcript}),
    );
    assert_eq!(
        transcript_import["result"]["structuredContent"]["imported_count"].as_u64(),
        Some(1)
    );

    let wakeup = server.call_tool(
        9,
        "packet28.wakeup",
        json!({"project":"mcp-project-b", "limit": 5, "max_tokens": 60, "format":"plain"}),
    );
    assert_eq!(
        wakeup["result"]["structuredContent"]["kind"].as_str(),
        Some("packet28.wakeup.v1")
    );
    assert_eq!(
        wakeup["result"]["structuredContent"]["format"].as_str(),
        Some("plain")
    );
    assert_eq!(
        wakeup["result"]["structuredContent"]["max_tokens"].as_u64(),
        Some(60)
    );
    assert!(wakeup["result"]["structuredContent"]["pack"]
        .as_str()
        .unwrap()
        .contains("mcp-project-b"));
    assert!(!wakeup["result"]["structuredContent"]["transcripts"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        wakeup["result"]["structuredContent"]["transcripts"][0]["project"].as_str(),
        Some("mcp-project-b")
    );
    assert_eq!(
        wakeup["result"]["structuredContent"]["feedback"][0]["project"].as_str(),
        Some("mcp-project-b")
    );
    assert_eq!(
        wakeup["result"]["structuredContent"]["memories"][0]["project"].as_str(),
        Some("mcp-project-b")
    );

    server.stop();
}
