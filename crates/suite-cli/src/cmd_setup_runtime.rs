use std::path::{Path, PathBuf};

use crate::agent_surface;
use crate::runtime_integrations::{
    antigravity, claude, cline, codex, copilot, cursor, gemini, hermes, kilocode, opencode, roo,
    windsurf,
};

pub(crate) struct RuntimeInfo {
    pub(crate) kind: RuntimeKind,
    pub(crate) name: &'static str,
    pub(crate) slug: &'static str,
    pub(crate) prompt_targets: Vec<PromptTarget>,
    pub(crate) detected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeKind {
    Claude,
    Cursor,
    Codex,
    Windsurf,
    Copilot,
    Gemini,
    OpenCode,
    Hermes,
    Cline,
    Roo,
    KiloCode,
    Antigravity,
}

#[derive(Clone, Debug)]
pub(crate) struct PromptTarget {
    pub(crate) path: PathBuf,
    pub(crate) format: agent_surface::AgentPromptFormat,
}

pub(crate) fn detect_runtimes(root: &Path) -> Vec<RuntimeInfo> {
    let home = dirs_home();
    vec![
        RuntimeInfo {
            kind: RuntimeKind::Claude,
            name: "Claude Code",
            slug: "claude",
            prompt_targets: vec![PromptTarget {
                path: claude::prompt_path(root),
                format: agent_surface::AgentPromptFormat::Claude,
            }],
            detected: detect_claude(&home),
        },
        RuntimeInfo {
            kind: RuntimeKind::Cursor,
            name: "Cursor",
            slug: "cursor",
            prompt_targets: cursor_prompt_targets(root),
            detected: detect_cursor(&home),
        },
        RuntimeInfo {
            kind: RuntimeKind::Codex,
            name: "Codex",
            slug: "codex",
            prompt_targets: vec![PromptTarget {
                path: codex::prompt_path(root),
                format: agent_surface::AgentPromptFormat::Agents,
            }],
            detected: detect_codex(),
        },
        RuntimeInfo {
            kind: RuntimeKind::Windsurf,
            name: "Windsurf",
            slug: "windsurf",
            prompt_targets: windsurf_prompt_targets(root),
            detected: detect_windsurf(&home),
        },
        RuntimeInfo {
            kind: RuntimeKind::Copilot,
            name: "GitHub Copilot",
            slug: "copilot",
            prompt_targets: vec![PromptTarget {
                path: copilot::instructions_path(root),
                format: agent_surface::AgentPromptFormat::Agents,
            }],
            detected: detect_copilot(root),
        },
        RuntimeInfo {
            kind: RuntimeKind::Gemini,
            name: "Gemini CLI",
            slug: "gemini",
            prompt_targets: vec![PromptTarget {
                path: gemini::prompt_path(root),
                format: agent_surface::AgentPromptFormat::Agents,
            }],
            detected: detect_gemini(&home),
        },
        RuntimeInfo {
            kind: RuntimeKind::OpenCode,
            name: "OpenCode",
            slug: "opencode",
            prompt_targets: vec![PromptTarget {
                path: opencode::prompt_path(root),
                format: agent_surface::AgentPromptFormat::Agents,
            }],
            detected: detect_opencode(&home),
        },
        RuntimeInfo {
            kind: RuntimeKind::Hermes,
            name: "Hermes",
            slug: "hermes",
            prompt_targets: vec![PromptTarget {
                path: hermes::prompt_path(root),
                format: agent_surface::AgentPromptFormat::Agents,
            }],
            detected: detect_hermes(&home),
        },
        RuntimeInfo {
            kind: RuntimeKind::Cline,
            name: "Cline",
            slug: "cline",
            prompt_targets: vec![PromptTarget {
                path: cline::rules_path(root),
                format: agent_surface::AgentPromptFormat::Agents,
            }],
            detected: detect_cline(&home),
        },
        RuntimeInfo {
            kind: RuntimeKind::Roo,
            name: "Roo Code",
            slug: "roo",
            prompt_targets: vec![PromptTarget {
                path: roo::rules_path(root),
                format: agent_surface::AgentPromptFormat::Agents,
            }],
            detected: detect_roo(&home),
        },
        RuntimeInfo {
            kind: RuntimeKind::KiloCode,
            name: "Kilo Code",
            slug: "kilocode",
            prompt_targets: vec![PromptTarget {
                path: kilocode::rules_path(root),
                format: agent_surface::AgentPromptFormat::Agents,
            }],
            detected: detect_kilocode(&home),
        },
        RuntimeInfo {
            kind: RuntimeKind::Antigravity,
            name: "Google Antigravity",
            slug: "antigravity",
            prompt_targets: vec![PromptTarget {
                path: antigravity::rules_path(root),
                format: agent_surface::AgentPromptFormat::Agents,
            }],
            detected: detect_antigravity(root),
        },
    ]
}

pub(crate) fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn detect_claude(home: &Path) -> bool {
    home.join(".claude").is_dir() || which_exists("claude")
}

fn detect_cursor(home: &Path) -> bool {
    home.join(".cursor").is_dir() || which_exists("cursor")
}

fn detect_codex() -> bool {
    which_exists("codex")
}

fn detect_windsurf(home: &Path) -> bool {
    home.join(".codeium").join("windsurf").is_dir() || which_exists("windsurf")
}

fn detect_copilot(root: &Path) -> bool {
    root.join(".github")
        .join("copilot-instructions.md")
        .is_file()
}

fn detect_gemini(home: &Path) -> bool {
    home.join(".gemini").is_dir() || which_exists("gemini")
}

fn detect_opencode(home: &Path) -> bool {
    home.join(".config").join("opencode").is_dir() || which_exists("opencode")
}

fn detect_hermes(home: &Path) -> bool {
    home.join(".hermes").is_dir() || which_exists("hermes")
}

fn detect_cline(home: &Path) -> bool {
    home.join(".cline").is_dir()
}

fn detect_roo(home: &Path) -> bool {
    home.join(".roo").is_dir()
}

fn detect_kilocode(home: &Path) -> bool {
    home.join(".kilocode").is_dir() || which_exists("kilocode")
}

fn detect_antigravity(root: &Path) -> bool {
    root.join(".agents").is_dir() || which_exists("antigravity")
}

pub(crate) fn which_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub(crate) fn find_claude_mcp_config(_home: &Path, root: &Path) -> Option<PathBuf> {
    Some(claude::mcp_config_path(root))
}

pub(crate) fn find_cursor_mcp_config(root: &Path) -> Option<PathBuf> {
    Some(cursor::mcp_config_path(root))
}

pub(crate) fn runtime_supports_mcp(kind: RuntimeKind) -> bool {
    matches!(
        kind,
        RuntimeKind::Claude | RuntimeKind::Cursor | RuntimeKind::Codex | RuntimeKind::Windsurf
    )
}

pub(crate) fn runtime_supports_hooks(kind: RuntimeKind) -> bool {
    matches!(
        kind,
        RuntimeKind::Claude
            | RuntimeKind::Copilot
            | RuntimeKind::Cursor
            | RuntimeKind::Gemini
            | RuntimeKind::Hermes
            | RuntimeKind::OpenCode
            | RuntimeKind::Windsurf
    )
}

pub(crate) fn runtime_needs_hook_runtime_config(kind: RuntimeKind) -> bool {
    matches!(kind, RuntimeKind::Claude)
}

pub(crate) fn mcp_config_path(kind: RuntimeKind, root: &Path) -> PathBuf {
    match kind {
        RuntimeKind::Claude => find_claude_mcp_config(&dirs_home(), root).expect("claude mcp path"),
        RuntimeKind::Cursor => find_cursor_mcp_config(root).expect("cursor mcp path"),
        RuntimeKind::Codex => codex_config_path(&dirs_home()),
        RuntimeKind::Windsurf => windsurf_mcp_config_path(&dirs_home()),
        RuntimeKind::Copilot
        | RuntimeKind::Gemini
        | RuntimeKind::OpenCode
        | RuntimeKind::Hermes
        | RuntimeKind::Cline
        | RuntimeKind::Roo
        | RuntimeKind::KiloCode
        | RuntimeKind::Antigravity => {
            unreachable!("instruction-only runtimes do not configure MCP")
        }
    }
}

pub(crate) fn hook_config_path(kind: RuntimeKind, root: &Path) -> PathBuf {
    match kind {
        RuntimeKind::Claude => claude::settings_path(root),
        RuntimeKind::Copilot => copilot::hook_config_path(root),
        RuntimeKind::Cursor => cursor::hook_config_path(root),
        RuntimeKind::Gemini => gemini::settings_path(&dirs_home()),
        RuntimeKind::Hermes => hermes::plugin_dir(&dirs_home()),
        RuntimeKind::OpenCode => opencode::plugin_path(&dirs_home()),
        RuntimeKind::Windsurf => windsurf::hook_config_path(root),
        RuntimeKind::Codex
        | RuntimeKind::Cline
        | RuntimeKind::Roo
        | RuntimeKind::KiloCode
        | RuntimeKind::Antigravity => {
            unreachable!("this runtime does not configure Packet28 hooks")
        }
    }
}

pub(crate) fn codex_config_path(home: &Path) -> PathBuf {
    codex::config_path(home)
}

pub(crate) fn windsurf_mcp_config_path(home: &Path) -> PathBuf {
    windsurf::mcp_config_path(home)
}

pub(crate) fn select_setup_runtimes<'a>(
    runtimes: &'a [RuntimeInfo],
    choice: &crate::cmd_setup::SetupPlanChoice,
) -> Vec<&'a RuntimeInfo> {
    match &choice.runtime_scope {
        crate::cmd_setup::SetupRuntimeScope::Detected => {
            runtimes.iter().filter(|runtime| runtime.detected).collect()
        }
        crate::cmd_setup::SetupRuntimeScope::All => runtimes.iter().collect(),
        crate::cmd_setup::SetupRuntimeScope::Single(slug) => runtimes
            .iter()
            .filter(|runtime| runtime.slug == slug)
            .collect(),
    }
}

pub(crate) fn push_prompt_targets(targets: &mut Vec<PromptTarget>, additions: &[PromptTarget]) {
    for addition in additions {
        if targets.iter().any(|target| target.path == addition.path) {
            continue;
        }
        targets.push(addition.clone());
    }
}

fn cursor_prompt_targets(root: &Path) -> Vec<PromptTarget> {
    let mut targets = vec![PromptTarget {
        path: cursor::rule_path(root),
        format: agent_surface::AgentPromptFormat::CursorRule,
    }];
    if cursor::legacy_rules_path(root).exists() {
        targets.push(PromptTarget {
            path: cursor::legacy_rules_path(root),
            format: agent_surface::AgentPromptFormat::Cursor,
        });
    }
    targets
}

fn windsurf_prompt_targets(root: &Path) -> Vec<PromptTarget> {
    vec![PromptTarget {
        path: windsurf::rule_path(root),
        format: agent_surface::AgentPromptFormat::WindsurfRule,
    }]
}

pub(crate) fn prompt_target_label(target: &PromptTarget) -> String {
    target
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| target.path.to_str().unwrap_or("prompt"))
        .to_string()
}
