use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    AgentPromptFormat, IntegrationAction, McpConfigStatus, PromptTarget, RuntimeAdapter,
    RuntimeEnvironment,
};

pub(crate) const ADAPTER: RuntimeAdapter = RuntimeAdapter {
    name: "Claude Code",
    slug: "claude",
    prompt_targets,
    detect,
    mcp: Some(IntegrationAction::new(
        configure_mcp,
        mcp_artifacts,
        mcp_status,
    )),
    hooks: Some(IntegrationAction::new(
        configure_hooks,
        hook_artifacts,
        hook_status,
    )),
    writes_hook_runtime_config: true,
};

pub(crate) fn mcp_config_path(root: &Path) -> PathBuf {
    root.join(".mcp.json")
}

pub(crate) fn settings_path(root: &Path) -> PathBuf {
    root.join(".claude").join("settings.json")
}

pub(crate) fn prompt_path(root: &Path) -> PathBuf {
    root.join("CLAUDE.md")
}

fn prompt_targets(environment: &RuntimeEnvironment<'_>) -> Vec<PromptTarget> {
    vec![PromptTarget {
        path: prompt_path(environment.root()),
        format: AgentPromptFormat::Claude,
    }]
}

fn detect(environment: &RuntimeEnvironment<'_>) -> bool {
    environment.home().join(".claude").is_dir() || environment.command_exists("claude")
}

fn configure_mcp(environment: &RuntimeEnvironment<'_>, auto_yes: bool) -> Result<McpConfigStatus> {
    crate::cmd_setup::write_mcp_config(
        &mcp_config_path(environment.root()),
        environment.root(),
        auto_yes,
    )
}

fn mcp_artifacts(environment: &RuntimeEnvironment<'_>) -> Vec<PathBuf> {
    vec![mcp_config_path(environment.root())]
}

fn mcp_status(environment: &RuntimeEnvironment<'_>) -> String {
    format!(
        "{} → {}",
        ADAPTER.name,
        mcp_config_path(environment.root()).display()
    )
}

fn configure_hooks(
    environment: &RuntimeEnvironment<'_>,
    auto_yes: bool,
) -> Result<McpConfigStatus> {
    crate::cmd_setup::setup_hooks::write_claude_hook_config(
        &settings_path(environment.root()),
        environment.root(),
        auto_yes,
    )
}

fn hook_artifacts(environment: &RuntimeEnvironment<'_>) -> Vec<PathBuf> {
    vec![
        settings_path(environment.root()),
        packet28_daemon_core::hook_runtime_config_path(environment.root()),
    ]
}

fn hook_status(environment: &RuntimeEnvironment<'_>) -> String {
    settings_path(environment.root()).display().to_string()
}
