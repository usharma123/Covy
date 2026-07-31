use crate::process_harness::{HarnessLimits, McpHarness, ProcessHarness};
use serde_json::{json, Value};
use std::time::Duration;
use tempfile::TempDir;

const MCP_IO_TIMEOUT: Duration = Duration::from_secs(10);
const MCP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

pub struct McpGraphServer {
    root: TempDir,
    home: TempDir,
    harness: McpHarness,
}

impl McpGraphServer {
    pub fn start() -> Self {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
        command
            .current_dir(root.path())
            .env("HOME", home.path())
            .args(["mcp", "serve", "--root", root.path().to_str().unwrap()]);
        let mut harness = McpHarness::spawn(&mut command, HarnessLimits::default())
            .unwrap_or_else(|error| panic!("failed to start graph MCP server: {error}"));
        harness
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
            .unwrap_or_else(|error| panic!("failed to initialize graph MCP server: {error}"));

        Self {
            root,
            home,
            harness,
        }
    }

    pub fn call_tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.harness
            .request_with_id(
                json!(id),
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments
                }),
                MCP_IO_TIMEOUT,
            )
            .unwrap_or_else(|error| panic!("MCP tool {name} failed: {error}"))
    }

    pub fn stop(mut self) {
        self.harness
            .finish(MCP_SHUTDOWN_TIMEOUT)
            .unwrap_or_else(|error| panic!("failed to stop graph MCP server: {error}"));
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
        command
            .current_dir(self.root.path())
            .env("HOME", self.home.path())
            .args([
                "daemon",
                "stop",
                "--root",
                self.root.path().to_str().unwrap(),
            ]);
        let output =
            ProcessHarness::run(&mut command, &[], COMMAND_TIMEOUT, HarnessLimits::default())
                .unwrap_or_else(|error| panic!("failed to run daemon stop: {error}"));
        assert!(
            output.status.success(),
            "daemon stop failed with {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
