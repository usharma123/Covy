#[expect(
    dead_code,
    reason = "this protocol fixture uses the shared build and command harness"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

use process_harness::{HarnessLimits, McpHarness};

const MCP_IO_TIMEOUT: Duration = Duration::from_secs(10);
const MCP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

fn mcp_cmd() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
}

fn ensure_packet28d_built() {
    process_harness::ensure_packet28d_built();
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
    process_harness::run_git(root, args);
}

fn init_repo(root: &Path) {
    write_repo_fixture(root);
    git(root, &["init"]);
}

fn write_mcp_message_newline(server: &mut McpHarness, value: &Value) {
    server
        .send_value(value, MCP_IO_TIMEOUT)
        .unwrap_or_else(|error| panic!("failed to write newline MCP message: {error}"));
}

fn read_mcp_message_newline(server: &mut McpHarness) -> Value {
    server
        .receive(MCP_IO_TIMEOUT)
        .unwrap_or_else(|error| panic!("failed to read newline MCP message: {error}"))
}

fn start_mcp_server(root: &Path) -> McpHarness {
    let mut command = mcp_cmd();
    command
        .current_dir(root)
        .args(["mcp", "serve", "--root", root.to_str().unwrap()]);
    McpHarness::spawn_newline_json(&mut command, HarnessLimits::default())
        .unwrap_or_else(|error| panic!("failed to start newline MCP server: {error}"))
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

    let mut server = start_mcp_server(dir.path());

    write_mcp_message_newline(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"protocolVersion":"2025-11-25","capabilities":{"roots":{}},"clientInfo":{"name":"claude-code","version":"2.1.72"}}
        }),
    );
    let initialize = read_mcp_message_newline(&mut server);
    assert_eq!(initialize["result"]["serverInfo"]["name"], "Packet28");
    assert_eq!(
        initialize["result"]["serverInfo"]["version"],
        workspace_packet28_version()
    );
    assert_eq!(initialize["result"]["protocolVersion"], "2025-03-26");
    assert!(initialize["result"]["capabilities"]["experimental"].is_null());

    write_mcp_message_newline(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/list"
        }),
    );
    let tools = read_mcp_message_newline(&mut server);
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
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "params":{}
        }),
    );
    let missing_method = read_mcp_message_newline(&mut server);
    assert_eq!(missing_method["error"]["code"], -32600);

    write_mcp_message_newline(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"definitely/unknown"
        }),
    );
    let unknown_method = read_mcp_message_newline(&mut server);
    assert_eq!(unknown_method["error"]["code"], -32601);

    write_mcp_message_newline(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/list"
        }),
    );
    let after_error = read_mcp_message_newline(&mut server);
    assert!(after_error["result"]["tools"].as_array().is_some());

    server
        .finish(MCP_SHUTDOWN_TIMEOUT)
        .unwrap_or_else(|error| panic!("failed to stop newline MCP server: {error}"));
}
