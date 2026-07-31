use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    AgentPromptFormat, IntegrationAction, McpConfigStatus, PromptTarget, RuntimeAdapter,
    RuntimeEnvironment,
};

pub(crate) const ADAPTER: RuntimeAdapter = RuntimeAdapter {
    name: "Gemini CLI",
    slug: "gemini",
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

pub(crate) fn settings_path(home: &Path) -> PathBuf {
    home.join(".gemini").join("settings.json")
}

pub(crate) fn prompt_path(root: &Path) -> PathBuf {
    root.join("GEMINI.md")
}

fn prompt_targets(environment: &RuntimeEnvironment<'_>) -> Vec<PromptTarget> {
    vec![PromptTarget {
        path: prompt_path(environment.root()),
        format: AgentPromptFormat::Agents,
    }]
}

fn detect(environment: &RuntimeEnvironment<'_>) -> bool {
    environment.home().join(".gemini").is_dir() || environment.command_exists("gemini")
}

fn configure_hooks(
    environment: &RuntimeEnvironment<'_>,
    auto_yes: bool,
) -> Result<McpConfigStatus> {
    crate::cmd_setup::setup_hooks::write_gemini_hook_config(
        &settings_path(environment.home()),
        environment.root(),
        auto_yes,
    )
}

fn hook_artifacts(environment: &RuntimeEnvironment<'_>) -> Vec<PathBuf> {
    vec![settings_path(environment.home())]
}

fn hook_status(environment: &RuntimeEnvironment<'_>) -> String {
    settings_path(environment.home()).display().to_string()
}
