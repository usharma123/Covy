use std::path::{Path, PathBuf};

pub(crate) mod claude {
    use super::*;

    pub(crate) fn mcp_config_path(root: &Path) -> PathBuf {
        root.join(".mcp.json")
    }

    pub(crate) fn settings_path(root: &Path) -> PathBuf {
        root.join(".claude").join("settings.json")
    }

    pub(crate) fn prompt_path(root: &Path) -> PathBuf {
        root.join("CLAUDE.md")
    }
}

pub(crate) mod cursor {
    use super::*;

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
}

pub(crate) mod codex {
    use super::*;

    pub(crate) fn config_path(home: &Path) -> PathBuf {
        home.join(".codex").join("config.toml")
    }

    pub(crate) fn prompt_path(root: &Path) -> PathBuf {
        root.join("AGENTS.md")
    }
}

pub(crate) mod windsurf {
    use super::*;

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
}
