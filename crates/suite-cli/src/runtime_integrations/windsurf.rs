use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    AgentPromptFormat, IntegrationAction, McpConfigStatus, PromptTarget, RuntimeAdapter,
    RuntimeEnvironment,
};

pub(crate) const ADAPTER: RuntimeAdapter = RuntimeAdapter {
    name: "Windsurf",
    slug: "windsurf",
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
    writes_hook_runtime_config: false,
};

pub(crate) fn mcp_config_path(home: &Path) -> PathBuf {
    home.join(".codeium")
        .join("windsurf")
        .join("mcp_config.json")
}

pub(crate) fn hook_config_path(root: &Path) -> PathBuf {
    root.join(".windsurf").join("hooks.json")
}

pub(crate) fn rule_path(root: &Path) -> PathBuf {
    root.join(".windsurf").join("rules").join("packet28.md")
}

fn prompt_targets(environment: &RuntimeEnvironment<'_>) -> Vec<PromptTarget> {
    vec![PromptTarget {
        path: rule_path(environment.root()),
        format: AgentPromptFormat::WindsurfRule,
    }]
}

fn detect(environment: &RuntimeEnvironment<'_>) -> bool {
    environment
        .home()
        .join(".codeium")
        .join("windsurf")
        .is_dir()
        || environment.command_exists("windsurf")
}

fn configure_mcp(environment: &RuntimeEnvironment<'_>, auto_yes: bool) -> Result<McpConfigStatus> {
    crate::cmd_setup::write_mcp_config_with_label(
        &mcp_config_path(environment.home()),
        environment.root(),
        auto_yes,
        Some(ADAPTER.name),
    )
}

fn mcp_artifacts(environment: &RuntimeEnvironment<'_>) -> Vec<PathBuf> {
    vec![mcp_config_path(environment.home())]
}

fn mcp_status(environment: &RuntimeEnvironment<'_>) -> String {
    format!(
        "{} → {}",
        ADAPTER.name,
        mcp_config_path(environment.home()).display()
    )
}

fn configure_hooks(
    environment: &RuntimeEnvironment<'_>,
    auto_yes: bool,
) -> Result<McpConfigStatus> {
    crate::cmd_setup::setup_hooks::write_windsurf_hook_config(
        &hook_config_path(environment.root()),
        environment.root(),
        auto_yes,
    )
}

fn hook_artifacts(environment: &RuntimeEnvironment<'_>) -> Vec<PathBuf> {
    vec![hook_config_path(environment.root())]
}

fn hook_status(environment: &RuntimeEnvironment<'_>) -> String {
    hook_config_path(environment.root()).display().to_string()
}
