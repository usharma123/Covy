use crate::process_harness::{HarnessLimits, McpHarness, ProcessHarness};
use assert_cmd::Command;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

const MCP_IO_TIMEOUT: Duration = Duration::from_secs(10);
const MCP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const BUILD_TIMEOUT: Duration = Duration::from_secs(180);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn mcp_cmd() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
}

pub fn ensure_packet28d_built() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut command = std::process::Command::new("cargo");
        command.args(["build", "-p", "packet28d", "--locked"]);
        let output =
            ProcessHarness::run(&mut command, &[], BUILD_TIMEOUT, HarnessLimits::default())
                .unwrap_or_else(|error| panic!("failed to run packet28d build: {error}"));
        assert!(
            output.status.success(),
            "failed to build packet28d\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    });
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
    let mut command = std::process::Command::new("git");
    command.current_dir(root).args(args);
    let output = ProcessHarness::run(&mut command, &[], COMMAND_TIMEOUT, HarnessLimits::default())
        .unwrap_or_else(|error| panic!("git {args:?} failed to run: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn init_repo(root: &Path) {
    git(root, &["init"]);
}

pub fn write_mcp_message(server: &mut McpHarness, value: &Value) {
    server
        .send_value(value)
        .unwrap_or_else(|error| panic!("failed to write MCP message: {error}"));
}

pub fn read_mcp_message(server: &mut McpHarness) -> Value {
    server
        .receive(MCP_IO_TIMEOUT)
        .unwrap_or_else(|error| panic!("failed to read MCP message: {error}"))
}

pub fn read_mcp_message_for_id(server: &mut McpHarness, expected_id: u64) -> Value {
    server
        .recv_for_id(&json!(expected_id), MCP_IO_TIMEOUT)
        .unwrap_or_else(|error| panic!("failed to read MCP response for id {expected_id}: {error}"))
}

pub fn start_mcp_proxy_server(root: &Path, config_path: &Path, task_id: &str) -> McpHarness {
    let mut command = mcp_cmd();
    command.current_dir(root).args([
        "mcp",
        "proxy",
        "--root",
        root.to_str().unwrap(),
        "--upstream-config",
        config_path.to_str().unwrap(),
        "--task-id",
        task_id,
    ]);
    McpHarness::spawn(&mut command, HarnessLimits::default())
        .unwrap_or_else(|error| panic!("failed to start MCP proxy: {error}"))
}

pub fn start_mcp_proxy_server_with_tool(
    root: &Path,
    config_path: &Path,
    task_id: &str,
    tool_name: &str,
) -> (McpHarness, Value) {
    for _ in 0..3 {
        let mut server = start_mcp_proxy_server(root, config_path, task_id);
        initialize_mcp_session(&mut server);
        write_mcp_message(
            &mut server,
            &json!({
                "jsonrpc":"2.0",
                "id":2,
                "method":"tools/list"
            }),
        );
        let tools = read_mcp_message_for_id(&mut server, 2);
        let has_tool = tools["result"]["tools"]
            .as_array()
            .is_some_and(|items| items.iter().any(|tool| tool["name"] == tool_name));
        if has_tool {
            return (server, tools);
        }
        drop(server);
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("proxy tool catalog never exposed required tool '{tool_name}'");
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

pub fn close_mcp_stdin(server: &mut McpHarness) {
    server
        .close_stdin()
        .unwrap_or_else(|error| panic!("failed to close MCP stdin: {error}"));
}

pub fn stop_mcp_server(mut server: McpHarness) {
    server
        .finish(MCP_SHUTDOWN_TIMEOUT)
        .unwrap_or_else(|error| panic!("failed to stop MCP server: {error}"));
}
