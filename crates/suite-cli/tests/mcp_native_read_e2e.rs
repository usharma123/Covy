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

fn run_claude_hook(root: &Path, payload: &Value) -> (i32, String) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
        .current_dir(root)
        .args(["hook", "claude", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(serde_json::to_string(payload).unwrap().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).to_string(),
    )
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

#[test]
#[cfg(unix)]
fn test_mcp_native_read_auto_captures_regions() {
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
