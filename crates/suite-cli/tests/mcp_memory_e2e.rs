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
fn test_mcp_memory_store_recall_uses_sqlite_home_db() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"mcp-learn-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nserde = \"1\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/lib.rs"), "pub fn fixture() {}\n").unwrap();
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
                "arguments":{
                    "content":"MCP memory survives locally",
                    "tags":"mcp",
                    "topic":"mcp-topic",
                    "importance":"high",
                    "keywords":"survives,locally",
                    "project":"mcp-project-a",
                    "source":"mcp-test",
                    "raw_excerpt":"verbatim mcp memory"
                }
            }
        }),
    );
    let stored = read_mcp_message_for_id(&mut stdout, 2);
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

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_recall",
                "arguments":{"query":"survives", "limit": 3}
            }
        }),
    );
    let recalled = read_mcp_message_for_id(&mut stdout, 3);
    assert_eq!(
        recalled["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP memory survives locally")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_list",
                "arguments":{"limit": 3}
            }
        }),
    );
    let listed = read_mcp_message_for_id(&mut stdout, 4);
    assert_eq!(
        listed["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP memory survives locally")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":41,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_update",
                "arguments":{"id":1, "content":"MCP memory updated locally", "topic":"mcp-updated", "project":"mcp-project-b", "source":"mcp-update"}
            }
        }),
    );
    let updated = read_mcp_message_for_id(&mut stdout, 41);
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

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":42,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_topics",
                "arguments":{}
            }
        }),
    );
    let topics = read_mcp_message_for_id(&mut stdout, 42);
    assert_eq!(
        topics["result"]["structuredContent"][0]["topic"].as_str(),
        Some("mcp-updated")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":43,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_stats",
                "arguments":{}
            }
        }),
    );
    let memory_stats = read_mcp_message_for_id(&mut stdout, 43);
    assert_eq!(
        memory_stats["result"]["structuredContent"]["memory_count"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":66,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_recall",
                "arguments":{
                    "query":"updated",
                    "topic":"mcp-updated",
                    "project":"mcp-project-b",
                    "keyword":"survives",
                    "limit":3
                }
            }
        }),
    );
    let filtered_recall = read_mcp_message_for_id(&mut stdout, 66);
    assert_eq!(
        filtered_recall["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP memory updated locally")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":67,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_list",
                "arguments":{"topic":"mcp-updated", "project":"mcp-project-b", "all":true, "sort":"importance"}
            }
        }),
    );
    let filtered_list = read_mcp_message_for_id(&mut stdout, 67);
    assert_eq!(
        filtered_list["result"]["structuredContent"][0]["topic"].as_str(),
        Some("mcp-updated")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":65,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_embed",
                "arguments":{"all":true, "dimensions":16}
            }
        }),
    );
    let memory_embed = read_mcp_message_for_id(&mut stdout, 65);
    assert_eq!(
        memory_embed["result"]["structuredContent"]["embedded_count"].as_u64(),
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
