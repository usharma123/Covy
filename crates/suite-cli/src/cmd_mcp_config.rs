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
}
