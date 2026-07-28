use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use super::config::{McpProxyConfig, McpProxyServerConfig};
use super::transport::{read_message, render_command_preview, write_message, McpMessageFraming};
use crate::runtime_integrations::windsurf;

pub(crate) struct McpSmokeReport {
    pub(crate) server_name: String,
    pub(crate) tool_count: usize,
}

pub(crate) fn smoke_test_agent_config(agent: &str) -> Result<McpSmokeReport> {
    let server = load_agent_mcp_server(agent)?;
    smoke_test_mcp_server(&server)
}

fn load_agent_mcp_server(agent: &str) -> Result<McpProxyServerConfig> {
    match agent {
        "windsurf" => load_named_mcp_server(&windsurf_mcp_config_path(), "packet28"),
        other => Err(anyhow!(
            "unsupported MCP config '{other}'; supported values: windsurf"
        )),
    }
}

fn windsurf_mcp_config_path() -> PathBuf {
    windsurf::mcp_config_path(&dirs_home())
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn load_named_mcp_server(path: &Path, name: &str) -> Result<McpProxyServerConfig> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read '{}'", path.display()))?;
    let config: McpProxyConfig = serde_json::from_str(&content)
        .with_context(|| format!("invalid MCP config '{}'", path.display()))?;
    config
        .mcp_servers
        .get(name)
        .cloned()
        .with_context(|| format!("MCP server '{name}' missing from '{}'", path.display()))
}

fn smoke_test_mcp_server(server: &McpProxyServerConfig) -> Result<McpSmokeReport> {
    if server.command.trim().is_empty() {
        return Err(anyhow!("MCP server command is empty"));
    }
    let mut harness = ConfiguredMcpHarness::start(server)?;
    harness.send(&json!({
        "jsonrpc":"2.0",
        "id":1,
        "method":"initialize",
        "params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"packet28-smoke-test","version":"1"}}
    }))?;
    let initialize = harness.read_response(1)?;
    let server_name = initialize["result"]["serverInfo"]["name"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    harness.send(&json!({
        "jsonrpc":"2.0",
        "id":2,
        "method":"tools/list"
    }))?;
    let tools = harness.read_response(2)?;
    let tool_names = tools["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>();
    if !tool_names.contains(&"packet28_search") {
        return Err(anyhow!("packet28_search missing from tools/list"));
    }
    Ok(McpSmokeReport {
        server_name,
        tool_count: tool_names.len(),
    })
}

struct ConfiguredMcpHarness {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ConfiguredMcpHarness {
    fn start(server: &McpProxyServerConfig) -> Result<Self> {
        let mut command = Command::new(&server.command);
        command.args(&server.args);
        if let Some(cwd) = server.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
            command.current_dir(cwd);
        }
        for (key, value) in &server.env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to start configured MCP server `{}`",
                    render_command_preview(&server.command, &server.args)
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to capture MCP stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture MCP stdout"))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    fn send(&mut self, value: &Value) -> Result<()> {
        write_message(&mut self.stdin, value, McpMessageFraming::ContentLength)
    }

    fn read_response(&mut self, expected_id: u64) -> Result<Value> {
        let started = Instant::now();
        loop {
            if started.elapsed() > Duration::from_secs(10) {
                return Err(anyhow!(
                    "timed out waiting for MCP response id={expected_id}"
                ));
            }
            let Some((value, _)) = read_message(&mut self.stdout)? else {
                return Err(anyhow!(
                    "MCP stream closed before response id={expected_id}"
                ));
            };
            if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
                if let Some(error) = value.get("error") {
                    return Err(anyhow!(
                        "MCP response id={expected_id} returned error: {error}"
                    ));
                }
                return Ok(value);
            }
        }
    }
}

impl Drop for ConfiguredMcpHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
