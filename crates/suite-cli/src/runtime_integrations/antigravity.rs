use std::path::{Path, PathBuf};

use super::{AgentPromptFormat, PromptTarget, RuntimeAdapter, RuntimeEnvironment};

pub(crate) const ADAPTER: RuntimeAdapter = RuntimeAdapter {
    name: "Google Antigravity",
    slug: "antigravity",
    prompt_targets,
    detect,
    mcp: None,
    hooks: None,
    writes_hook_runtime_config: false,
};

pub(crate) fn rules_path(root: &Path) -> PathBuf {
    root.join(".agents")
        .join("rules")
        .join("antigravity-packet28-rules.md")
}

fn prompt_targets(environment: &RuntimeEnvironment<'_>) -> Vec<PromptTarget> {
    vec![PromptTarget {
        path: rules_path(environment.root()),
        format: AgentPromptFormat::Agents,
    }]
}

fn detect(environment: &RuntimeEnvironment<'_>) -> bool {
    environment.root().join(".agents").is_dir() || environment.command_exists("antigravity")
}
