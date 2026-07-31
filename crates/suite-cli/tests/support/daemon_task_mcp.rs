use crate::process_harness::{HarnessLimits, McpHarness, ProcessHarness};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

const MCP_IO_TIMEOUT: Duration = Duration::from_secs(10);
const PROCESS_TIMEOUT: Duration = Duration::from_secs(10);
const MCP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub fn start_mcp_server(root: &Path) -> McpHarness {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    command
        .current_dir(root)
        .args(["mcp", "serve", "--root", root.to_str().unwrap()]);
    McpHarness::spawn(&mut command, HarnessLimits::default())
        .unwrap_or_else(|error| panic!("failed to start MCP server: {error}"))
}

pub fn initialize_mcp_session(server: &mut McpHarness) {
    server
        .request_with_id(
            json!(1),
            "initialize",
            json!({
                "protocolVersion":"2024-11-05",
                "capabilities":{},
                "clientInfo":{"name":"test","version":"1"}
            }),
            MCP_IO_TIMEOUT,
        )
        .unwrap_or_else(|error| panic!("failed to initialize MCP server: {error}"));
}

pub fn write_intention_via_mcp(
    server: &mut McpHarness,
    id: u64,
    task_id: &str,
    text: &str,
    step_id: &str,
    paths: &[&str],
) -> Value {
    server
        .request_with_id(
            json!(id),
            "tools/call",
            json!({
                "name":"packet28.write_intention",
                "arguments":{
                    "task_id":task_id,
                    "text":text,
                    "step_id":step_id,
                    "paths":paths,
                }
            }),
            MCP_IO_TIMEOUT,
        )
        .unwrap_or_else(|error| panic!("failed to write intention through MCP: {error}"))
}

pub fn stop_mcp_server(mut server: McpHarness) {
    server
        .finish(MCP_SHUTDOWN_TIMEOUT)
        .unwrap_or_else(|error| panic!("failed to stop MCP server: {error}"));
}

pub fn run_claude_hook(root: &Path, payload: &Value) -> (i32, String) {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    command
        .current_dir(root)
        .args(["hook", "claude", "--root", root.to_str().unwrap()]);
    let input = serde_json::to_vec(payload).unwrap();
    let output = ProcessHarness::run(
        &mut command,
        &input,
        PROCESS_TIMEOUT,
        HarnessLimits::default(),
    )
    .unwrap_or_else(|error| panic!("Claude hook process failed: {error}"));
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).to_string(),
    )
}
