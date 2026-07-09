use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::OnceLock;
use tempfile::TempDir;

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
    write_repo_fixture(root);
    git(root, &["init"]);
}

fn write_mcp_message_newline(stdin: &mut ChildStdin, value: &Value) {
    let body = serde_json::to_vec(value).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
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

#[test]
#[cfg(unix)]
fn test_mcp_native_stdio_accepts_newline_json() {
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
    assert_eq!(initialize["result"]["protocolVersion"], "2025-03-26");
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

    write_mcp_message_newline(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "params":{}
        }),
    );
    let missing_method = read_mcp_message_newline(&mut stdout);
    assert_eq!(missing_method["error"]["code"], -32600);

    write_mcp_message_newline(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"definitely/unknown"
        }),
    );
    let unknown_method = read_mcp_message_newline(&mut stdout);
    assert_eq!(unknown_method["error"]["code"], -32601);

    write_mcp_message_newline(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/list"
        }),
    );
    let after_error = read_mcp_message_newline(&mut stdout);
    assert!(after_error["result"]["tools"].as_array().is_some());

    let _ = child.kill();
}
