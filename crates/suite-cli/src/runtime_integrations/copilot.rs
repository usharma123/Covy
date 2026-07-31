use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    AgentPromptFormat, IntegrationAction, McpConfigStatus, PromptTarget, RuntimeAdapter,
    RuntimeEnvironment,
};

pub(crate) const ADAPTER: RuntimeAdapter = RuntimeAdapter {
    name: "GitHub Copilot",
    slug: "copilot",
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

pub(crate) fn instructions_path(root: &Path) -> PathBuf {
    root.join(".github").join("copilot-instructions.md")
}

pub(crate) fn hook_config_path(root: &Path) -> PathBuf {
    root.join(".github")
        .join("hooks")
        .join("packet28-rewrite.json")
}

fn prompt_targets(environment: &RuntimeEnvironment<'_>) -> Vec<PromptTarget> {
    vec![PromptTarget {
        path: instructions_path(environment.root()),
        format: AgentPromptFormat::Agents,
    }]
}

fn detect(environment: &RuntimeEnvironment<'_>) -> bool {
    instructions_path(environment.root()).is_file()
}

fn configure_hooks(
    environment: &RuntimeEnvironment<'_>,
    auto_yes: bool,
) -> Result<McpConfigStatus> {
    crate::cmd_setup::setup_hooks::write_copilot_hook_config(
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
