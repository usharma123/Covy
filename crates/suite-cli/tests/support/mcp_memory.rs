use crate::support::mcp::{
    initialize_mcp_session, packet28_cmd, packet28_process, read_mcp_message_for_id,
    write_mcp_message,
};
use serde_json::{json, Value};
use std::fs;
use std::io::BufReader;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use tempfile::TempDir;

pub struct McpMemoryServer {
    root: TempDir,
    home: TempDir,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl McpMemoryServer {
    pub fn start(package_name: &str) -> Self {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        fs::write(
            root.path().join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nserde = \"1\"\n"
            ),
        )
        .unwrap();
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/lib.rs"), "pub fn fixture() {}\n").unwrap();

        let mut child = packet28_process()
            .current_dir(root.path())
            .env("HOME", home.path())
            .args(["mcp", "serve", "--root", root.path().to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        initialize_mcp_session(&mut stdin, &mut stdout);

        Self {
            root,
            home,
            child,
            stdin,
            stdout,
        }
    }

    pub fn call_tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        write_mcp_message(
            &mut self.stdin,
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
        read_mcp_message_for_id(&mut self.stdout, id)
    }

    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
