use assert_cmd::Command;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::OnceLock;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
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

#[test]
#[cfg(unix)]
fn test_mcp_native_write_intention_derives_task_id_from_full_text() {
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
fn test_mcp_native_tools_return_slim_results_and_fetch_full_artifacts() {
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
