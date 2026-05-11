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

pub(crate) mod copilot {
    use super::*;

    pub(crate) fn instructions_path(root: &Path) -> PathBuf {
        root.join(".github").join("copilot-instructions.md")
    }
}

pub(crate) mod gemini {
    use super::*;

    pub(crate) fn prompt_path(root: &Path) -> PathBuf {
        root.join("GEMINI.md")
    }
}

pub(crate) mod opencode {
    use super::*;

    pub(crate) fn prompt_path(root: &Path) -> PathBuf {
        root.join("AGENTS.md")
    }
}

pub(crate) mod hermes {
    use super::*;

    pub(crate) fn prompt_path(root: &Path) -> PathBuf {
        root.join("AGENTS.md")
    }
}

pub(crate) mod cline {
    use super::*;

    pub(crate) fn rules_path(root: &Path) -> PathBuf {
        root.join(".clinerules")
    }
}

pub(crate) mod roo {
    use super::*;

    pub(crate) fn rules_path(root: &Path) -> PathBuf {
        root.join(".roo").join("rules").join("packet28.md")
    }
}

pub(crate) mod kilocode {
    use super::*;

    pub(crate) fn rules_path(root: &Path) -> PathBuf {
        root.join(".kilocode")
            .join("rules")
            .join("packet28-rules.md")
    }
}

pub(crate) mod antigravity {
    use super::*;

    pub(crate) fn rules_path(root: &Path) -> PathBuf {
        root.join(".agents")
            .join("rules")
            .join("antigravity-packet28-rules.md")
    }
}
