use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

pub(super) struct FffMcpClient {
    pub(super) root: PathBuf,
    child: Child,
    stdin: io::BufWriter<ChildStdin>,
    stdout: io::BufReader<ChildStdout>,
    next_id: u64,
}

impl FffMcpClient {
    pub(super) fn spawn(root: &Path) -> Result<Self> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let bin = std::env::var("P28_FFF_MCP_BIN").unwrap_or_else(|_| "fff-mcp".to_string());
        let mut child = Command::new(&bin)
            .arg(&root)
            .arg("--no-update-check")
            .arg("--no-watch")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to launch fff MCP backend '{bin}'; install fff-mcp or set P28_FFF_MCP_BIN"
                )
            })?;
        let stdin = io::BufWriter::new(
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow!("fff MCP backend did not expose stdin"))?,
        );
        let stdout = io::BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| anyhow!("fff MCP backend did not expose stdout"))?,
        );
        let mut client = Self {
            root,
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        client.initialize()?;
        Ok(client)
    }

    fn initialize(&mut self) -> Result<()> {
        let id = self.next_request_id();
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "packet28-mcp", "version": env!("CARGO_PKG_VERSION")}
            }
        }))?;
        let _ = self.read_response(id)?;
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))?;
        Ok(())
    }

    pub(super) fn call_grep(
        &mut self,
        request: &packet28_reducer_core::SearchRequest,
    ) -> Result<String> {
        let query = fff_query(request);
        let max_results = request.max_total_matches.unwrap_or(200).max(1);
        let mut last_text = String::new();
        for attempt in 0..10 {
            let id = self.next_request_id();
            self.send(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": "grep",
                    "arguments": {
                        "query": query,
                        "maxResults": max_results,
                        "output_mode": "content"
                    }
                }
            }))?;
            let response = self.read_response(id)?;
            if let Some(error) = response.get("error") {
                return Err(anyhow!("fff MCP grep failed: {error}"));
            }
            let result = response
                .get("result")
                .ok_or_else(|| anyhow!("fff MCP grep response missing result"))?;
            let chunks = result
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            last_text = chunks.join("\n");
            if !is_fff_empty_result(&last_text) || attempt == 9 {
                return Ok(last_text);
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(last_text)
    }

    fn next_request_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send(&mut self, value: Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, &value)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_response(&mut self, id: u64) -> Result<Value> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = self.stdout.read_line(&mut line)?;
            if bytes == 0 {
                return Err(anyhow!("fff MCP backend exited before response id {id}"));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(trimmed)
                .with_context(|| format!("failed to parse fff MCP JSON line: {trimmed}"))?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(value);
            }
        }
    }
}

impl Drop for FffMcpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fff_query(request: &packet28_reducer_core::SearchRequest) -> String {
    if request.requested_paths.is_empty() {
        return request.query.clone();
    }
    let mut parts = request
        .requested_paths
        .iter()
        .map(|path| path.trim_start_matches("./").to_string())
        .collect::<Vec<_>>();
    parts.push(request.query.clone());
    parts.join(" ")
}

fn is_fff_empty_result(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed == "0 matches." || trimmed.starts_with("0 results ")
}
