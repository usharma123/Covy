mod support;

use serde_json::json;
use std::fs;
use std::io::BufReader;
use std::process::Stdio;
use support::mcp::{
    initialize_mcp_session, packet28_cmd, packet28_process, read_mcp_message_for_id,
    write_mcp_message,
};
use tempfile::TempDir;

#[test]
fn test_mcp_memory_maintenance_consolidates_forgets_decays_and_prunes() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"mcp-memory-maintenance-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

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
                "name":"packet28.memory_store",
                "arguments":{"content":"MCP memory updated locally", "topic":"mcp-updated", "project":"mcp-project-b", "source":"mcp-update"}
            }
        }),
    );
    let _first_memory = read_mcp_message_for_id(&mut stdout, 2);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_store",
                "arguments":{"content":"Second MCP memory before consolidation", "topic":"mcp-updated"}
            }
        }),
    );
    let _second_memory = read_mcp_message_for_id(&mut stdout, 3);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_consolidate",
                "arguments":{"topic":"mcp-updated"}
            }
        }),
    );
    let consolidated = read_mcp_message_for_id(&mut stdout, 4);
    assert_eq!(
        consolidated["result"]["structuredContent"]["status"].as_str(),
        Some("consolidated")
    );
    assert_eq!(
        consolidated["result"]["structuredContent"]["source_count"].as_u64(),
        Some(2)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_health",
                "arguments":{"topic":"mcp-updated", "consolidation_threshold": 1}
            }
        }),
    );
    let health = read_mcp_message_for_id(&mut stdout, 5);
    assert_eq!(
        health["result"]["structuredContent"]["topic_filter"].as_str(),
        Some("mcp-updated")
    );
    assert_eq!(
        health["result"]["structuredContent"]["topics_needing_consolidation"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_forget",
                "arguments":{"topic":"mcp-updated"}
            }
        }),
    );
    let forgotten = read_mcp_message_for_id(&mut stdout, 6);
    assert_eq!(
        forgotten["result"]["structuredContent"]["deleted"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":7,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_store",
                "arguments":{"content":"MCP prunable memory", "topic":"mcp-prune", "importance":"low"}
            }
        }),
    );
    let _prunable = read_mcp_message_for_id(&mut stdout, 7);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":8,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_decay",
                "arguments":{"factor":0.1}
            }
        }),
    );
    let decayed = read_mcp_message_for_id(&mut stdout, 8);
    assert_eq!(
        decayed["result"]["structuredContent"]["decayed_count"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":9,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_prune",
                "arguments":{"threshold":0.5, "dry_run":true}
            }
        }),
    );
    let prune_preview = read_mcp_message_for_id(&mut stdout, 9);
    assert_eq!(
        prune_preview["result"]["structuredContent"]["candidate_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        prune_preview["result"]["structuredContent"]["deleted_count"].as_u64(),
        Some(0)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":10,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_prune",
                "arguments":{"threshold":0.5}
            }
        }),
    );
    let pruned = read_mcp_message_for_id(&mut stdout, 10);
    assert_eq!(
        pruned["result"]["structuredContent"]["deleted_count"].as_u64(),
        Some(1)
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
