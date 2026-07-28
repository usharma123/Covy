#[path = "support/hypothesis.rs"]
mod hypothesis;

use hypothesis::{ensure_packet28d_built, init_repo, suite_cmd, write_repo_fixture};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command as ProcessCommand, Stdio};
use tempfile::TempDir;

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

fn read_mcp_message_for_id(stdout: &mut BufReader<ChildStdout>, expected_id: u64) -> Value {
    loop {
        let value = read_mcp_message(stdout);
        if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
            return value;
        }
    }
}

fn start_mcp_server(root: &Path) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = ProcessCommand::new(env!("CARGO_BIN_EXE_Packet28"))
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
#[cfg(unix)]
fn test_hypothesis_mcp_tools_track_active_assumptions() {
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
