use crate::support::mcp::{
    initialize_mcp_session, packet28_cmd, packet28_process, read_mcp_message_for_id, spawn_mcp,
    stop_mcp_server, write_mcp_message,
};
use crate::support::process_harness::McpHarness;
use serde_json::{json, Value};
use tempfile::TempDir;

pub struct McpContextTranscriptServer {
    root: TempDir,
    home: TempDir,
    harness: McpHarness,
}

impl McpContextTranscriptServer {
    pub fn start() -> Self {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let mut command = packet28_process();
        command
            .current_dir(root.path())
            .env("HOME", home.path())
            .args(["mcp", "serve", "--root", root.path().to_str().unwrap()]);
        let mut harness = spawn_mcp(&mut command);
        initialize_mcp_session(&mut harness);

        Self {
            root,
            home,
            harness,
        }
    }

    pub fn call_tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        write_mcp_message(
            &mut self.harness,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": name,
                    "arguments": arguments
                }
            }),
        );
        read_mcp_message_for_id(&mut self.harness, id)
    }

    pub fn stop(self) {
        stop_mcp_server(self.harness);
        packet28_cmd()
            .current_dir(self.root.path())
            .env("HOME", self.home.path())
            .args([
                "daemon",
                "stop",
                "--root",
                self.root.path().to_str().unwrap(),
            ])
            .assert()
            .success();
    }
}
