use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::OnceLock;
use std::time::Duration;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn agent_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("packet28-agent")
}

fn mcp_cmd() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
}

fn ensure_packet28d_built() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let status = std::process::Command::new("cargo")
            .args(["build", "-p", "packet28d"])
            .status()
            .unwrap();
        assert!(status.success(), "failed to build packet28d");
    });
}

fn fixture(rel: &str) -> String {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    workspace
        .join("tests")
        .join("fixtures")
        .join(rel)
        .to_string_lossy()
        .to_string()
}

fn write_manifest(path: &Path) {
    let line = format!(
        "{{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    std::fs::write(path, line).unwrap();
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

#[test]
#[cfg(unix)]
fn test_packet28_mcp_hypothesis_tools_track_active_assumptions() {
    ensure_packet28d_built();
    let root = TempDir::new().unwrap();
    init_repo(root.path());
    write_repo_fixture(root.path());
    let task_id = "task-mcp-hypothesis";

    let (mut child, mut stdin, mut stdout) = start_mcp_server(root.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"packet28.hypothesis_add",
                "arguments":{
                    "task_id":task_id,
                    "id":"auth-cache",
                    "text":"Auth cache invalidation is the regression source",
                    "paths":["src/auth.rs"],
                    "symbols":["AuthCache"],
                    "artifact_id":"artifact-auth-cache"
                }
            }
        }),
    );
    let added = read_mcp_message_for_id(&mut stdout, 2);
    let added_payload = &added["result"]["structuredContent"];
    assert_eq!(added_payload["id"], "auth-cache");
    assert_eq!(added_payload["status"], "active");
    assert_eq!(added_payload["decision_id"], "hypothesis:auth-cache");

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.hypothesis_list",
                "arguments":{
                    "task_id":task_id
                }
            }
        }),
    );
    let listed = read_mcp_message_for_id(&mut stdout, 3);
    let listed_payload = listed["result"]["structuredContent"].as_array().unwrap();
    assert_eq!(listed_payload.len(), 1);
    assert_eq!(listed_payload[0]["id"], "auth-cache");
    assert_eq!(
        listed_payload[0]["text"],
        "Auth cache invalidation is the regression source"
    );
    assert_eq!(listed_payload[0]["related_paths"][0], "src/auth.rs");
    assert_eq!(listed_payload[0]["related_symbols"][0], "AuthCache");
    assert_eq!(
        listed_payload[0]["related_artifact_ids"][0],
        "artifact-auth-cache"
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.hypothesis_resolve",
                "arguments":{
                    "task_id":task_id,
                    "id":"auth-cache",
                    "status":"rejected"
                }
            }
        }),
    );
    let rejected = read_mcp_message_for_id(&mut stdout, 4);
    assert_eq!(
        rejected["result"]["structuredContent"]["status"],
        "rejected"
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28.hypothesis_list",
                "arguments":{
                    "task_id":task_id
                }
            }
        }),
    );
    let listed_after_reject = read_mcp_message_for_id(&mut stdout, 5);
    assert!(listed_after_reject["result"]["structuredContent"]
        .as_array()
        .unwrap()
        .is_empty());

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}

fn write_mcp_message(stdin: &mut ChildStdin, value: &Value) {
    let body = serde_json::to_vec(value).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.flush().unwrap();
}

fn write_mcp_message_newline(stdin: &mut ChildStdin, value: &Value) {
    let body = serde_json::to_vec(value).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.write_all(b"\n").unwrap();
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
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
    }
    let mut body = vec![0_u8; content_length.unwrap()];
    stdout.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn read_mcp_message_newline(stdout: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    loop {
        line.clear();
        stdout.read_line(&mut line).unwrap();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        return serde_json::from_str(trimmed).unwrap();
    }
}

fn read_mcp_message_for_id(stdout: &mut BufReader<ChildStdout>, expected_id: u64) -> Value {
    loop {
        let value = read_mcp_message(stdout);
        if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
            return value;
        }
    }
}

fn start_mcp_server(root: &Path) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = mcp_cmd()
        .current_dir(root)
        .args(["mcp", "serve", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (child, stdin, stdout)
}

fn start_mcp_proxy_server(
    root: &Path,
    config_path: &Path,
    task_id: &str,
) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = mcp_cmd()
        .current_dir(root)
        .args([
            "mcp",
            "proxy",
            "--root",
            root.to_str().unwrap(),
            "--upstream-config",
            config_path.to_str().unwrap(),
            "--task-id",
            task_id,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (child, stdin, stdout)
}

fn start_mcp_proxy_server_with_tool(
    root: &Path,
    config_path: &Path,
    task_id: &str,
    tool_name: &str,
) -> (Child, ChildStdin, BufReader<ChildStdout>, Value) {
    for _ in 0..3 {
        let (mut child, mut stdin, mut stdout) = start_mcp_proxy_server(root, config_path, task_id);
        initialize_mcp_session(&mut stdin, &mut stdout);
        write_mcp_message(
            &mut stdin,
            &json!({
                "jsonrpc":"2.0",
                "id":2,
                "method":"tools/list"
            }),
        );
        let tools = read_mcp_message_for_id(&mut stdout, 2);
        let has_tool = tools["result"]["tools"]
            .as_array()
            .is_some_and(|items| items.iter().any(|tool| tool["name"] == tool_name));
        if has_tool {
            return (child, stdin, stdout, tools);
        }
        let _ = child.kill();
        let _ = child.wait();
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("proxy tool catalog never exposed required tool '{tool_name}'");
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

fn workspace_packet28_version() -> String {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = workspace.parent().unwrap().parent().unwrap();
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let value: toml::Value = toml::from_str(&manifest).unwrap();
    value["workspace"]["package"]["version"]
        .as_str()
        .unwrap()
        .to_string()
}

fn write_intention_via_mcp(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    id: u64,
    task_id: &str,
    text: &str,
    step_id: &str,
    paths: &[&str],
) -> Value {
    write_mcp_message(
        stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":"tools/call",
            "params":{
                "name":"packet28.write_intention",
                "arguments":{
                    "task_id":task_id,
                    "text":text,
                    "step_id":step_id,
                    "paths":paths,
                }
            }
        }),
    );
    read_mcp_message_for_id(stdout, id)
}

fn run_claude_hook(root: &Path, payload: &Value) -> (i32, String) {
    let (status, stdout, _) =
        run_hook_raw("claude", root, &serde_json::to_string(payload).unwrap());
    (status, stdout)
}

fn run_hook_raw(runtime: &str, root: &Path, stdin_payload: &str) -> (i32, String, String) {
    run_hook_raw_with_env(runtime, root, stdin_payload, &[])
}

fn run_hook_raw_with_env(
    runtime: &str,
    root: &Path,
    stdin_payload: &str,
    envs: &[(&str, &std::ffi::OsStr)],
) -> (i32, String, String) {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    command
        .current_dir(root)
        .args(["hook", runtime, "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_payload.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn write_governed_context(path: &Path) {
    fs::write(
        path,
        r#"
version: 1
policy:
  tools:
    allowlist: ["diffy", "testy", "stacky", "buildy", "contextq"]
  reducers:
    allowlist: ["analyze", "impact", "slice", "reduce", "assemble", "contextq.assemble", "diffy.analyze", "testy.impact", "stacky.slice", "buildy.reduce", "governed.assemble"]
  paths:
    include: ["**"]
    exclude: []
  token_budget:
    cap: 5000
  runtime_budget:
    cap_ms: 5000
  tool_call_budget:
    cap: 10
  redaction:
    forbidden_patterns: []
  human_review:
    required: false
    on_policy_violation: true
    on_budget_violation: true
    on_redaction_violation: true
    paths: []
"#,
    )
    .unwrap();
}

fn write_context_packet(path: &Path, packet_id: &str, title: &str, body: &str, path_ref: &str) {
    fs::write(
        path,
        format!(
            r#"{{
  "packet_id": "{packet_id}",
  "tool": "{packet_id}",
  "reducer": "reduce",
  "paths": ["{path_ref}"],
  "sections": [
    {{
      "title": "{title}",
      "body": "{body}",
      "refs": [{{ "kind": "file", "value": "{path_ref}" }}],
      "relevance": 0.9
    }}
  ]
}}"#
        ),
    )
    .unwrap();
}

fn write_packet_value(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

fn write_stack_log(path: &Path) {
    fs::write(
        path,
        r#"
java.lang.IllegalStateException: boom
  at com.example.Service.run(src/service.rs:42)
  at com.example.Main.main(src/main.rs:10)

java.lang.IllegalStateException: boom
  at com.example.Service.run(src/service.rs:42)
  at com.example.Main.main(src/main.rs:10)
"#,
    )
    .unwrap();
}

fn write_build_log(path: &Path) {
    fs::write(
        path,
        r#"
src/lib.rs:10:5: error: cannot find value `x` in this scope [E0425]
src/lib.rs:10:5: error: cannot find value `x` in this scope [E0425]
main.c(40,2): warning C4996: use of deprecated function
"#,
    )
    .unwrap();
}

fn write_repo_fixture(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("alpha.rs"),
        r#"
use crate::beta::Beta;

fn alpha() {}
struct Alpha;
"#,
    )
    .unwrap();
    fs::write(
        src.join("beta.rs"),
        r#"
fn beta() {}
enum Beta {
  A,
}
"#,
    )
    .unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed with {status}", args);
}

fn init_repo(root: &Path) {
    git(root, &["init"]);
}

fn parse_packet_wrapper(output: &[u8], packet_type: &str) -> Value {
    let value: Value = serde_json::from_slice(output).unwrap();
    assert_eq!(
        value.get("schema_version").and_then(Value::as_str),
        Some("suite.packet.v1")
    );
    assert_eq!(
        value.get("packet_type").and_then(Value::as_str),
        Some(packet_type)
    );
    assert!(value.get("packet").is_some());
    value
}

fn packet_payload(wrapper: &Value) -> &Value {
    wrapper
        .get("packet")
        .and_then(|packet| packet.get("payload"))
        .expect("packet.payload should exist")
}

fn write_cached_coverage_state(root: &Path) {
    let mut coverage = suite_packet_core::CoverageData::new();
    let mut file = suite_packet_core::FileCoverage::new();
    file.lines_instrumented.insert(1);
    file.lines_covered.insert(1);
    coverage.files.insert("src/alpha.rs".to_string(), file);
    let bytes = suite_foundation_core::cache::serialize_coverage(&coverage).unwrap();
    let state_dir = root.join(".covy").join("state");
    fs::create_dir_all(&state_dir).unwrap();
    fs::write(state_dir.join("latest.bin"), bytes).unwrap();
}

fn write_cached_testmap_state(root: &Path) {
    let mut index = suite_packet_core::TestMapIndex::default();
    index.file_to_tests.insert(
        "src/alpha.rs".to_string(),
        ["tests/alpha_test.rs".to_string()].into_iter().collect(),
    );
    let state_dir = root.join(".covy").join("state");
    fs::create_dir_all(&state_dir).unwrap();
    testy_core::pipeline_testmap::write_testmap(&state_dir.join("testmap.bin"), &index).unwrap();
}

fn write_state_event(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
}

#[test]
fn test_suite_governed_local_workflow_smoke() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    let context = dir.path().join("context.yaml");
    let packet_a = dir.path().join("a.json");
    let packet_b = dir.path().join("b.json");

    write_manifest(&manifest);
    write_governed_context(&context);
    write_context_packet(
        &packet_a,
        "diffy",
        "Diff gate",
        "critical regression in coverage",
        "src/lib.rs",
    );
    write_context_packet(
        &packet_b,
        "impact",
        "Impact plan",
        "selected tests for src/lib.rs",
        "src/lib.rs",
    );

    suite_cmd()
        .args(["guard", "validate", "--config", context.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"valid\": true"));

    suite_cmd()
        .args([
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"governed_packet\""))
        .stdout(predicate::str::contains("\"kernel_audit\""));

    suite_cmd()
        .args([
            "test",
            "map",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success();

    suite_cmd()
        .args([
            "test",
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"governed_packet\""))
        .stdout(predicate::str::contains("\"kernel_audit\""));

    let output = suite_cmd()
        .args([
            "context",
            "assemble",
            "--packet",
            packet_a.to_str().unwrap(),
            "--packet",
            packet_b.to_str().unwrap(),
            "--budget-tokens",
            "1200",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.context.assemble.v1");
    assert_eq!(
        value
            .get("packet")
            .and_then(|packet| packet.get("tool"))
            .and_then(Value::as_str),
        Some("contextq")
    );
    assert!(packet_payload(&value).get("assembly").is_some());
}

#[cfg(unix)]
fn seed_checkpointed_handoff_task(
    dir: &Path,
    task_id: &str,
    intention_text: &str,
    _checkpoint_id: &str,
) {
    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir);
    initialize_mcp_session(&mut stdin, &mut stdout);
    let _ = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        2,
        task_id,
        intention_text,
        "investigating",
        &["src/alpha.rs"],
    );
    let _ = child.kill();
    let _ = child.wait();
    let (status, _) = run_claude_hook(
        dir,
        &json!({
            "hook_event_name":"Stop",
            "task_id":task_id,
            "session_id": format!("session-{task_id}"),
        }),
    );
    assert_eq!(status, 0);
}

#[test]
#[cfg(unix)]
fn test_packet28_agent_bootstraps_broker_session() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    let task_text = "design auth broker";
    let task_id = suite_cli::broker_client::derive_task_id(task_text);

    let output = agent_cmd()
        .current_dir(dir.path())
        .args([
            "--task",
            task_text,
            "--",
            "sh",
            "-c",
            "printf '%s\n%s\n%s\n%s\n%s\n%s\n%s\n%s\n' \"$PACKET28_BOOTSTRAP_MODE\" \"$PACKET28_TASK_ID\" \"$PACKET28_BROKER_BRIEF_PATH\" \"$PACKET28_BROKER_STATE_PATH\" \"$PACKET28_MCP_COMMAND\" \"$PACKET28_BROKER_WINDOW_MODE\" \"$PACKET28_BROKER_SUPERSESSION\" \"$PACKET28_BROKER_PREPARE_HANDOFF_TOOL\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let lines = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 8);
    assert_eq!(lines[0], "fresh");
    assert_eq!(lines[1], task_id);
    assert!(Path::new(&lines[2]).exists(), "brief path should exist");
    assert!(Path::new(&lines[3]).exists(), "state path should exist");
    assert!(lines[4].contains("Packet28 mcp serve --root"));
    assert_eq!(lines[5], "replace");
    assert_eq!(lines[6], "1");
    assert_eq!(lines[7], "packet28.prepare_handoff");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_agent_resumes_from_checkpoint_handoff() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-handoff-agent",
        "Resume from checkpointed Alpha investigation",
        "cp-agent-1",
    );

    let output = agent_cmd()
        .current_dir(dir.path())
        .args([
            "--wait-for-handoff",
            "--handoff-timeout-secs",
            "5",
            "--task-id",
            "task-handoff-agent",
            "--",
            "sh",
            "-c",
            "printf '%s\n%s\n%s\n%s\n%s\n%s\n' \"$PACKET28_BOOTSTRAP_MODE\" \"$PACKET28_BOOTSTRAP_PATH\" \"$PACKET28_HANDOFF_PATH\" \"$PACKET28_HANDOFF_ARTIFACT_ID\" \"$PACKET28_HANDOFF_CHECKPOINT_ID\" \"$PACKET28_BROKER_PREPARE_HANDOFF_TOOL\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let lines = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 6);
    assert_eq!(lines[0], "handoff");
    assert!(Path::new(&lines[1]).exists(), "bootstrap path should exist");
    assert!(Path::new(&lines[2]).exists(), "handoff path should exist");
    assert!(
        !lines[3].is_empty(),
        "handoff artifact id should be exported"
    );
    assert!(lines[4].is_empty());
    assert_eq!(lines[5], "packet28.prepare_handoff");

    let bootstrap: Value = serde_json::from_str(&fs::read_to_string(&lines[1]).unwrap()).unwrap();
    assert_eq!(
        bootstrap["latest_intention"]["text"],
        "Resume from checkpointed Alpha investigation"
    );
    assert_eq!(bootstrap["response_mode"], "full");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_agent_wait_for_handoff_times_out_when_checkpoint_missing() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    agent_cmd()
        .current_dir(dir.path())
        .args([
            "--wait-for-handoff",
            "--handoff-timeout-secs",
            "1",
            "--handoff-poll-ms",
            "50",
            "--task-id",
            "task-timeout-handoff",
            "--",
            "sh",
            "-c",
            "exit 0",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "timed out waiting for Packet28 handoff",
        ));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_daemon_task_await_handoff_reports_ready_status() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-await",
        "Prepare daemon-owned handoff wait",
        "cp-daemon-1",
    );

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-await",
            "--timeout-ms",
            "1000",
            "--poll-ms",
            "50",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["task_status"]["handoff_ready"], true);
    assert!(value["waited_ms"].as_u64().unwrap() <= 1_000);
    assert!(value["polls"].as_u64().unwrap() >= 1);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_daemon_task_launch_agent_spawns_child_from_handoff() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-launch",
        "Launch fresh worker from daemon",
        "cp-daemon-launch-1",
    );

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "launch-agent",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-launch",
            "--json",
            "--",
            "sh",
            "-c",
            "printf '%s\\n%s\\n' \"$PACKET28_BOOTSTRAP_MODE\" \"$PACKET28_TASK_ID\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launch_value: Value = serde_json::from_slice(&output).unwrap();
    let log_path = launch_value["log_path"].as_str().unwrap();
    assert_eq!(launch_value["bootstrap_mode"], "handoff");
    assert!(launch_value["pid"].as_u64().unwrap() > 0);

    let mut log_contents = String::new();
    for _ in 0..40 {
        if let Ok(raw) = fs::read_to_string(log_path) {
            log_contents = raw;
            if log_contents.contains("handoff") && log_contents.contains("task-daemon-launch") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(log_contents.contains("handoff"));
    assert!(log_contents.contains("task-daemon-launch"));

    let status_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-launch",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_value: Value = serde_json::from_slice(&status_output).unwrap();
    assert_eq!(status_value["latest_agent_bootstrap_mode"], "handoff");
    assert_eq!(
        status_value["latest_agent_pid"].as_u64().unwrap(),
        launch_value["pid"].as_u64().unwrap()
    );
    assert_eq!(status_value["latest_agent_log_path"], log_path);
    assert_eq!(
        status_value["latest_agent_handoff_artifact_id"],
        launch_value["handoff_artifact_id"]
    );
    assert_eq!(
        status_value["latest_agent_handoff_checkpoint_id"],
        launch_value["handoff_checkpoint_id"]
    );
    assert!(status_value["latest_agent_context_version"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_daemon_task_await_handoff_can_require_newer_context_version() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-newer-handoff",
        "Prepare initial handoff",
        "cp-daemon-newer-1",
    );
    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    let launch_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "launch-agent",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--json",
            "--",
            "sh",
            "-c",
            "printf '%s\\n' \"$PACKET28_BOOTSTRAP_MODE\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launch_value: Value = serde_json::from_slice(&launch_output).unwrap();
    let launched_context_version = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launched_status: Value = serde_json::from_slice(&launched_context_version).unwrap();
    let previous_context_version = launched_status["latest_agent_context_version"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(launch_value["bootstrap_mode"], "handoff");

    suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--after-context-version",
            &previous_context_version,
            "--timeout-ms",
            "100",
            "--poll-ms",
            "20",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "newer handoff than context version",
        ));

    let _ = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        4,
        "task-daemon-newer-handoff",
        "Resume from a newer handoff",
        "editing",
        &["src/beta.rs"],
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreCompact",
            "task_id":"task-daemon-newer-handoff",
            "session_id":"session-daemon-newer-handoff",
        }),
    );
    assert_eq!(status, 0);

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--after-context-version",
            &previous_context_version,
            "--timeout-ms",
            "1000",
            "--poll-ms",
            "50",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["task_status"]["handoff_ready"], true);
    assert_ne!(
        value["task_status"]["latest_context_version"]
            .as_str()
            .unwrap(),
        previous_context_version
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_doctor_reports_healthy_stack() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    git(dir.path(), &["add", "src/alpha.rs", "src/beta.rs"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "init",
        ],
    );
    write_cached_coverage_state(dir.path());
    write_cached_testmap_state(dir.path());
    fs::write(
        dir.path().join(".mcp.json"),
        json!({
            "mcpServers": {
                "packet28": {
                    "command": "packet28-mcp",
                    "args": ["--root", dir.path().to_str().unwrap()]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    for _ in 0..2 {
        let output = suite_cmd()
            .current_dir(dir.path())
            .args(["doctor", "--root", dir.path().to_str().unwrap(), "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let payload: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(payload["daemon"]["ok"], true);
        assert_eq!(payload["index"]["ok"], true);
        assert!(payload["mcp_config"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["packet28_configured"] == true));
        assert_eq!(payload["handshake"]["ok"], true);
        assert_eq!(payload["reducer_round_trip"]["ok"], true);
        assert!(payload.get("push_notifications").is_some());
        assert_eq!(payload["handoff_round_trip"]["ok"], true);
        assert!(payload["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["name"] == "experiment_manifest"));
    }

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_prepare_handoff_requires_checkpoint_and_persists_artifact() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());

    initialize_mcp_session(&mut stdin, &mut stdout);
    let intention = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        2,
        "task-handoff",
        "Inspect Alpha before editing it",
        "investigating",
        &["src/alpha.rs"],
    );
    assert_eq!(intention["result"]["structuredContent"]["accepted"], true);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.prepare_handoff",
                "arguments":{
                    "task_id":"task-handoff"
                }
            }
        }),
    );
    let not_ready = read_mcp_message_for_id(&mut stdout, 3);
    let not_ready_payload = &not_ready["result"]["structuredContent"];
    assert_eq!(not_ready_payload["handoff_ready"], false);
    assert!(not_ready_payload["context"].is_null());

    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"Stop",
            "task_id":"task-handoff",
            "session_id":"session-task-handoff",
        }),
    );
    assert_eq!(status, 0);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.prepare_handoff",
                "arguments":{
                    "task_id":"task-handoff",
                    "response_mode":"slim"
                }
            }
        }),
    );
    let handoff = read_mcp_message_for_id(&mut stdout, 4);
    let handoff_payload = &handoff["result"]["structuredContent"];
    assert_eq!(handoff_payload["handoff_ready"], true);
    assert!(handoff_payload["latest_checkpoint_id"].is_null());
    assert_eq!(
        handoff_payload["latest_intention"]["text"],
        "Inspect Alpha before editing it"
    );
    let handoff_context = &handoff_payload["context"];
    assert_eq!(handoff_context["response_mode"], "slim");
    assert_eq!(handoff_context["handoff_ready"], true);
    assert!(handoff_context["brief"]
        .as_str()
        .unwrap()
        .contains("Latest Intention"));
    let handoff_artifact_id = handoff_context["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28.fetch_context",
                "arguments":{
                    "task_id":"task-handoff",
                    "artifact_id": handoff_artifact_id
                }
            }
        }),
    );
    let fetched = read_mcp_message_for_id(&mut stdout, 5);
    let fetched_payload = &fetched["result"]["structuredContent"];
    assert_eq!(fetched_payload["response_mode"], "full");
    assert_eq!(
        fetched_payload["latest_intention"]["step_id"],
        "investigating"
    );
    assert!(fetched_payload["sections"]
        .as_array()
        .unwrap()
        .iter()
        .any(|section| section["id"] == "agent_intention"));

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"packet28.task_status",
                "arguments":{
                    "task_id":"task-handoff"
                }
            }
        }),
    );
    let status = read_mcp_message_for_id(&mut stdout, 6);
    let status_payload = &status["result"]["structuredContent"];
    assert_eq!(status_payload["handoff_ready"], true);
    assert!(status_payload["latest_handoff_checkpoint_id"].is_null());
    assert_eq!(
        status_payload["latest_handoff_artifact_id"],
        handoff_context["artifact_id"]
    );

    let (resume_status, resume_output) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"SessionStart",
            "task_id":"task-handoff",
            "session_id":"session-task-handoff-resume",
            "cwd": dir.path().display().to_string(),
        }),
    );
    assert_eq!(resume_status, 0);
    let resume_payload: Value = serde_json::from_str(&resume_output).unwrap();
    let additional_context = resume_payload["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(additional_context.contains("Packet28 Context v"));
    assert!(additional_context.contains("Latest Intention"));

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_session_start_injects_wakeup_pack() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    let project = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Session start should inject this Packet28 wakeup fact",
            "--project",
            &project,
            "--topic",
            "session-start",
            "--importance",
            "critical",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Session start has a second Packet28 wakeup fact that proves budgeted hook packs truncate deterministically",
            "--project",
            &project,
            "--topic",
            "session-start",
            "--importance",
            "high",
            "--json",
        ])
        .assert()
        .success();

    let payload = json!({
        "hook_event_name":"SessionStart",
        "task_id":"task-wakeup-hook",
        "session_id":"session-wakeup-hook",
        "cwd": dir.path().display().to_string(),
    });
    let (status, stdout, stderr) = run_hook_raw_with_env(
        "claude",
        dir.path(),
        &serde_json::to_string(&payload).unwrap(),
        &[("HOME", home.path().as_os_str())],
    );
    assert_eq!(status, 0, "stderr={stderr}");
    let rendered: Value = serde_json::from_str(&stdout).unwrap();
    let additional_context = rendered["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(additional_context.contains("Packet28 Wake-Up Pack"));
    assert!(additional_context.contains("Session start should inject this Packet28 wakeup fact"));
    assert!(additional_context.contains("Critical memories"));
    let (budget_status, budget_stdout, budget_stderr) = run_hook_raw_with_env(
        "claude",
        dir.path(),
        &serde_json::to_string(&payload).unwrap(),
        &[
            ("HOME", home.path().as_os_str()),
            ("PACKET28_HOOK_WAKEUP_TOKENS", std::ffi::OsStr::new("12")),
        ],
    );
    assert_eq!(budget_status, 0, "stderr={budget_stderr}");
    let budget_rendered: Value = serde_json::from_str(&budget_stdout).unwrap();
    let budget_context = budget_rendered["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(budget_context.contains("Packet28 Wake-Up Pack"));
    assert!(budget_context.contains("budget:"));
    assert!(budget_context.contains("truncated"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_write_intention_derives_task_id_from_full_text() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    let intention_text = "Investigate parser regression in the handoff pipeline";
    let derived_task_id = suite_cli::broker_client::derive_task_id(intention_text);
    let response = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        2,
        "",
        intention_text,
        "investigating",
        &["crates/packet28d/src/hooks.rs"],
    );
    assert_eq!(response["result"]["structuredContent"]["accepted"], true);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.task_status",
                "arguments":{
                    "task_id": derived_task_id
                }
            }
        }),
    );
    let status = read_mcp_message_for_id(&mut stdout, 3);
    assert_eq!(
        status["result"]["structuredContent"]["task"]["task_id"],
        derived_task_id
    );

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_native_read_auto_captures_regions() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    git(dir.path(), &["add", "src/alpha.rs", "src/beta.rs"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "init",
        ],
    );
    write_cached_coverage_state(dir.path());
    write_cached_testmap_state(dir.path());

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);
    let _ = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        2,
        "task-native-read",
        "Locate the Alpha definition",
        "investigating",
        &["src/alpha.rs"],
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PostToolUse",
            "task_id":"task-native-read",
            "session_id":"session-native-read",
            "tool_name":"Read",
            "tool_input":{"file_path":"src/alpha.rs","offset":4,"limit":1},
            "tool_response":{"content":"fn alpha() {}\nstruct Alpha;\n","symbols":["Alpha"],"regions":["src/alpha.rs:4-5"]}
        }),
    );
    assert_eq!(status, 0);
    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"Stop",
            "task_id":"task-native-read",
            "session_id":"session-native-read",
        }),
    );
    assert_eq!(status, 0);

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.prepare_handoff",
                "arguments":{
                    "task_id":"task-native-read",
                    "query":"Where is Alpha defined?",
                    "response_mode":"full"
                }
            }
        }),
    );
    let inspect = read_mcp_message_for_id(&mut stdout, 3);
    let inspect_payload = &inspect["result"]["structuredContent"]["context"];
    assert!(inspect["result"]["structuredContent"]["handoff_ready"]
        .as_bool()
        .unwrap());
    assert!(inspect_payload["recent_tool_invocations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["tool_name"] == "Read"
                && item["regions"].as_array().is_some_and(|regions| {
                    regions.iter().any(|region| region == "src/alpha.rs:4-5")
                })
        }));
    assert!(inspect_payload["discovered_paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item == "src/alpha.rs"));
    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_native_tools_return_slim_results_and_fetch_full_artifacts() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"packet28_search",
                "arguments":{
                    "task_id":"task-native-tools",
                    "query":"Alpha",
                    "response_mode":"slim"
                }
            }
        }),
    );
    let search = read_mcp_message_for_id(&mut stdout, 2);
    let search_payload = &search["result"]["structuredContent"];
    assert_eq!(search_payload["response_mode"], "slim");
    assert!(search_payload["artifact_id"].as_str().is_some());
    assert!(search_payload["match_count"].as_u64().unwrap() >= 1);
    assert_eq!(search_payload["search_strategy"], "hybrid");
    assert!(search_payload["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == "src/alpha.rs"));
    assert!(search_payload["regions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|region| region
            .as_str()
            .is_some_and(|value| value.starts_with("src/alpha.rs:"))));
    assert!(search_payload["engine"].is_object());
    assert!(search_payload["hybrid"].is_object());
    let search_artifact = search_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28_fetch_tool_result",
                "arguments":{
                    "task_id":"task-native-tools",
                    "artifact_id": search_artifact
                }
            }
        }),
    );
    let search_full = read_mcp_message_for_id(&mut stdout, 3);
    let search_full_payload = &search_full["result"]["structuredContent"];
    assert_eq!(search_full_payload["response_mode"], "full");
    assert_eq!(search_full_payload["query"], "Alpha");
    assert_eq!(search_full_payload["search_strategy"], "hybrid");
    assert_eq!(search_full_payload["content_format"], "path:line:text");
    assert!(search_full_payload["groups"].is_null());
    assert!(search_full_payload["content"]
        .as_str()
        .is_some_and(|content| content.contains("src/alpha.rs:")));
    assert!(search_full_payload["engine"].is_object());
    assert!(search_full_payload["hybrid"].is_object());

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28_read_regions",
                "arguments":{
                    "task_id":"task-native-tools",
                    "path":"src/alpha.rs",
                    "line_start":1,
                    "line_end":2,
                    "response_mode":"slim"
                }
            }
        }),
    );
    let read_regions = read_mcp_message_for_id(&mut stdout, 4);
    let read_payload = &read_regions["result"]["structuredContent"];
    assert_eq!(read_payload["response_mode"], "slim");
    assert!(read_payload["artifact_id"].as_str().is_some());
    let read_artifact = read_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28_fetch_tool_result",
                "arguments":{
                    "task_id":"task-native-tools",
                    "artifact_id": read_artifact
                }
            }
        }),
    );
    let read_full = read_mcp_message_for_id(&mut stdout, 5);
    let read_full_payload = &read_full["result"]["structuredContent"];
    assert_eq!(read_full_payload["response_mode"], "full");
    assert_eq!(read_full_payload["path"], "src/alpha.rs");
    assert_eq!(read_full_payload["line_count"], 2);
    assert!(read_full_payload["content"]
        .as_str()
        .is_some_and(|content| content.contains("2: use crate::beta::Beta;")));

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"packet28_glob",
                "arguments":{
                    "task_id":"task-native-tools",
                    "pattern":"src/*.rs",
                    "response_mode":"slim"
                }
            }
        }),
    );
    let glob = read_mcp_message_for_id(&mut stdout, 6);
    let glob_payload = &glob["result"]["structuredContent"];
    assert_eq!(glob_payload["response_mode"], "slim");
    assert!(glob_payload["artifact_id"].as_str().is_some());
    let glob_artifact = glob_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":7,
            "method":"tools/call",
            "params":{
                "name":"packet28_fetch_tool_result",
                "arguments":{
                    "task_id":"task-native-tools",
                    "artifact_id": glob_artifact
                }
            }
        }),
    );
    let glob_full = read_mcp_message_for_id(&mut stdout, 7);
    let glob_full_payload = &glob_full["result"]["structuredContent"];
    assert_eq!(glob_full_payload["response_mode"], "full");
    assert_eq!(glob_full_payload["pattern"], "src/*.rs");
    assert!(glob_full_payload["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == "src/alpha.rs"));

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_doctor_reports_healthy_runtime() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let output = suite_cmd()
        .args(["doctor", "--root", dir.path().to_str().unwrap(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["daemon"]["ok"], true);
    assert_eq!(report["handshake"]["ok"], true);
    assert_eq!(report["reducer_round_trip"]["ok"], true);
    assert_eq!(report["handoff_round_trip"]["ok"], true);
    assert!(report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["name"] == "experiment_manifest"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_reducer_runner_reuses_cached_summary_without_rerunning_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("sample.txt"), "Alpha\nBeta\n").unwrap();

    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let counter_path = dir.path().join("cat-count.txt");
    fs::write(&counter_path, "0\n").unwrap();
    let script_path = bin_dir.join("cat");
    fs::write(
        &script_path,
        format!(
            "#!/bin/sh\ncount=$(/bin/cat \"{count}\" 2>/dev/null || echo 0)\ncount=$((count + 1))\nprintf '%s\\n' \"$count\" > \"{count}\"\nexec /bin/cat \"$@\"\n",
            count = counter_path.display()
        ),
    )
    .unwrap();
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();

    let spec = packet28_reducer_core::classify_command("cat sample.txt").unwrap();
    let path_env = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut first = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    first.current_dir(dir.path()).env("PATH", &path_env).args([
        "hook",
        "reducer-runner",
        "--root",
        dir.path().to_str().unwrap(),
        "--task-id",
        "task-runner-cache",
        "--family",
        &spec.family,
        "--kind",
        &spec.canonical_kind,
        "--fingerprint",
        &spec.cache_fingerprint,
        "--cwd",
        dir.path().to_str().unwrap(),
        "--",
        "cat",
        "sample.txt",
    ]);
    let first = first.output().unwrap();
    assert!(first.status.success());

    let mut second = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    second.current_dir(dir.path()).env("PATH", &path_env).args([
        "hook",
        "reducer-runner",
        "--root",
        dir.path().to_str().unwrap(),
        "--task-id",
        "task-runner-cache",
        "--family",
        &spec.family,
        "--kind",
        &spec.canonical_kind,
        "--fingerprint",
        &spec.cache_fingerprint,
        "--cwd",
        dir.path().to_str().unwrap(),
        "--",
        "cat",
        "sample.txt",
    ]);
    let second = second.output().unwrap();
    assert!(second.status.success());
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(fs::read_to_string(&counter_path).unwrap().trim(), "1");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_reducer_runner_busts_cache_after_out_of_band_file_edit() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("sample.txt"), "Alpha\nBeta\n").unwrap();

    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let counter_path = dir.path().join("cat-count.txt");
    fs::write(&counter_path, "0\n").unwrap();
    let script_path = bin_dir.join("cat");
    fs::write(
        &script_path,
        format!(
            "#!/bin/sh\ncount=$(/bin/cat \"{count}\" 2>/dev/null || echo 0)\ncount=$((count + 1))\nprintf '%s\\n' \"$count\" > \"{count}\"\nexec /bin/cat \"$@\"\n",
            count = counter_path.display()
        ),
    )
    .unwrap();
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();

    let spec = packet28_reducer_core::classify_command("cat sample.txt").unwrap();
    let path_env = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let runner_args = [
        "hook",
        "reducer-runner",
        "--root",
        dir.path().to_str().unwrap(),
        "--task-id",
        "task-runner-stale-cache",
        "--family",
        &spec.family,
        "--kind",
        &spec.canonical_kind,
        "--fingerprint",
        &spec.cache_fingerprint,
        "--cwd",
        dir.path().to_str().unwrap(),
        "--",
        "cat",
        "sample.txt",
    ];

    let mut first = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    first
        .current_dir(dir.path())
        .env("PATH", &path_env)
        .args(runner_args);
    let first = first.output().unwrap();
    assert!(first.status.success());

    fs::write(dir.path().join("sample.txt"), "Alpha\nBeta\nGamma\n").unwrap();

    let mut second = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    second
        .current_dir(dir.path())
        .env("PATH", &path_env)
        .args(runner_args);
    let second = second.output().unwrap();
    assert!(second.status.success());
    assert_ne!(first.stdout, second.stdout);
    assert_eq!(fs::read_to_string(&counter_path).unwrap().trim(), "2");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hooks_degrade_gracefully_on_bad_json_and_no_rewrite() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());

    let (status, stdout, stderr) = run_hook_raw("claude", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout, stderr) = run_hook_raw("cursor", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout, stderr) = run_hook_raw("copilot", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout, stderr) = run_hook_raw("gemini", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-no-rewrite",
            "session_id":"session-pretool-no-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"definitely-unsupported-packet28-tool --flag"}
        }),
    );
    assert_eq!(status, 0);
    assert!(matches!(stdout.trim(), "" | "{}"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_git_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-rewrite",
            "session_id":"session-pretool-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"git status --short src/alpha.rs"}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family git"));
    assert!(rewritten.contains("--kind git_status"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_is_idempotent_and_ignores_non_bash_tools() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let base_payload = json!({
        "hook_event_name":"PreToolUse",
        "task_id":"task-pretool-idempotent",
        "session_id":"session-pretool-idempotent",
        "cwd":dir.path().to_str().unwrap(),
        "tool_name":"Bash",
        "tool_input":{"command":"git status --short src/alpha.rs"}
    });
    let (status, stdout) = run_claude_hook(dir.path(), &base_payload);
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-idempotent",
            "session_id":"session-pretool-idempotent",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command": rewritten}
        }),
    );
    assert_eq!(status, 0);
    assert!(matches!(stdout.trim(), "" | "{}"));

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-non-bash",
            "session_id":"session-pretool-idempotent",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Read",
            "tool_input":{"file_path":"src/alpha.rs"}
        }),
    );
    assert_eq!(status, 0);
    assert!(matches!(stdout.trim(), "" | "{}"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_cursor_hook_pretool_rewrites_and_returns_empty_json_on_noop() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let payloads = [
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "command":"git status --short src/alpha.rs"
        }),
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-tool-input-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"command":"git status --short src/alpha.rs"}
        }),
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-command-line-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "command_line":"git status --short src/alpha.rs"
        }),
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-shell-command-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "shell_command":"git status --short src/alpha.rs"
        }),
    ];
    let mut first_rewritten = String::new();
    for payload in payloads {
        let (status, stdout, _stderr) = run_hook_raw(
            "cursor",
            dir.path(),
            &serde_json::to_string(&payload).unwrap(),
        );
        assert_eq!(status, 0);
        let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(rendered["permission"].as_str(), Some("allow"));
        let rewritten = rendered["updated_input"]["command"].as_str().unwrap();
        assert!(rewritten.contains("hook reducer-runner"));
        assert!(rewritten.contains("--family git"));
        assert!(rewritten.contains("--kind git_status"));
        if first_rewritten.is_empty() {
            first_rewritten = rewritten.to_string();
        }
    }

    let (status, stdout, _stderr) = run_hook_raw(
        "cursor",
        dir.path(),
        &serde_json::to_string(&json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-idempotent",
            "cwd":dir.path().to_str().unwrap(),
            "command":first_rewritten
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    assert_eq!(stdout.trim(), "{}");

    let (status, stdout, _stderr) = run_hook_raw(
        "cursor",
        dir.path(),
        &serde_json::to_string(&json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-noop",
            "cwd":dir.path().to_str().unwrap(),
            "command":"definitely-unsupported-packet28-tool --flag"
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    assert_eq!(stdout.trim(), "{}");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_gemini_hook_before_tool_rewrites_shell_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout, _stderr) = run_hook_raw(
        "gemini",
        dir.path(),
        &serde_json::to_string(&json!({
            "tool_name":"run_shell_command",
            "session_id":"gemini-session-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"command":"git status --short src/alpha.rs"}
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(rendered["decision"].as_str(), Some("allow"));
    let rewritten = rendered["hookSpecificOutput"]["tool_input"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family git"));
    assert!(rewritten.contains("--kind git_status"));

    let (status, stdout, _stderr) = run_hook_raw(
        "gemini",
        dir.path(),
        &serde_json::to_string(&json!({
            "tool_name":"read_file",
            "session_id":"gemini-session-noop",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"path":"src/alpha.rs"}
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(rendered["decision"].as_str(), Some("allow"));
    assert!(rendered.get("hookSpecificOutput").is_none());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_copilot_hook_rewrites_vscode_and_denies_cli_with_suggestion() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout, _stderr) = run_hook_raw(
        "copilot",
        dir.path(),
        &serde_json::to_string(&json!({
            "tool_name":"Bash",
            "session_id":"copilot-vscode-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"command":"git status --short src/alpha.rs"}
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        rendered["hookSpecificOutput"]["permissionDecision"].as_str(),
        Some("allow")
    );
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family git"));
    assert!(rewritten.contains("--kind git_status"));

    let tool_args = serde_json::to_string(&json!({
        "command":"git status --short src/alpha.rs"
    }))
    .unwrap();
    let (status, stdout, _stderr) = run_hook_raw(
        "copilot",
        dir.path(),
        &serde_json::to_string(&json!({
            "toolName":"bash",
            "toolArgs":tool_args
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(rendered["permissionDecision"].as_str(), Some("deny"));
    let reason = rendered["permissionDecisionReason"].as_str().unwrap();
    assert!(reason.contains("hook reducer-runner"));
    assert!(reason.contains("Packet28"));

    let (status, stdout, _stderr) = run_hook_raw(
        "copilot",
        dir.path(),
        &serde_json::to_string(&json!({
            "toolName":"view",
            "toolArgs":"{}"
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_github_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-gh-rewrite",
            "session_id":"session-pretool-gh-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"gh pr list --limit 5"}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family github"));
    assert!(rewritten.contains("--kind gh_pr_list"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_python_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-python-rewrite",
            "session_id":"session-pretool-python-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"python3 -m pytest tests"}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family python"));
    assert!(rewritten.contains("--kind python_pytest"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_javascript_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-js-rewrite",
            "session_id":"session-pretool-js-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"npx tsc --noEmit"}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family javascript"));
    assert!(rewritten.contains("--kind javascript_tsc"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_go_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-go-rewrite",
            "session_id":"session-pretool-go-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"go test ./..."}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family go"));
    assert!(rewritten.contains("--kind go_test"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_infra_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-infra-rewrite",
            "session_id":"session-pretool-infra-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"kubectl get pods"}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family infra"));
    assert!(rewritten.contains("--kind kubectl_get"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_accepts_newline_json_stdio() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());

    write_mcp_message_newline(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"protocolVersion":"2025-11-25","capabilities":{"roots":{}},"clientInfo":{"name":"claude-code","version":"2.1.72"}}
        }),
    );
    let initialize = read_mcp_message_newline(&mut stdout);
    assert_eq!(initialize["result"]["serverInfo"]["name"], "Packet28");
    assert_eq!(
        initialize["result"]["serverInfo"]["version"],
        workspace_packet28_version()
    );
    assert_eq!(initialize["result"]["protocolVersion"], "2024-11-05");
    assert!(initialize["result"]["capabilities"]["experimental"].is_null());

    write_mcp_message_newline(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/list"
        }),
    );
    let tools = read_mcp_message_newline(&mut stdout);
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28_write_intention"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28_search"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28_read_regions"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28_glob"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28_fetch_tool_result"));
    assert!(!tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28_sync"));

    let _ = child.kill();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_proxy_namespaces_colliding_tools() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_alpha = dir.path().join("alpha_mcp.py");
    fs::write(
        &script_alpha,
        r#"import json, sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    params = message.get("params", {})
    if method == "initialize":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}, "resources": {}}, "serverInfo": {"name": "alpha", "version": "1"}}})
    elif method == "tools/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": [{"name": "shared.read", "description": "alpha shared tool", "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "resources/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resources": []}})
    elif method == "resources/templates/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resourceTemplates": []}})
    elif method == "tools/call":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"content": [{"type": "text", "text": "alpha ok"}], "structuredContent": {"owner": "alpha"}}})
    else:
        write_message({"jsonrpc": "2.0", "id": msg_id, "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .unwrap();

    let script_beta = dir.path().join("beta_mcp.py");
    fs::write(
        &script_beta,
        r#"import json, sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    params = message.get("params", {})
    if method == "initialize":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}, "resources": {}}, "serverInfo": {"name": "beta", "version": "1"}}})
    elif method == "tools/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": [{"name": "shared.read", "description": "beta shared tool", "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "resources/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resources": []}})
    elif method == "resources/templates/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resourceTemplates": []}})
    elif method == "tools/call":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"content": [{"type": "text", "text": "beta ok"}], "structuredContent": {"owner": "beta"}}})
    else:
        write_message({"jsonrpc": "2.0", "id": msg_id, "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .unwrap();

    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "alpha": {
                    "command": "python3",
                    "args": ["-u", script_alpha.to_str().unwrap()]
                },
                "beta": {
                    "command": "python3",
                    "args": ["-u", script_beta.to_str().unwrap()]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut child, mut stdin, mut stdout) =
        start_mcp_proxy_server(dir.path(), &config_path, "task-proxy-collision");

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}
        }),
    );
    let _ = read_mcp_message_for_id(&mut stdout, 1);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/list"
        }),
    );
    let tools = read_mcp_message_for_id(&mut stdout, 2);
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "alpha.shared.read"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "beta.shared.read"));

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"beta.shared.read",
                "arguments":{}
            }
        }),
    );
    let response = read_mcp_message_for_id(&mut stdout, 3);
    assert_eq!(
        response["result"]["structuredContent"]["owner"]
            .as_str()
            .unwrap(),
        "beta"
    );

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_proxy_caches_tool_catalog_and_respects_timeout_ms() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let counter_path = dir.path().join("tools-list-count.txt");
    let script_path = dir.path().join("slow_mcp.py");
    fs::write(
        &script_path,
        format!(
            r#"import json, pathlib, sys, time

COUNTER = pathlib.Path({counter:?})

def read_message():
    headers = {{}}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {{len(body)}}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    if method == "initialize":
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"protocolVersion": "2024-11-05", "capabilities": {{"tools": {{}}, "resources": {{}}}}, "serverInfo": {{"name": "slow", "version": "1"}}}}}})
    elif method == "tools/list":
        count = 0
        if COUNTER.exists():
            count = int(COUNTER.read_text() or "0")
        COUNTER.write_text(str(count + 1))
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"tools": [{{"name": "slow.read", "description": "slow tool", "inputSchema": {{"type": "object", "properties": {{}}}}}}]}}}})
    elif method == "resources/list":
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"resources": []}}}})
    elif method == "resources/templates/list":
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"resourceTemplates": []}}}})
    elif method == "tools/call":
        time.sleep(0.2)
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"content": [{{"type": "text", "text": "slow ok"}}]}}}})
    else:
        write_message({{"jsonrpc": "2.0", "id": msg_id, "error": {{"code": -32601, "message": "unknown method"}}}})
"#,
            counter = counter_path,
        ),
    )
    .unwrap();

    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "slow": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "timeout_ms": 50
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut child, mut stdin, mut stdout, tools) = start_mcp_proxy_server_with_tool(
        dir.path(),
        &config_path,
        "task-proxy-timeout",
        "slow.read",
    );
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "slow.read"));
    let catalog_refresh_count = fs::read_to_string(&counter_path)
        .unwrap()
        .trim()
        .parse::<u64>()
        .unwrap();
    assert!(catalog_refresh_count >= 1);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":10,
            "method":"tools/call",
            "params":{
                "name":"slow.read",
                "arguments":{}
            }
        }),
    );
    let timeout = read_mcp_message_for_id(&mut stdout, 10);
    assert!(timeout["error"]["message"]
        .as_str()
        .unwrap()
        .contains("50ms"));
    assert!(timeout["error"]["message"]
        .as_str()
        .unwrap()
        .contains("python3 -u"));
    assert_eq!(
        fs::read_to_string(&counter_path)
            .unwrap()
            .trim()
            .parse::<u64>()
            .unwrap(),
        catalog_refresh_count
    );

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_proxy_compacts_allowlisted_read_tool_results() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("compact_mcp.py");
    fs::write(
        &script_path,
        r#"import json, sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    if method == "initialize":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}, "resources": {}}, "serverInfo": {"name": "compact", "version": "1"}}})
    elif method == "tools/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": [{"name": "compact.read", "description": "compact test tool", "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "resources/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resources": []}})
    elif method == "resources/templates/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resourceTemplates": []}})
    elif method == "tools/call":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"content": [{"type": "text", "text": "Alpha content line 1\nAlpha content line 2"}], "structuredContent": {"path": "src/alpha.rs", "lines": ["pub struct Alpha;", "impl Alpha {}"], "notes": "verbose upstream payload"}}})
    else:
        write_message({"jsonrpc": "2.0", "id": msg_id, "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .unwrap();

    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "compact": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "compact_tools": ["compact.read"]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut child, mut stdin, mut stdout, tools) = start_mcp_proxy_server_with_tool(
        dir.path(),
        &config_path,
        "task-proxy-compact",
        "compact.read",
    );
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "compact.read"));

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"compact.read",
                "arguments":{}
            }
        }),
    );
    let compact = read_mcp_message_for_id(&mut stdout, 2);
    let compact_payload = &compact["result"]["structuredContent"];
    assert_eq!(compact_payload["response_mode"], "slim");
    assert_eq!(compact_payload["original_tool"], "compact.read");
    assert!(compact_payload["artifact_id"].as_str().is_some());
    let artifact_id = compact_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.fetch_tool_result",
                "arguments":{
                    "task_id":"task-proxy-compact",
                    "artifact_id": artifact_id
                }
            }
        }),
    );
    let fetched = read_mcp_message_for_id(&mut stdout, 3);
    let fetched_payload = &fetched["result"]["structuredContent"];
    assert_eq!(fetched_payload["structuredContent"]["path"], "src/alpha.rs");
    assert!(fetched_payload["structuredContent"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line == "pub struct Alpha;"));

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_daemon_suppresses_disconnect_log_noise() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    init_repo(dir.path());

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let status_output = suite_cmd()
        .args([
            "daemon",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: Value = serde_json::from_slice(&status_output).unwrap();
    let socket = PathBuf::from(status.get("socket_path").and_then(Value::as_str).unwrap());
    let start = std::time::Instant::now();
    let mut stream = loop {
        match std::os::unix::net::UnixStream::connect(&socket) {
            Ok(stream) => break stream,
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound
                    && start.elapsed() < std::time::Duration::from_secs(15) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(err) => panic!(
                "failed to connect to daemon socket {}: {err}",
                socket.display()
            ),
        }
    };
    packet28_daemon_core::write_socket_message(
        &mut stream,
        &packet28_daemon_core::DaemonRequest::Status,
    )
    .unwrap();
    drop(stream);

    std::thread::sleep(std::time::Duration::from_millis(300));

    let log_path = dir.path().join(".packet28/daemon/packet28d.log");
    let start = std::time::Instant::now();
    while !log_path.exists() && start.elapsed() < std::time::Duration::from_secs(2) {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let log = fs::read_to_string(&log_path).unwrap();
    assert!(!log.contains("request handling failed: Broken pipe"));
    assert!(!log.contains("request handling failed: Connection reset"));
    assert!(!log.contains("request handling failed: unexpected end of file"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_stack_and_build_via_daemon_emit_packet_wrappers() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let stack_input = dir.path().join("stack.log");
    let build_input = dir.path().join("build.log");
    write_stack_log(&stack_input);
    write_build_log(&build_input);

    let stack_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "stack",
            "slice",
            "--input",
            stack_input.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stack_value = parse_packet_wrapper(&stack_output, "suite.stack.slice.v1");
    assert!(packet_payload(&stack_value)
        .get("failures")
        .and_then(Value::as_array)
        .is_some());

    let build_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "build",
            "reduce",
            "--input",
            build_input.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let build_value = parse_packet_wrapper(&build_output, "suite.build.reduce.v1");
    assert!(packet_payload(&build_value)
        .get("groups")
        .and_then(Value::as_array)
        .is_some());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_context_non_assemble_via_daemon_smoke() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let diff = dir.path().join("diff.json");
    let impact = dir.path().join("impact.json");
    let event = dir.path().join("event.json");
    let packet_a = dir.path().join("a.json");
    let packet_b = dir.path().join("b.json");

    write_packet_value(
        &diff,
        &json!({
            "version": "1",
            "tool": "diffy",
            "kind": "diff_analyze",
            "hash": "diff-hash",
            "summary": "changed StopWatch",
            "files": [{"path": "src/StopWatch.java", "relevance": 1.0}],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["diff"], "generated_at_unix": 1},
            "payload": {
                "gate_result": {"passed": true, "violations": []},
                "diffs": [{"path": "src/StopWatch.java", "old_path": null, "status": "Modified", "changed_lines": [10, 11]}]
            }
        }),
    );
    write_packet_value(
        &impact,
        &json!({
            "version": "1",
            "tool": "testy",
            "kind": "test_impact",
            "hash": "impact-hash",
            "summary": "impact",
            "files": [],
            "symbols": [{"name": "StopWatchTest#testSplit", "kind": "test_id", "relevance": 1.0}],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["testmap.bin"], "generated_at_unix": 1},
            "payload": {
                "result": {
                    "selected_tests": ["StopWatchTest#testSplit"],
                    "smoke_tests": [],
                    "missing_mappings": [],
                    "confidence": 0.9,
                    "stale": false,
                    "escalate_full_suite": false
                },
                "known_tests": 1,
                "print_command": null
            }
        }),
    );
    write_state_event(
        &event,
        r#"{
  "event_id": "evt-1",
  "occurred_at_unix": 1,
  "actor": "tester",
  "kind": "focus_set",
  "paths": ["src/lib.rs"],
  "symbols": [],
  "data": {"type": "focus_set"}
}"#,
    );
    write_context_packet(
        &packet_a,
        "diffy",
        "Diff gate",
        "critical regression in coverage",
        "src/lib.rs",
    );
    write_context_packet(
        &packet_b,
        "testy",
        "Impact plan",
        "selected tests for src/lib.rs",
        "src/lib.rs",
    );

    let correlate_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "correlate",
            "--packet",
            diff.to_str().unwrap(),
            "--packet",
            impact.to_str().unwrap(),
            "--task-id",
            "task-correlation",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let correlate_value = parse_packet_wrapper(&correlate_output, "suite.context.correlate.v1");
    assert!(packet_payload(&correlate_value)
        .get("findings")
        .and_then(Value::as_array)
        .is_some());

    let state_append_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "state",
            "append",
            "--task-id",
            "task-state",
            "--input",
            event.to_str().unwrap(),
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let state_append_value = parse_packet_wrapper(&state_append_output, "suite.agent.state.v1");
    assert_eq!(
        packet_payload(&state_append_value)
            .get("task_id")
            .and_then(Value::as_str),
        Some("task-state")
    );

    let state_snapshot_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "state",
            "snapshot",
            "--task-id",
            "task-state",
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let state_snapshot_value =
        parse_packet_wrapper(&state_snapshot_output, "suite.agent.snapshot.v1");
    assert_eq!(
        packet_payload(&state_snapshot_value)
            .get("task_id")
            .and_then(Value::as_str),
        Some("task-state")
    );

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "assemble",
            "--packet",
            packet_a.to_str().unwrap(),
            "--packet",
            packet_b.to_str().unwrap(),
        ])
        .assert()
        .success();

    let store_list_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "list",
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let store_list_value: Value = serde_json::from_slice(&store_list_output).unwrap();
    let entries = store_list_value
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap();
    assert!(!entries.is_empty());

    let key = entries[0]
        .get("cache_key")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "get",
            "--root",
            ".",
            "--key",
            &key,
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&key));

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "stats",
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"stats\""));

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "recall",
            "--root",
            ".",
            "--query",
            "critical regression",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"query\":\"critical regression\"",
        ));

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "prune",
            "--root",
            ".",
            "--all",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"report\""));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
