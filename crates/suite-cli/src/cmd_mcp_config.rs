use std::collections::BTreeMap;

#[derive(Debug, Clone, serde::Deserialize, Default)]
#[serde(default)]
pub(crate) struct McpProxyConfig {
    #[serde(rename = "mcpServers")]
    pub(crate) mcp_servers: BTreeMap<String, McpProxyServerConfig>,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
#[serde(default)]
pub(crate) struct McpProxyServerConfig {
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) cwd: Option<String>,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) compact_tools: Vec<String>,
    pub(crate) framing: McpProxyStdioFraming,
}

/// Framing used on an upstream server's stdio transport.
///
/// MCP stdio uses newline-delimited JSON. `content_length` remains available
/// only for servers that intentionally implement the older LSP-style framing.
#[derive(Debug, Clone, Copy, serde::Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum McpProxyStdioFraming {
    #[default]
    NewlineJson,
    ContentLength,
}

impl From<McpProxyStdioFraming> for super::transport::McpMessageFraming {
    fn from(value: McpProxyStdioFraming) -> Self {
        match value {
            McpProxyStdioFraming::NewlineJson => Self::NewlineJson,
            McpProxyStdioFraming::ContentLength => Self::ContentLength,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_stdio_framing_defaults_to_mcp_newline_json() {
        let config: McpProxyServerConfig =
            serde_json::from_value(serde_json::json!({"command": "server"})).unwrap();

        assert_eq!(config.framing, McpProxyStdioFraming::NewlineJson);
    }

    #[test]
    fn upstream_stdio_framing_accepts_explicit_legacy_content_length() {
        let config: McpProxyServerConfig = serde_json::from_value(serde_json::json!({
            "command": "legacy-server",
            "framing": "content_length"
        }))
        .unwrap();

        assert_eq!(config.framing, McpProxyStdioFraming::ContentLength);
    }
}
