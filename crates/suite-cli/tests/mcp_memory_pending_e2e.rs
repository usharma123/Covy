mod support;

use serde_json::json;
use support::mcp::{
    initialize_mcp_session, packet28_cmd, packet28_process, read_mcp_message_for_id, spawn_mcp,
    stop_mcp_server, write_mcp_message,
};
use tempfile::TempDir;

#[test]
fn test_mcp_memory_pending_extraction_round_trip() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let mut command = packet28_process();
    command
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["mcp", "serve", "--root", root.path().to_str().unwrap()]);
    let mut server = spawn_mcp(&mut command);
    initialize_mcp_session(&mut server);

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_pending_enqueue",
                "arguments":{
                    "raw_output":"- MCP pending extraction stores durable facts",
                    "project":"mcp-project-b",
                    "tool_name":"Bash"
                }
            }
        }),
    );
    let pending_enqueue = read_mcp_message_for_id(&mut server, 2);
    assert_eq!(
        pending_enqueue["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_pending_stats",
                "arguments":{}
            }
        }),
    );
    let pending_stats = read_mcp_message_for_id(&mut server, 3);
    assert_eq!(
        pending_stats["result"]["structuredContent"]["pending_extraction_count"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_pending_process",
                "arguments":{"limit": 5}
            }
        }),
    );
    let pending_process = read_mcp_message_for_id(&mut server, 4);
    assert_eq!(
        pending_process["result"]["structuredContent"]["extracted_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        pending_process["result"]["structuredContent"]["deleted_count"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_recall",
                "arguments":{"query":"durable facts", "project":"mcp-project-b"}
            }
        }),
    );
    let pending_recall = read_mcp_message_for_id(&mut server, 5);
    assert_eq!(
        pending_recall["result"]["structuredContent"][0]["source"].as_str(),
        Some("pending-extraction:Bash")
    );

    stop_mcp_server(server);
    packet28_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
