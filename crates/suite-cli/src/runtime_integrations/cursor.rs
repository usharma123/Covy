use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    AgentPromptFormat, IntegrationAction, McpConfigStatus, PromptTarget, RuntimeAdapter,
    RuntimeEnvironment,
};

pub(crate) const ADAPTER: RuntimeAdapter = RuntimeAdapter {
    name: "Cursor",
    slug: "cursor",
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

pub(crate) fn mcp_config_path(root: &Path) -> PathBuf {
    root.join(".cursor").join("mcp.json")
}

pub(crate) fn hook_config_path(root: &Path) -> PathBuf {
    root.join(".cursor").join("hooks.json")
}

pub(crate) fn rule_path(root: &Path) -> PathBuf {
    root.join(".cursor").join("rules").join("packet28.mdc")
}

pub(crate) fn legacy_rules_path(root: &Path) -> PathBuf {
    root.join(".cursorrules")
}

fn prompt_targets(environment: &RuntimeEnvironment<'_>) -> Vec<PromptTarget> {
    let mut targets = vec![PromptTarget {
        path: rule_path(environment.root()),
        format: AgentPromptFormat::CursorRule,
    }];
    if legacy_rules_path(environment.root()).exists() {
        targets.push(PromptTarget {
            path: legacy_rules_path(environment.root()),
            format: AgentPromptFormat::Cursor,
        });
    }
    targets
}

fn detect(environment: &RuntimeEnvironment<'_>) -> bool {
    environment.home().join(".cursor").is_dir() || environment.command_exists("cursor")
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
    crate::cmd_setup::setup_hooks::write_cursor_hook_config(
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
