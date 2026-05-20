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
fn test_mcp_context_feedback_tools_project_scoping() {
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
                    "content":"MCP wakeup memory stays project scoped",
                    "topic":"mcp-context",
                    "project":"mcp-project-b",
                    "importance":"high"
                }
            }
        }),
    );
    let memory = read_mcp_message_for_id(&mut stdout, 2);
    assert_eq!(
        memory["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_record",
                "arguments":{
                    "subject":"mcp",
                    "correction":"store feedback locally",
                    "topic":"mcp-feedback",
                    "context":"MCP feedback context",
                    "predicted":"ignore feedback",
                    "reason":"user correction",
                    "source":"mcp-test",
                    "project":"mcp-project-b"
                }
            }
        }),
    );
    let feedback = read_mcp_message_for_id(&mut stdout, 3);
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

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_search",
                "arguments":{"query":"feedback", "project":"mcp-project-b", "limit": 3}
            }
        }),
    );
    let feedback_search = read_mcp_message_for_id(&mut stdout, 4);
    assert_eq!(
        feedback_search["result"]["structuredContent"][0]["correction"].as_str(),
        Some("store feedback locally")
    );
    assert_eq!(
        feedback_search["result"]["structuredContent"][0]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_list",
                "arguments":{"topic":"mcp-feedback", "limit": 3}
            }
        }),
    );
    let feedback_list = read_mcp_message_for_id(&mut stdout, 5);
    assert_eq!(
        feedback_list["result"]["structuredContent"][0]["topic"].as_str(),
        Some("mcp-feedback")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_apply",
                "arguments":{"id":1}
            }
        }),
    );
    let feedback_apply = read_mcp_message_for_id(&mut stdout, 6);
    assert_eq!(
        feedback_apply["result"]["structuredContent"]["applied_count"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":7,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_stats",
                "arguments":{}
            }
        }),
    );
    let feedback_stats = read_mcp_message_for_id(&mut stdout, 7);
    assert_eq!(
        feedback_stats["result"]["structuredContent"]["feedback_count"].as_i64(),
        Some(1)
    );
    assert_eq!(
        feedback_stats["result"]["structuredContent"]["applied_count"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":8,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_delete",
                "arguments":{"id":1}
            }
        }),
    );
    let feedback_delete = read_mcp_message_for_id(&mut stdout, 8);
    assert_eq!(
        feedback_delete["result"]["structuredContent"]["deleted"].as_u64(),
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
