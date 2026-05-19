use assert_cmd::Command;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{ChildStdin, ChildStdout, Stdio};
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn mcp_cmd() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
}

fn write_mcp_message(stdin: &mut ChildStdin, value: &Value) {
    let body = serde_json::to_vec(value).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.flush().unwrap();
}

fn read_mcp_message(stdout: &mut BufReader<ChildStdout>) -> Value {
    let mut content_length = None::<usize>;
    let mut line = String::new();
    loop {
        line.clear();
        stdout.read_line(&mut line).unwrap();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(":") {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
    }
    let mut body = vec![0_u8; content_length.unwrap()];
    stdout.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn read_mcp_message_for_id(stdout: &mut BufReader<ChildStdout>, expected_id: u64) -> Value {
    loop {
        let value = read_mcp_message(stdout);
        if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
            return value;
        }
    }
}

fn initialize_mcp_session(stdin: &mut ChildStdin, stdout: &mut BufReader<ChildStdout>) {
    write_mcp_message(
        stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}
        }),
    );
    let _ = read_mcp_message_for_id(stdout, 1);
}

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
    let mut child = mcp_cmd()
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

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":46,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_store",
                "arguments":{"content":"Second MCP memory before consolidation", "topic":"mcp-updated"}
            }
        }),
    );
    let _second_memory = read_mcp_message_for_id(&mut stdout, 46);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":47,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_consolidate",
                "arguments":{"topic":"mcp-updated"}
            }
        }),
    );
    let consolidated = read_mcp_message_for_id(&mut stdout, 47);
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
            "id":45,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_health",
                "arguments":{"topic":"mcp-updated", "consolidation_threshold": 1}
            }
        }),
    );
    let health = read_mcp_message_for_id(&mut stdout, 45);
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
            "id":5,
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
    let feedback = read_mcp_message_for_id(&mut stdout, 5);
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
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_search",
                "arguments":{"query":"feedback", "project":"mcp-project-b", "limit": 3}
            }
        }),
    );
    let feedback_search = read_mcp_message_for_id(&mut stdout, 6);
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
            "id":52,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_list",
                "arguments":{"topic":"mcp-feedback", "limit": 3}
            }
        }),
    );
    let feedback_list = read_mcp_message_for_id(&mut stdout, 52);
    assert_eq!(
        feedback_list["result"]["structuredContent"][0]["topic"].as_str(),
        Some("mcp-feedback")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":53,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_apply",
                "arguments":{"id":1}
            }
        }),
    );
    let feedback_apply = read_mcp_message_for_id(&mut stdout, 53);
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
            "id":54,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_delete",
                "arguments":{"id":1}
            }
        }),
    );
    let feedback_delete = read_mcp_message_for_id(&mut stdout, 54);
    assert_eq!(
        feedback_delete["result"]["structuredContent"]["deleted"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":55,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_record",
                "arguments":{
                    "subject":"mcp wakeup",
                    "correction":"wake-up feedback stays project scoped",
                    "topic":"mcp-feedback",
                    "project":"mcp-project-b"
                }
            }
        }),
    );
    let wakeup_feedback = read_mcp_message_for_id(&mut stdout, 55);
    assert_eq!(
        wakeup_feedback["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":60,
            "method":"tools/call",
            "params":{
                "name":"packet28.transcript_append",
                "arguments":{
                    "content":"MCP transcript recall should find reducer notes",
                    "session":"mcp-session",
                    "agent":"codex",
                    "role":"assistant",
                    "source":"mcp-test",
                    "project":"mcp-project-b"
                }
            }
        }),
    );
    let transcript = read_mcp_message_for_id(&mut stdout, 60);
    assert_eq!(
        transcript["result"]["structuredContent"]["session_key"].as_str(),
        Some("mcp-session")
    );
    assert_eq!(
        transcript["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":61,
            "method":"tools/call",
            "params":{
                "name":"packet28.transcript_search",
                "arguments":{"query":"reducer", "project":"mcp-project-b", "limit": 3}
            }
        }),
    );
    let transcript_search = read_mcp_message_for_id(&mut stdout, 61);
    assert_eq!(
        transcript_search["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP transcript recall should find reducer notes")
    );
    assert_eq!(
        transcript_search["result"]["structuredContent"][0]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":62,
            "method":"tools/call",
            "params":{
                "name":"packet28.transcript_stats",
                "arguments":{}
            }
        }),
    );
    let transcript_stats = read_mcp_message_for_id(&mut stdout, 62);
    assert_eq!(
        transcript_stats["result"]["structuredContent"]["message_count"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":64,
            "method":"tools/call",
            "params":{
                "name":"packet28.transcript_export",
                "arguments":{"session":"mcp-session"}
            }
        }),
    );
    let transcript_export = read_mcp_message_for_id(&mut stdout, 64);
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

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":65,
            "method":"tools/call",
            "params":{
                "name":"packet28.transcript_import",
                "arguments":{"content": exported_transcript}
            }
        }),
    );
    let transcript_import = read_mcp_message_for_id(&mut stdout, 65);
    assert_eq!(
        transcript_import["result"]["structuredContent"]["imported_count"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":63,
            "method":"tools/call",
            "params":{
                "name":"packet28.wakeup",
                "arguments":{"project":"mcp-project-b", "limit": 5, "max_tokens": 60, "format":"plain"}
            }
        }),
    );
    let wakeup = read_mcp_message_for_id(&mut stdout, 63);
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

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":68,
            "method":"tools/call",
            "params":{
                "name":"packet28.learn_project",
                "arguments":{"directory":root.path().to_str().unwrap(), "name":"McpLearnFixture", "memoir":"McpLearnMemoir", "limit":5}
            }
        }),
    );
    let learned = read_mcp_message_for_id(&mut stdout, 68);
    assert_eq!(
        learned["result"]["structuredContent"]["project_name"].as_str(),
        Some("McpLearnFixture")
    );
    assert_eq!(
        learned["result"]["structuredContent"]["memoir_name"].as_str(),
        Some("McpLearnMemoir")
    );
    assert!(
        learned["result"]["structuredContent"]["total_concepts"]
            .as_u64()
            .unwrap()
            >= 3
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":54,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_create",
                "arguments":{"name":"McpMemoir", "description":"MCP graph container"}
            }
        }),
    );
    let graph_memoir = read_mcp_message_for_id(&mut stdout, 54);
    assert_eq!(
        graph_memoir["result"]["structuredContent"]["name"].as_str(),
        Some("McpMemoir")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":55,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_add_concept",
                "arguments":{
                    "name":"Packet28",
                    "description":"local context runtime",
                    "memoir":"McpMemoir",
                    "labels":["domain:context"],
                    "confidence":0.91,
                    "source_ids":["memory:mcp"]
                }
            }
        }),
    );
    let graph_concept = read_mcp_message_for_id(&mut stdout, 55);
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

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":56,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_refine",
                "arguments":{"name":"Packet28", "description":"local context runtime with reducers"}
            }
        }),
    );
    let refined = read_mcp_message_for_id(&mut stdout, 56);
    assert_eq!(
        refined["result"]["structuredContent"]["description"].as_str(),
        Some("local context runtime with reducers")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":53,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_add_concept",
                "arguments":{"name":"Reducers", "memoir":"McpMemoir"}
            }
        }),
    );
    let reducer_concept = read_mcp_message_for_id(&mut stdout, 53);
    assert_eq!(
        reducer_concept["result"]["structuredContent"]["memoir_name"].as_str(),
        Some("McpMemoir")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":57,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_link",
                "arguments":{"source":"Packet28", "target":"Reducers", "relation":"uses"}
            }
        }),
    );
    let relation = read_mcp_message_for_id(&mut stdout, 57);
    assert_eq!(
        relation["result"]["structuredContent"]["relation"].as_str(),
        Some("uses")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":58,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_search",
                "arguments":{"query":"context", "memoir":"McpMemoir", "label":"domain:context", "limit": 5}
            }
        }),
    );
    let graph_search = read_mcp_message_for_id(&mut stdout, 58);
    assert!(!graph_search["result"]["structuredContent"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        graph_search["result"]["structuredContent"][0]["memoir_name"].as_str(),
        Some("McpMemoir")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":59,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_export",
                "arguments":{"format":"dot", "limit": 5}
            }
        }),
    );
    let graph_export = read_mcp_message_for_id(&mut stdout, 59);
    assert_eq!(
        graph_export["result"]["structuredContent"]["format"].as_str(),
        Some("dot")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":64,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_stats",
                "arguments":{}
            }
        }),
    );
    let graph_stats = read_mcp_message_for_id(&mut stdout, 64);
    assert!(
        graph_stats["result"]["structuredContent"]["relation_count"]
            .as_i64()
            .unwrap()
            >= 1
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":66,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_show",
                "arguments":{"name":"McpMemoir", "limit": 5}
            }
        }),
    );
    let graph_show = read_mcp_message_for_id(&mut stdout, 66);
    assert_eq!(
        graph_show["result"]["structuredContent"]["memoir"]["name"].as_str(),
        Some("McpMemoir")
    );
    assert_eq!(
        graph_show["result"]["structuredContent"]["concepts"][0]["revision"].as_i64(),
        Some(2)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":8,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_inspect",
                "arguments":{"limit": 5}
            }
        }),
    );
    let graph = read_mcp_message_for_id(&mut stdout, 8);
    assert!(graph["result"]["structuredContent"]["concepts"].is_array());

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":67,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_inspect_concept",
                "arguments":{"name":"Packet28", "memoir":"McpMemoir", "depth": 1}
            }
        }),
    );
    let graph_concept_inspect = read_mcp_message_for_id(&mut stdout, 67);
    assert_eq!(
        graph_concept_inspect["result"]["structuredContent"]["concept"]["name"].as_str(),
        Some("Packet28")
    );
    assert!(
        graph_concept_inspect["result"]["structuredContent"]["neighbors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|concept| concept["name"] == "Reducers")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":69,
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
    let mcp_distill_memory = read_mcp_message_for_id(&mut stdout, 69);
    assert_eq!(
        mcp_distill_memory["result"]["structuredContent"]["topic"].as_str(),
        Some("mcp-distill")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":70,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_distill",
                "arguments":{"from_topic":"mcp-distill", "into":"McpMemoir", "limit": 5}
            }
        }),
    );
    let graph_distill = read_mcp_message_for_id(&mut stdout, 70);
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
        (72, "Pattern extraction should group adapter memories"),
        (
            73,
            "Adapter pattern extraction should create graph concepts",
        ),
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
            "id":74,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_extract_patterns",
                "arguments":{"topic":"mcp-patterns", "memoir":"McpMemoir", "min_cluster_size":2}
            }
        }),
    );
    let memory_patterns = read_mcp_message_for_id(&mut stdout, 74);
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

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":44,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_forget",
                "arguments":{"topic":"mcp-updated"}
            }
        }),
    );
    let forgotten = read_mcp_message_for_id(&mut stdout, 44);
    assert_eq!(
        forgotten["result"]["structuredContent"]["deleted"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":48,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_store",
                "arguments":{"content":"MCP prunable memory", "topic":"mcp-prune", "importance":"low"}
            }
        }),
    );
    let _prunable = read_mcp_message_for_id(&mut stdout, 48);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":49,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_decay",
                "arguments":{"factor":0.1}
            }
        }),
    );
    let decayed = read_mcp_message_for_id(&mut stdout, 49);
    assert_eq!(
        decayed["result"]["structuredContent"]["decayed_count"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":50,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_prune",
                "arguments":{"threshold":0.5, "dry_run":true}
            }
        }),
    );
    let prune_preview = read_mcp_message_for_id(&mut stdout, 50);
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
            "id":51,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_prune",
                "arguments":{"threshold":0.5}
            }
        }),
    );
    let pruned = read_mcp_message_for_id(&mut stdout, 51);
    assert_eq!(
        pruned["result"]["structuredContent"]["deleted_count"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":66,
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
    let pending_enqueue = read_mcp_message_for_id(&mut stdout, 66);
    assert_eq!(
        pending_enqueue["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":69,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_pending_stats",
                "arguments":{}
            }
        }),
    );
    let pending_stats = read_mcp_message_for_id(&mut stdout, 69);
    assert_eq!(
        pending_stats["result"]["structuredContent"]["pending_extraction_count"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":70,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_pending_process",
                "arguments":{"limit": 5}
            }
        }),
    );
    let pending_process = read_mcp_message_for_id(&mut stdout, 70);
    assert_eq!(
        pending_process["result"]["structuredContent"]["extracted_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        pending_process["result"]["structuredContent"]["deleted_count"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":71,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_recall",
                "arguments":{"query":"durable facts", "project":"mcp-project-b"}
            }
        }),
    );
    let pending_recall = read_mcp_message_for_id(&mut stdout, 71);
    assert_eq!(
        pending_recall["result"]["structuredContent"][0]["source"].as_str(),
        Some("pending-extraction:Bash")
    );

    let _ = child.kill();
    let _ = child.wait();
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
