use std::fs;
use std::path::Path;
use std::time::Duration;

use assert_cmd::Command;
use serde_json::{json, Value};

use crate::process_harness::{HarnessLimits, McpHarness};

const MCP_IO_TIMEOUT: Duration = Duration::from_secs(10);
const MCP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn mcp_cmd() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
}

pub fn ensure_packet28d_built() {
    crate::process_harness::ensure_packet28d_built();
}

pub fn write_mcp_message(server: &mut McpHarness, value: &Value) {
    server
        .send_value(value, MCP_IO_TIMEOUT)
        .unwrap_or_else(|error| panic!("failed to write MCP message: {error}"));
}

pub fn read_mcp_message_for_id(server: &mut McpHarness, expected_id: u64) -> Value {
    server
        .recv_for_id(&json!(expected_id), MCP_IO_TIMEOUT)
        .unwrap_or_else(|error| panic!("failed to read MCP response for id {expected_id}: {error}"))
}

pub fn start_mcp_server(root: &Path) -> McpHarness {
    let mut command = mcp_cmd();
    command
        .current_dir(root)
        .args(["mcp", "serve", "--root", root.to_str().unwrap()]);
    McpHarness::spawn(&mut command, HarnessLimits::default())
        .unwrap_or_else(|error| panic!("failed to start MCP server: {error}"))
}

pub fn initialize_mcp_session(server: &mut McpHarness) {
    write_mcp_message(
        server,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}
        }),
    );
    let _ = read_mcp_message_for_id(server, 1);
}

pub fn stop_mcp_server(mut server: McpHarness) {
    server
        .finish(MCP_SHUTDOWN_TIMEOUT)
        .unwrap_or_else(|error| panic!("failed to stop MCP server: {error}"));
}

pub fn write_repo_fixture(root: &Path) {
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
    crate::process_harness::run_git(root, args);
}

pub fn init_repo(root: &Path) {
    git(root, &["init"]);
}
