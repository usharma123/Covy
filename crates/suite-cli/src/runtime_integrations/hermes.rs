use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    AgentPromptFormat, IntegrationAction, McpConfigStatus, PromptTarget, RuntimeAdapter,
    RuntimeEnvironment,
};

pub(crate) const ADAPTER: RuntimeAdapter = RuntimeAdapter {
    name: "Hermes",
    slug: "hermes",
    prompt_targets,
    detect,
    mcp: None,
    hooks: Some(IntegrationAction::new(
        configure_hooks,
        hook_artifacts,
        hook_status,
    )),
    writes_hook_runtime_config: false,
};

pub(crate) fn prompt_path(root: &Path) -> PathBuf {
    root.join("AGENTS.md")
}

pub(crate) fn plugin_dir(home: &Path) -> PathBuf {
    home.join(".hermes")
        .join("plugins")
        .join("packet28-rewrite")
}

pub(crate) fn config_path(home: &Path) -> PathBuf {
    home.join(".hermes").join("config.yaml")
}

fn prompt_targets(environment: &RuntimeEnvironment<'_>) -> Vec<PromptTarget> {
    vec![PromptTarget {
        path: prompt_path(environment.root()),
        format: AgentPromptFormat::Agents,
    }]
}

fn detect(environment: &RuntimeEnvironment<'_>) -> bool {
    environment.home().join(".hermes").is_dir() || environment.command_exists("hermes")
}

fn configure_hooks(
    environment: &RuntimeEnvironment<'_>,
    auto_yes: bool,
) -> Result<McpConfigStatus> {
    crate::cmd_setup::setup_plugins::write_hermes_plugin(environment.home(), auto_yes)
}

fn hook_artifacts(environment: &RuntimeEnvironment<'_>) -> Vec<PathBuf> {
    let plugin_dir = plugin_dir(environment.home());
    vec![
        plugin_dir.join("__init__.py"),
        plugin_dir.join("plugin.yaml"),
        config_path(environment.home()),
    ]
}

fn hook_status(environment: &RuntimeEnvironment<'_>) -> String {
    plugin_dir(environment.home()).display().to_string()
}
