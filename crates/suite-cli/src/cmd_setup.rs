use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
#[cfg(test)]
use packet28_daemon_core::RelaunchPreference;
use packet28_daemon_core::{DaemonIndexRebuildRequest, DaemonRequest, DaemonResponse};
use serde_json::{json, Value};
use toml::value::Table as TomlTable;

use crate::agent_surface;
#[cfg(test)]
use crate::cmd_setup_index::classify_setup_index_status;
use crate::cmd_setup_index::{
    render_setup_index_progress, verify_setup_index, SetupIndexVerification,
};
use crate::cmd_setup_render::{
    format_runtime_detection_badge, format_setup_badge, render_setup_banner,
    render_setup_detection_overview, render_setup_intro, render_setup_menu_hint,
    render_setup_menu_option, render_setup_section, render_setup_step, runtime_capability_summary,
    setup_menu_index_badge, SetupBadgeStyle,
};
#[cfg(test)]
use crate::cmd_setup_runtime::PromptTarget;
use crate::cmd_setup_runtime::{
    codex_config_path, detect_runtimes, dirs_home, hook_config_path, mcp_config_path,
    prompt_target_label, push_prompt_targets, runtime_needs_hook_runtime_config,
    runtime_supports_hooks, runtime_supports_mcp, select_setup_runtimes, which_exists, RuntimeInfo,
    RuntimeKind,
};
#[cfg(test)]
use crate::runtime_integrations::hermes;

#[path = "cmd_setup_commands.rs"]
mod setup_commands;
use setup_commands::resolve_packet28_mcp_command;
#[cfg(test)]
use setup_commands::{
    apply_generated_relaunch_command, generated_relaunch_command, guarded_packet28_hook_command,
    resolve_packet28_cli_command, shell_escape,
};
#[path = "cmd_setup_hooks.rs"]
mod setup_hooks;
use setup_hooks::{
    write_claude_hook_config, write_copilot_hook_config, write_cursor_hook_config,
    write_gemini_hook_config, write_hook_runtime_config, write_windsurf_hook_config,
};
#[path = "cmd_setup_plugins.rs"]
mod setup_plugins;
#[cfg(test)]
use setup_plugins::{hermes_config_enables_packet28, patch_hermes_config};
use setup_plugins::{write_hermes_plugin, write_opencode_plugin};

#[derive(Args)]
pub struct SetupArgs {
    /// Workspace root for Packet28
    #[arg(long, default_value = ".")]
    pub root: String,

    /// Skip interactive prompts and auto-configure all detected runtimes
    #[arg(long)]
    pub yes: bool,

    /// Only generate agent.md fallback files, skip MCP config
    #[arg(long)]
    pub fallback_only: bool,

    /// Specific runtime to configure (claude, cursor, codex, windsurf, copilot, gemini, opencode, hermes, cline, roo, kilocode, antigravity, all)
    #[arg(long, default_value = "all")]
    pub runtime: String,
}

enum McpConfigStatus {
    Written,
    AlreadyConfigured,
    Declined,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetupMode {
    Recommended,
    Custom,
    GuidanceOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SetupRuntimeScope {
    Detected,
    All,
    Single(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SetupPlanChoice {
    pub(crate) mode: SetupMode,
    pub(crate) runtime_scope: SetupRuntimeScope,
    pub(crate) fallback_only: bool,
}

pub fn run(args: SetupArgs) -> Result<i32> {
    let root = crate::cmd_daemon::resolve_root_arg(&args.root);
    let root_display = root.display().to_string();

    // Detect runtimes
    let runtimes = detect_runtimes(&root);
    let setup_choice = match resolve_setup_choice(&args, &runtimes)? {
        Some(choice) => choice,
        None => {
            println!("  {}", "Setup canceled.".yellow().bold());
            println!();
            return Ok(0);
        }
    };
    let selected_runtimes = select_setup_runtimes(&runtimes, &setup_choice);
    let auto_apply = render_setup_intro(
        &root_display,
        &runtimes,
        &selected_runtimes,
        &setup_choice,
        args.yes,
    )?;
    if !auto_apply {
        println!("  {}", "Setup canceled.".yellow().bold());
        println!();
        return Ok(0);
    }

    let mut prompt_targets = Vec::new();
    let mut mcp_configured = false;
    let mut hook_configured = false;
    let mut any_hook_runtime_configs_written = false;
    let mut agent_files_ready = false;
    let mut exit_code = 0;

    render_setup_step(
        1,
        4,
        "Configure Runtime Integrations",
        "Wire up MCP and hooks for the runtimes you selected.",
    );
    if !setup_choice.fallback_only {
        if !selected_runtimes
            .iter()
            .copied()
            .any(|runtime| runtime_supports_mcp(runtime.kind))
        {
            println!(
                "  {} No MCP-capable runtimes selected. Falling back to instruction files.",
                format_setup_badge("note", SetupBadgeStyle::Warning)
            );
            println!();
        } else {
            render_setup_section("MCP servers");
            for rt in &selected_runtimes {
                if !runtime_supports_mcp(rt.kind) {
                    continue;
                }
                match configure_runtime_mcp(rt, &root, auto_apply)? {
                    McpConfigStatus::Written => {
                        mcp_configured = true;
                        push_prompt_targets(&mut prompt_targets, &rt.prompt_targets);
                        println!(
                            "    {} {}",
                            "✓".green().bold(),
                            runtime_mcp_status(rt, &root).dimmed()
                        );
                    }
                    McpConfigStatus::AlreadyConfigured => {
                        mcp_configured = true;
                        push_prompt_targets(&mut prompt_targets, &rt.prompt_targets);
                        println!("    {} {} (already configured)", "·".dimmed(), rt.name,);
                    }
                    McpConfigStatus::Declined => {
                        println!("    {} {} (skipped)", "·".dimmed(), rt.name);
                    }
                }
            }
            println!();
        }
    }

    if selected_runtimes
        .iter()
        .copied()
        .any(|runtime| runtime_supports_hooks(runtime.kind))
    {
        render_setup_section("Runtime hooks");
        for rt in &selected_runtimes {
            if !runtime_supports_hooks(rt.kind) {
                continue;
            }
            match configure_runtime_hooks(rt, &root, auto_apply)? {
                McpConfigStatus::Written => {
                    hook_configured = true;
                    if runtime_needs_hook_runtime_config(rt.kind) {
                        any_hook_runtime_configs_written = true;
                    }
                    println!(
                        "    {} {} hooks → {}",
                        "✓".green().bold(),
                        rt.name,
                        runtime_hook_status(rt, &root).dimmed()
                    );
                }
                McpConfigStatus::AlreadyConfigured => {
                    hook_configured = true;
                    if runtime_needs_hook_runtime_config(rt.kind) {
                        any_hook_runtime_configs_written = true;
                    }
                    println!(
                        "    {} {} hooks (already configured)",
                        "·".dimmed(),
                        rt.name
                    );
                }
                McpConfigStatus::Declined => {
                    println!("    {} {} hooks (skipped)", "·".dimmed(), rt.name);
                }
            }
        }
        if matches!(
            write_hook_runtime_config(&root, any_hook_runtime_configs_written)?,
            McpConfigStatus::Written
        ) {
            println!(
                "    {} Packet28 hook runtime → {}",
                "✓".green().bold(),
                packet28_daemon_core::hook_runtime_config_path(&root)
                    .display()
                    .to_string()
                    .dimmed()
            );
        }
        if any_hook_runtime_configs_written {
            match crate::cmd_daemon::restart_daemon(&root) {
                Ok(_) => {
                    println!(
                        "    {} restarted packet28d for hook compatibility",
                        "✓".green().bold()
                    );
                }
                Err(err) => {
                    println!(
                        "    {} could not restart packet28d automatically: {}",
                        "·".dimmed(),
                        err
                    );
                }
            }
        }
        println!();
    }

    for rt in selected_runtimes
        .iter()
        .copied()
        .filter(|runtime| setup_choice.fallback_only || !runtime_supports_mcp(runtime.kind))
    {
        push_prompt_targets(&mut prompt_targets, &rt.prompt_targets);
    }

    render_setup_step(
        2,
        4,
        "Write Instruction Files",
        "Refresh the prompt fragments each runtime reads from this repo.",
    );
    let root_str = if root_display == "." {
        None
    } else {
        Some(root_display.as_str())
    };

    for target in &prompt_targets {
        let content = agent_surface::render_prompt_fragment(target.format, root_str);
        let path = &target.path;

        let wrote = write_agent_file(path, &content)?;
        agent_files_ready = true;
        if wrote {
            println!(
                "    {} {} → {}",
                "✓".green().bold(),
                prompt_target_label(target),
                path.display().to_string().dimmed()
            );
        } else {
            println!(
                "    {} {} (already up to date)",
                "·".dimmed(),
                prompt_target_label(target),
            );
        }
    }

    // Write a generic fallback only when no runtime-specific target was selected.
    if prompt_targets.is_empty() {
        if selected_runtimes.is_empty() {
            let generic_path = root.join("agent.md");
            let content = agent_surface::render_prompt_fragment(
                agent_surface::AgentPromptFormat::Agents,
                root_str,
            );
            let wrote = write_agent_file(&generic_path, &content)?;
            agent_files_ready = true;
            if wrote {
                println!(
                    "    {} generic → {}",
                    "✓".green().bold(),
                    generic_path.display().to_string().dimmed()
                );
            } else {
                println!("    {} generic (already up to date)", "·".dimmed());
            }
        } else {
            println!("    {} no runtime instruction files selected", "·".dimmed());
        }
    }
    println!();

    render_setup_step(
        3,
        4,
        "Start Daemon And Build Indexes",
        "Bring packet28d online, then verify both indexes are healthy.",
    );
    render_setup_section("Daemon");
    match crate::cmd_daemon::ensure_daemon(&root) {
        Ok(_) => {
            println!("    {} daemon running", "✓".green().bold());
            render_setup_section("Repo index");
            match crate::cmd_daemon::send_request(
                &root,
                &DaemonRequest::DaemonIndexRebuild {
                    request: DaemonIndexRebuildRequest {
                        root: root.display().to_string(),
                        full: true,
                        paths: Vec::new(),
                    },
                },
            ) {
                Ok(DaemonResponse::DaemonIndexRebuild { response }) => {
                    if response.accepted {
                        println!("    {} rebuild queued", "✓".green().bold());
                    } else {
                        println!("    {} rebuild request was not accepted", "✗".red().bold());
                        exit_code = 1;
                    }
                    match verify_setup_index(&root)? {
                        SetupIndexVerification::Ready(response) => {
                            println!(
                                "    {} index ready (generation={}, regex_generation={}, files={}, regex_files={})",
                                "✓".green().bold(),
                                response.manifest.generation,
                                response.manifest.regex_generation.unwrap_or_default(),
                                response.manifest.indexed_files,
                                response.manifest.regex_indexed_files
                            );
                        }
                        SetupIndexVerification::Building(response) => {
                            println!(
                                "    {} index still building ({})",
                                "·".yellow().bold(),
                                render_setup_index_progress(&response)
                            );
                        }
                        SetupIndexVerification::Failed { response, reason } => {
                            if let Some(response) = response {
                                println!(
                                    "    {} index failed: {} (status={}, regex_status={}, generation={}, regex_generation={})",
                                    "✗".red().bold(),
                                    reason,
                                    response.manifest.status,
                                    response.manifest.regex_status.as_deref().unwrap_or("missing"),
                                    response.manifest.generation,
                                    response.manifest.regex_generation.unwrap_or_default()
                                );
                            } else {
                                println!("    {} index failed: {}", "✗".red().bold(), reason);
                            }
                            println!(
                                "    {} run `Packet28 daemon index rebuild --root {}` and inspect `Packet28 daemon index status --root {} --json`",
                                "hint:".cyan().bold(),
                                root_display,
                                root_display
                            );
                            exit_code = 1;
                        }
                    }
                }
                Ok(other) => {
                    println!(
                        "    {} unexpected index rebuild response: {other:?}",
                        "·".dimmed()
                    );
                    exit_code = 1;
                }
                Err(err) => {
                    println!(
                        "    {} failed to queue index build: {}",
                        "✗".red().bold(),
                        err
                    );
                    exit_code = 1;
                }
            }
        }
        Err(e) => {
            println!("    {} daemon failed to start: {}", "✗".red().bold(), e);
            println!(
                "    {} run `packet28 daemon start --root {}` manually",
                "hint:".cyan().bold(),
                root_display
            );
            exit_code = 1;
        }
    }
    println!();

    render_setup_step(
        4,
        4,
        "Summary",
        "A quick recap of what changed and how to verify the install.",
    );
    if exit_code == 0 {
        println!("    {}", "Setup complete.".green().bold());
    } else {
        println!("    {}", "Setup finished with errors.".red().bold());
    }
    println!();
    render_setup_section("What changed");

    if mcp_configured && !setup_choice.fallback_only {
        println!("    Your agent runtimes are configured to use Packet28 control-plane MCP tools.");
        if hook_configured {
            println!(
                "    Selected runtime hooks will capture tool activity directly into Packet28."
            );
        }
        println!("    Reducer-runner cache safety is enabled with workspace fingerprints.");
        println!("    Start a new session and Packet28 intent/handoff tools will be available.");
    } else if agent_files_ready {
        println!("    Agent instruction files have been written.");
        println!("    Include them in your agent's context or system prompt.");
    } else {
        println!("    No runtime artifacts were written.");
        println!("    Re-run setup and select a runtime to configure.");
    }

    println!();
    render_setup_section("Verify with");
    println!("    packet28 --version");
    println!("    packet28 daemon status --root {root_display}");
    println!("    packet28 doctor --root {root_display}");
    println!("    packet28-mcp --root {root_display}  # then call packet28.agent_status");
    println!();

    Ok(exit_code)
}

fn resolve_setup_choice(
    args: &SetupArgs,
    runtimes: &[RuntimeInfo],
) -> Result<Option<SetupPlanChoice>> {
    if args.yes || has_explicit_setup_overrides(args) {
        return explicit_setup_choice(args, runtimes).map(Some);
    }
    prompt_setup_choice(runtimes)
}

fn has_explicit_setup_overrides(args: &SetupArgs) -> bool {
    args.fallback_only || args.runtime != "all"
}

fn explicit_setup_choice(args: &SetupArgs, runtimes: &[RuntimeInfo]) -> Result<SetupPlanChoice> {
    let runtime_scope = if args.runtime == "all" {
        SetupRuntimeScope::Detected
    } else if runtimes.iter().any(|runtime| runtime.slug == args.runtime) {
        SetupRuntimeScope::Single(args.runtime.clone())
    } else {
        anyhow::bail!(
            "unknown runtime '{}'. Use: {}, or all",
            args.runtime,
            supported_runtime_slugs().join(", ")
        );
    };
    let mode = if args.fallback_only {
        SetupMode::GuidanceOnly
    } else if args.runtime == "all" {
        SetupMode::Recommended
    } else {
        SetupMode::Custom
    };
    Ok(SetupPlanChoice {
        mode,
        runtime_scope,
        fallback_only: args.fallback_only,
    })
}

fn supported_runtime_slugs() -> Vec<&'static str> {
    vec![
        "claude",
        "cursor",
        "codex",
        "windsurf",
        "copilot",
        "gemini",
        "opencode",
        "hermes",
        "cline",
        "roo",
        "kilocode",
        "antigravity",
    ]
}

fn prompt_setup_choice(runtimes: &[RuntimeInfo]) -> Result<Option<SetupPlanChoice>> {
    render_setup_banner(
        "Packet28 Setup Wizard",
        "Choose how much Packet28 should configure for this workspace.",
    );
    render_setup_detection_overview(runtimes);
    render_setup_menu_option(
        1,
        "Recommended",
        "Detected runtimes",
        "Configure the runtimes Packet28 found and keep the rest untouched.",
        true,
    );
    render_setup_menu_option(
        2,
        "Advanced",
        "Choose scope",
        "Pick detected, all supported, or a single runtime before anything is written.",
        false,
    );
    render_setup_menu_option(
        3,
        "Guidance only",
        "Instruction files only",
        "Skip MCP integration and only write the prompt files needed for manual setup.",
        false,
    );
    render_setup_menu_hint();
    println!();

    let selection = prompt_menu_selection("  Select a setup path [1]: ", 3, 1)?;
    let Some(selection) = selection else {
        return Ok(None);
    };

    let choice = match selection {
        1 => SetupPlanChoice {
            mode: SetupMode::Recommended,
            runtime_scope: SetupRuntimeScope::Detected,
            fallback_only: false,
        },
        2 => match prompt_advanced_setup_choice(runtimes)? {
            Some(choice) => choice,
            None => return Ok(None),
        },
        3 => SetupPlanChoice {
            mode: SetupMode::GuidanceOnly,
            runtime_scope: SetupRuntimeScope::Detected,
            fallback_only: true,
        },
        _ => unreachable!("validated menu choice"),
    };
    Ok(Some(choice))
}

fn prompt_advanced_setup_choice(runtimes: &[RuntimeInfo]) -> Result<Option<SetupPlanChoice>> {
    render_setup_banner(
        "Advanced Runtime Scope",
        "Choose exactly which runtimes Packet28 should target.",
    );
    render_setup_detection_overview(runtimes);
    render_setup_menu_option(
        1,
        "Detected runtimes",
        "Low risk",
        "Only touch runtimes that already appear to be installed on this machine.",
        true,
    );
    render_setup_menu_option(
        2,
        "All supported runtimes",
        "Broader coverage",
        "Write setup for every supported runtime so this repo is ready across editors.",
        false,
    );
    render_setup_menu_option(
        3,
        "Single runtime",
        "Most precise",
        "Target one runtime explicitly and leave every other integration alone.",
        false,
    );
    render_setup_menu_hint();
    println!();

    let selection = prompt_menu_selection("  Select a runtime scope [1]: ", 3, 1)?;
    let Some(selection) = selection else {
        return Ok(None);
    };

    let runtime_scope = match selection {
        1 => SetupRuntimeScope::Detected,
        2 => SetupRuntimeScope::All,
        3 => prompt_single_runtime_scope(runtimes)?,
        _ => unreachable!("validated menu choice"),
    };

    Ok(Some(SetupPlanChoice {
        mode: SetupMode::Custom,
        runtime_scope,
        fallback_only: false,
    }))
}

fn prompt_single_runtime_scope(runtimes: &[RuntimeInfo]) -> Result<SetupRuntimeScope> {
    render_setup_banner(
        "Choose A Runtime",
        "Pick the single runtime Packet28 should configure in this workspace.",
    );
    for (idx, runtime) in runtimes.iter().enumerate() {
        let availability = format_runtime_detection_badge(runtime.detected);
        let capability = runtime_capability_summary(runtime);
        println!(
            "    {} {}",
            setup_menu_index_badge(idx + 1),
            runtime.name.bold()
        );
        println!("        {}  {}", availability, capability.dimmed());
        println!();
    }
    render_setup_menu_hint();
    println!();

    let selection =
        prompt_menu_selection("  Select a runtime [1]: ", runtimes.len(), 1)?.unwrap_or(1);
    Ok(SetupRuntimeScope::Single(
        runtimes
            .get(selection - 1)
            .map(|runtime| runtime.slug.to_string())
            .unwrap_or_else(|| runtimes[0].slug.to_string()),
    ))
}

fn prompt_menu_selection(prompt: &str, max: usize, default: usize) -> Result<Option<usize>> {
    loop {
        eprint!("{prompt}");
        io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let trimmed = input.trim().to_lowercase();
        if trimmed == "q" || trimmed == "quit" || trimmed == "cancel" {
            println!();
            return Ok(None);
        }
        if trimmed.is_empty() {
            println!();
            return Ok(Some(default));
        }
        if let Ok(value) = trimmed.parse::<usize>() {
            if (1..=max).contains(&value) {
                println!();
                return Ok(Some(value));
            }
        }
        println!(
            "  {} Choose 1-{max}, press Enter for {default}, or type 'q' to cancel.",
            "hint:".cyan().bold()
        );
    }
}

fn configure_runtime_mcp(
    runtime: &RuntimeInfo,
    root: &Path,
    auto_yes: bool,
) -> Result<McpConfigStatus> {
    match runtime.kind {
        RuntimeKind::Claude | RuntimeKind::Cursor => {
            write_mcp_config(&mcp_config_path(runtime.kind, root), root, auto_yes)
        }
        RuntimeKind::Codex => configure_codex_mcp(root, auto_yes),
        RuntimeKind::Windsurf => {
            write_windsurf_mcp_config(&mcp_config_path(runtime.kind, root), root, auto_yes)
        }
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

fn configure_runtime_hooks(
    runtime: &RuntimeInfo,
    root: &Path,
    auto_yes: bool,
) -> Result<McpConfigStatus> {
    match runtime.kind {
        RuntimeKind::Claude => {
            write_claude_hook_config(&hook_config_path(runtime.kind, root), root, auto_yes)
        }
        RuntimeKind::Cursor => {
            write_cursor_hook_config(&hook_config_path(runtime.kind, root), root, auto_yes)
        }
        RuntimeKind::Copilot => {
            write_copilot_hook_config(&hook_config_path(runtime.kind, root), root, auto_yes)
        }
        RuntimeKind::Gemini => {
            write_gemini_hook_config(&hook_config_path(runtime.kind, root), root, auto_yes)
        }
        RuntimeKind::OpenCode => {
            write_opencode_plugin(&hook_config_path(runtime.kind, root), auto_yes)
        }
        RuntimeKind::Hermes => write_hermes_plugin(&dirs_home(), auto_yes),
        RuntimeKind::Windsurf => {
            write_windsurf_hook_config(&hook_config_path(runtime.kind, root), root, auto_yes)
        }
        RuntimeKind::Codex
        | RuntimeKind::Cline
        | RuntimeKind::Roo
        | RuntimeKind::KiloCode
        | RuntimeKind::Antigravity => {
            unreachable!("this runtime does not configure Packet28 hooks")
        }
    }
}

fn runtime_mcp_status(runtime: &RuntimeInfo, root: &Path) -> String {
    match runtime.kind {
        RuntimeKind::Claude => format!(
            "{} → {}",
            runtime.name,
            mcp_config_path(runtime.kind, root).display()
        ),
        RuntimeKind::Cursor => format!(
            "{} → {}",
            runtime.name,
            mcp_config_path(runtime.kind, root).display()
        ),
        RuntimeKind::Codex => format!(
            "{} → {}",
            runtime.name,
            mcp_config_path(runtime.kind, root).display()
        ),
        RuntimeKind::Windsurf => format!(
            "{} → {}",
            runtime.name,
            mcp_config_path(runtime.kind, root).display()
        ),
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

fn runtime_hook_status(runtime: &RuntimeInfo, root: &Path) -> String {
    match runtime.kind {
        RuntimeKind::Claude
        | RuntimeKind::Copilot
        | RuntimeKind::Cursor
        | RuntimeKind::Gemini
        | RuntimeKind::Hermes
        | RuntimeKind::OpenCode
        | RuntimeKind::Windsurf => hook_config_path(runtime.kind, root).display().to_string(),
        RuntimeKind::Codex
        | RuntimeKind::Cline
        | RuntimeKind::Roo
        | RuntimeKind::KiloCode
        | RuntimeKind::Antigravity => {
            unreachable!("this runtime does not configure Packet28 hooks")
        }
    }
}

fn write_mcp_config(path: &Path, root: &Path, auto_yes: bool) -> Result<McpConfigStatus> {
    let root_arg = if root == Path::new(".") {
        ".".to_string()
    } else {
        root.display().to_string()
    };

    let command = resolve_packet28_mcp_command();
    let packet28_entry = json!({
        "command": command,
        "args": ["--root", root_arg, "--toolset", "core"]
    });

    // Read existing config or start fresh
    let mut config: BTreeMap<String, Value> = if path.exists() {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read '{}'", path.display()))?;
        serde_json::from_str(&content).with_context(|| {
            format!(
                "refusing to overwrite invalid JSON in '{}'; fix the file and rerun setup",
                path.display()
            )
        })?
    } else {
        BTreeMap::new()
    };

    // Check if packet28 is already configured
    let servers = config
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));

    if !auto_yes {
        eprint!(
            "    Write MCP config to {}? [Y/n] ",
            path.display().to_string().dimmed()
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let trimmed = input.trim().to_lowercase();
        if !trimmed.is_empty() && trimmed != "y" && trimmed != "yes" {
            return Ok(McpConfigStatus::Declined);
        }
    }

    // Insert packet28 server
    if let Some(obj) = servers.as_object_mut() {
        let needs_write = obj.get("packet28") != Some(&packet28_entry);
        if !needs_write {
            return Ok(McpConfigStatus::AlreadyConfigured);
        }
        obj.insert("packet28".to_string(), packet28_entry);
    }

    // Write back
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(&config)?;
    fs::write(path, format!("{content}\n"))?;

    Ok(McpConfigStatus::Written)
}

fn configure_codex_mcp(root: &Path, auto_yes: bool) -> Result<McpConfigStatus> {
    let config_path = codex_config_path(&dirs_home());
    if codex_mcp_entry_matches(&config_path, root)? {
        return Ok(McpConfigStatus::AlreadyConfigured);
    }
    if !auto_yes {
        eprint!(
            "    Register Packet28 MCP in Codex via {}? [Y/n] ",
            config_path.display().to_string().dimmed()
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let trimmed = input.trim().to_lowercase();
        if !trimmed.is_empty() && trimmed != "y" && trimmed != "yes" {
            return Ok(McpConfigStatus::Declined);
        }
    }
    if which_exists("codex") && run_codex_mcp_add(root).unwrap_or(false) {
        return Ok(McpConfigStatus::Written);
    }
    write_codex_mcp_config(&config_path, root)
}

fn codex_mcp_entry_matches(path: &Path, root: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let config = read_toml_config(path)?;
    let Some(server) = config
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .and_then(|servers| servers.get("packet28"))
        .and_then(toml::Value::as_table)
    else {
        return Ok(false);
    };
    let command_matches = server
        .get("command")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        == Some(resolve_packet28_mcp_command().as_str());
    let expected_root = root.display().to_string();
    let args_matches = server
        .get("args")
        .and_then(toml::Value::as_array)
        .map(|args| {
            args.iter()
                .filter_map(toml::Value::as_str)
                .collect::<Vec<_>>()
                == vec!["--root", expected_root.as_str(), "--toolset", "core"]
        })
        .unwrap_or(false);
    Ok(command_matches && args_matches)
}

fn run_codex_mcp_add(root: &Path) -> Result<bool> {
    let status = std::process::Command::new("codex")
        .args([
            "mcp",
            "add",
            "packet28",
            "--",
            &resolve_packet28_mcp_command(),
            "--root",
            &root.display().to_string(),
            "--toolset",
            "core",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("failed to run `codex mcp add`")?;
    Ok(status.success())
}

fn write_codex_mcp_config(path: &Path, root: &Path) -> Result<McpConfigStatus> {
    let mut config = read_toml_config_or_default(path)?;
    let table = config
        .as_table_mut()
        .context("Codex config must be a TOML table")?;
    let servers = toml_table_entry(table, "mcp_servers", path)?;
    let desired_command = resolve_packet28_mcp_command();
    let desired_root = root.display().to_string();
    let desired_args = vec![
        toml::Value::String("--root".to_string()),
        toml::Value::String(desired_root.clone()),
        toml::Value::String("--toolset".to_string()),
        toml::Value::String("core".to_string()),
    ];
    let already_configured = servers
        .get("packet28")
        .and_then(toml::Value::as_table)
        .is_some_and(|packet28| {
            packet28
                .get("command")
                .and_then(toml::Value::as_str)
                .map(str::trim)
                == Some(desired_command.as_str())
                && packet28.get("args").and_then(toml::Value::as_array) == Some(&desired_args)
        });
    if already_configured {
        return Ok(McpConfigStatus::AlreadyConfigured);
    }
    let mut packet28 = TomlTable::new();
    packet28.insert("command".to_string(), toml::Value::String(desired_command));
    packet28.insert("args".to_string(), toml::Value::Array(desired_args));
    servers.insert("packet28".to_string(), toml::Value::Table(packet28));
    write_toml_config(path, &config)?;
    Ok(McpConfigStatus::Written)
}

fn write_windsurf_mcp_config(path: &Path, root: &Path, auto_yes: bool) -> Result<McpConfigStatus> {
    let root_arg = root.display().to_string();
    let packet28_entry = json!({
        "command": resolve_packet28_mcp_command(),
        "args": ["--root", root_arg, "--toolset", "core"]
    });
    let mut config: BTreeMap<String, Value> = if path.exists() {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read '{}'", path.display()))?;
        serde_json::from_str(&content).with_context(|| {
            format!(
                "refusing to overwrite invalid JSON in '{}'; fix the file and rerun setup",
                path.display()
            )
        })?
    } else {
        BTreeMap::new()
    };
    let servers = config
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));

    if !auto_yes {
        eprint!(
            "    Write Windsurf MCP config to {}? [Y/n] ",
            path.display().to_string().dimmed()
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let trimmed = input.trim().to_lowercase();
        if !trimmed.is_empty() && trimmed != "y" && trimmed != "yes" {
            return Ok(McpConfigStatus::Declined);
        }
    }

    if let Some(obj) = servers.as_object_mut() {
        let needs_write = obj.get("packet28") != Some(&packet28_entry);
        if !needs_write {
            return Ok(McpConfigStatus::AlreadyConfigured);
        }
        obj.insert("packet28".to_string(), packet28_entry);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(&config)?;
    fs::write(path, format!("{content}\n"))?;
    Ok(McpConfigStatus::Written)
}

fn read_toml_config(path: &Path) -> Result<toml::Value> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read '{}'", path.display()))?;
    toml::from_str(&content).with_context(|| {
        format!(
            "refusing to overwrite invalid TOML in '{}'; fix the file and rerun setup",
            path.display()
        )
    })
}

fn read_toml_config_or_default(path: &Path) -> Result<toml::Value> {
    if path.exists() {
        read_toml_config(path)
    } else {
        Ok(toml::Value::Table(TomlTable::new()))
    }
}

fn toml_table_entry<'a>(
    table: &'a mut TomlTable,
    key: &str,
    path: &Path,
) -> Result<&'a mut TomlTable> {
    let value = table
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(TomlTable::new()));
    value.as_table_mut().with_context(|| {
        format!(
            "refusing to overwrite '{}' in '{}'; expected a TOML table",
            key,
            path.display()
        )
    })
}

fn write_toml_config(path: &Path, config: &toml::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let rendered = toml::to_string_pretty(config)?;
    fs::write(path, format!("{rendered}\n"))?;
    Ok(())
}

fn write_agent_file(path: &Path, content: &str) -> Result<bool> {
    // If file exists, check if it already contains Packet28 guidance
    if path.exists() {
        let existing = fs::read_to_string(path)?;
        if existing.contains("packet28.write_intention")
            || existing.contains("packet28.prepare_handoff")
            || existing.contains("Packet28 mcp serve")
            || existing.contains("hook claude")
        {
            return Ok(false); // already has Packet28 instructions
        }

        // Append to existing file
        let separator = if existing.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        fs::write(path, format!("{existing}{separator}{content}\n"))?;
        return Ok(true);
    }

    // Write new file
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{content}\n"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use packet28_daemon_core::{DaemonIndexManifest, DaemonIndexStatusResponse};
    use tempfile::tempdir;

    fn runtime(
        name: &'static str,
        slug: &'static str,
        detected: bool,
        has_mcp: bool,
    ) -> RuntimeInfo {
        RuntimeInfo {
            kind: match slug {
                "claude" => RuntimeKind::Claude,
                "cursor" => RuntimeKind::Cursor,
                "codex" => RuntimeKind::Codex,
                "windsurf" => RuntimeKind::Windsurf,
                "copilot" => RuntimeKind::Copilot,
                "gemini" => RuntimeKind::Gemini,
                "opencode" => RuntimeKind::OpenCode,
                "hermes" => RuntimeKind::Hermes,
                "cline" => RuntimeKind::Cline,
                "roo" => RuntimeKind::Roo,
                "kilocode" => RuntimeKind::KiloCode,
                "antigravity" => RuntimeKind::Antigravity,
                other => panic!("unknown runtime slug: {other}"),
            },
            name,
            slug,
            prompt_targets: has_mcp
                .then(|| PromptTarget {
                    path: PathBuf::from(format!("{slug}.md")),
                    format: agent_surface::AgentPromptFormat::Agents,
                })
                .into_iter()
                .collect(),
            detected,
        }
    }

    #[test]
    fn select_setup_runtimes_prefers_detected_runtimes_for_all() {
        let runtimes = vec![
            runtime("Claude Code", "claude", false, true),
            runtime("Cursor", "cursor", false, true),
            runtime("Codex", "codex", true, false),
            runtime("Windsurf", "windsurf", true, false),
        ];
        let choice = SetupPlanChoice {
            mode: SetupMode::Recommended,
            runtime_scope: SetupRuntimeScope::Detected,
            fallback_only: false,
        };

        let selected = select_setup_runtimes(&runtimes, &choice);
        let slugs: Vec<&str> = selected.iter().map(|runtime| runtime.slug).collect();

        assert_eq!(slugs, vec!["codex", "windsurf"]);
    }

    #[test]
    fn select_setup_runtimes_supports_all_and_single_scopes() {
        let runtimes = vec![
            runtime("Claude Code", "claude", false, true),
            runtime("Cursor", "cursor", true, true),
        ];
        let all_choice = SetupPlanChoice {
            mode: SetupMode::Custom,
            runtime_scope: SetupRuntimeScope::All,
            fallback_only: false,
        };
        let single_choice = SetupPlanChoice {
            mode: SetupMode::Custom,
            runtime_scope: SetupRuntimeScope::Single("claude".to_string()),
            fallback_only: false,
        };

        let all_selected = select_setup_runtimes(&runtimes, &all_choice);
        let all_slugs: Vec<&str> = all_selected.iter().map(|runtime| runtime.slug).collect();
        let single_selected = select_setup_runtimes(&runtimes, &single_choice);
        let single_slugs: Vec<&str> = single_selected.iter().map(|runtime| runtime.slug).collect();

        assert_eq!(all_slugs, vec!["claude", "cursor"]);
        assert_eq!(single_slugs, vec!["claude"]);
    }

    #[test]
    fn explicit_setup_choice_maps_default_flags_to_recommended() {
        let runtimes = vec![
            runtime("Claude Code", "claude", true, true),
            runtime("Codex", "codex", true, true),
        ];
        let args = SetupArgs {
            root: ".".to_string(),
            yes: true,
            fallback_only: false,
            runtime: "all".to_string(),
        };

        let choice = explicit_setup_choice(&args, &runtimes).unwrap();

        assert_eq!(
            choice,
            SetupPlanChoice {
                mode: SetupMode::Recommended,
                runtime_scope: SetupRuntimeScope::Detected,
                fallback_only: false,
            }
        );
    }

    #[test]
    fn explicit_setup_choice_maps_runtime_override_to_custom_single_scope() {
        let runtimes = vec![runtime("Claude Code", "claude", false, true)];
        let args = SetupArgs {
            root: ".".to_string(),
            yes: false,
            fallback_only: false,
            runtime: "claude".to_string(),
        };

        let choice = explicit_setup_choice(&args, &runtimes).unwrap();

        assert_eq!(
            choice,
            SetupPlanChoice {
                mode: SetupMode::Custom,
                runtime_scope: SetupRuntimeScope::Single("claude".to_string()),
                fallback_only: false,
            }
        );
    }

    #[test]
    fn detect_runtimes_includes_instruction_only_parity_targets() {
        let root = tempdir().unwrap();
        let runtimes = detect_runtimes(root.path());
        let by_slug = runtimes
            .iter()
            .map(|runtime| (runtime.slug, runtime))
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(
            by_slug["copilot"].prompt_targets[0].path,
            root.path().join(".github").join("copilot-instructions.md")
        );
        assert_eq!(
            by_slug["gemini"].prompt_targets[0].path,
            root.path().join("GEMINI.md")
        );
        assert_eq!(
            by_slug["cline"].prompt_targets[0].path,
            root.path().join(".clinerules")
        );
        assert_eq!(
            by_slug["roo"].prompt_targets[0].path,
            root.path().join(".roo").join("rules").join("packet28.md")
        );
        assert_eq!(
            by_slug["kilocode"].prompt_targets[0].path,
            root.path()
                .join(".kilocode")
                .join("rules")
                .join("packet28-rules.md")
        );
        assert_eq!(
            by_slug["antigravity"].prompt_targets[0].path,
            root.path()
                .join(".agents")
                .join("rules")
                .join("antigravity-packet28-rules.md")
        );

        assert!(!runtime_supports_mcp(by_slug["copilot"].kind));
        assert!(runtime_supports_hooks(by_slug["copilot"].kind));
        assert!(!runtime_supports_mcp(by_slug["gemini"].kind));
        assert!(runtime_supports_hooks(by_slug["gemini"].kind));
        assert!(!runtime_supports_mcp(by_slug["opencode"].kind));
        assert!(runtime_supports_hooks(by_slug["opencode"].kind));
        assert!(!runtime_supports_mcp(by_slug["hermes"].kind));
        assert!(runtime_supports_hooks(by_slug["hermes"].kind));

        for slug in ["cline", "roo", "kilocode", "antigravity"] {
            assert!(!runtime_supports_mcp(by_slug[slug].kind));
            assert!(!runtime_supports_hooks(by_slug[slug].kind));
        }
    }

    #[test]
    fn write_claude_hook_config_installs_packet28_hooks() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude").join("settings.json");
        let status = write_claude_hook_config(&path, dir.path(), true).unwrap();
        assert!(matches!(status, McpConfigStatus::Written));
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        // Hooks should be at top-level event keys, not nested under "packet28".
        assert!(value["hooks"]["SessionStart"].is_array());
        assert!(value["hooks"]["PostToolUse"].is_array());
        assert!(value["hooks"]["PostToolUseFailure"].is_array());
        assert!(value["hooks"].get("packet28").is_none());
        assert_eq!(
            value["hooks"]["SessionStart"][0]["hooks"][0]["type"].as_str(),
            Some("command")
        );
        assert_eq!(
            value["hooks"]["UserPromptSubmit"][0]["hooks"][0]["type"].as_str(),
            Some("command")
        );
        assert_eq!(
            value["hooks"]["PreToolUse"][0]["hooks"][0]["type"].as_str(),
            Some("http")
        );
        assert_eq!(
            value["hooks"]["Stop"][0]["hooks"][0]["type"].as_str(),
            Some("http")
        );
        let http_url = value["hooks"]["PreToolUse"][0]["hooks"][0]["url"]
            .as_str()
            .unwrap();
        assert!(http_url.starts_with("http://127.0.0.1:"));
        assert_eq!(
            value["allowedHttpHookUrls"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>(),
            vec![http_url]
        );
    }

    #[test]
    fn generated_packet28_hook_command_exits_zero_when_binary_is_missing() {
        let dir = tempdir().unwrap();
        for runtime in ["claude", "cursor", "copilot", "gemini", "windsurf"] {
            let command = guarded_packet28_hook_command("/missing/Packet28", runtime, dir.path());
            assert!(command.contains(&format!(" hook {runtime} ")));
            assert!(command.contains("exit 0"));

            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(&command)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "generated {runtime} hook failed: status={:?} stderr={}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());
        }
    }

    #[test]
    fn write_claude_hook_config_replaces_legacy_command_hooks() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude").join("settings.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let command = resolve_packet28_cli_command();
        let root_arg = shell_escape(dir.path().display().to_string());
        let hook_command = format!("{command} hook claude --root \"{root_arg}\"");
        fs::write(
            &path,
            serde_json::to_string_pretty(&json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": hook_command}]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let status = write_claude_hook_config(&path, dir.path(), true).unwrap();
        assert!(matches!(status, McpConfigStatus::Written));
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["hooks"][0]["type"].as_str(), Some("http"));
    }

    #[test]
    fn write_claude_hook_config_removes_stale_packet28_command_paths() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude").join("settings.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            serde_json::to_string_pretty(&json!({
                "hooks": {
                    "SessionStart": [
                        {
                            "matcher": "startup|resume|clear|compact",
                            "hooks": [{"type": "command", "command": "/missing/Packet28 hook claude --root \"/tmp/demo\""}]
                        },
                        {
                            "matcher": "startup|resume|clear|compact",
                            "hooks": [{"type": "command", "command": "/other/tool"}]
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let status = write_claude_hook_config(&path, dir.path(), true).unwrap();
        assert!(matches!(status, McpConfigStatus::Written));

        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let entries = value["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        let commands = entries
            .iter()
            .filter_map(|entry| entry["hooks"][0]["command"].as_str())
            .collect::<Vec<_>>();
        assert!(commands
            .iter()
            .any(|command| command.contains(" hook claude ")));
        assert!(commands.contains(&"/other/tool"));
        assert!(!commands
            .iter()
            .any(|command| command.starts_with("/missing/Packet28")));
    }

    #[test]
    fn write_cursor_hook_config_installs_packet28_hooks() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".cursor").join("hooks.json");
        let status = write_cursor_hook_config(&path, dir.path(), true).unwrap();
        assert!(matches!(status, McpConfigStatus::Written));
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value["hooks"]["beforeSubmitPrompt"].is_array());
        assert!(value["hooks"]["beforeShellExecution"].is_array());
        assert!(value["hooks"]["afterShellExecution"].is_array());
        assert!(value["hooks"]["stop"].is_array());
    }

    #[test]
    fn write_gemini_hook_config_installs_packet28_before_tool_hook() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".gemini").join("settings.json");
        let status = write_gemini_hook_config(&path, dir.path(), true).unwrap();
        assert!(matches!(status, McpConfigStatus::Written));
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let hooks = value["hooks"]["BeforeTool"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["matcher"].as_str(), Some("run_shell_command"));
        let command = hooks[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.contains(" hook gemini "));
    }

    #[test]
    fn write_copilot_hook_config_installs_packet28_pretool_hook() {
        let dir = tempdir().unwrap();
        let path = dir
            .path()
            .join(".github")
            .join("hooks")
            .join("packet28-rewrite.json");
        let status = write_copilot_hook_config(&path, dir.path(), true).unwrap();
        assert!(matches!(status, McpConfigStatus::Written));
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let hooks = value["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["type"].as_str(), Some("command"));
        assert_eq!(hooks[0]["timeout"].as_i64(), Some(5));
        let command = hooks[0]["command"].as_str().unwrap();
        assert!(command.contains(" hook copilot "));
    }

    #[test]
    fn write_opencode_plugin_installs_packet28_rewrite_plugin() {
        let dir = tempdir().unwrap();
        let path = dir
            .path()
            .join(".config")
            .join("opencode")
            .join("plugins")
            .join("packet28.ts");
        let status = write_opencode_plugin(&path, true).unwrap();
        assert!(matches!(status, McpConfigStatus::Written));
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("Packet28 rewrite"));
        assert!(content.contains("tool.execute.before"));
        assert!(content.contains("args as Record<string, unknown>).command = rewritten"));

        let status = write_opencode_plugin(&path, true).unwrap();
        assert!(matches!(status, McpConfigStatus::AlreadyConfigured));
    }

    #[test]
    fn opencode_plugin_smoke_rewrites_and_passes_through_empty_stdout() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }

        let dir = tempdir().unwrap();
        let path = dir
            .path()
            .join(".config")
            .join("opencode")
            .join("plugins")
            .join("packet28.ts");
        write_opencode_plugin(&path, true).unwrap();
        let script = r#"
const fs = require("fs")
let code = fs.readFileSync(process.argv[1], "utf8")
code = code.replace(/^import type .*$/m, "")
code = code.replace("export const Packet28OpenCodePlugin: Plugin =", "const Packet28OpenCodePlugin =")
code = code.replaceAll("(args as Record<string, unknown>)", "args")
code += `
;(async () => {
  const calls = []
  function $(strings, ...values) {
    const rendered = strings.reduce((acc, part, index) => acc + part + (index < values.length ? values[index] : ""), "")
    calls.push({ rendered, values })
    return {
      quiet() { return this },
      nothrow() {
        const command = String(values[0] ?? "")
        if (command === "git status --short") return Promise.resolve({ stdout: "rewritten git status\\n" })
        return Promise.resolve({ stdout: "" })
      },
      then(resolve) { resolve({ stdout: "" }) },
    }
  }
  const plugin = await Packet28OpenCodePlugin({ $ })
  const rewriteArgs = { command: "git status --short" }
  const passthroughArgs = { command: "htop" }
  await plugin["tool.execute.before"]({ tool: "bash" }, { args: rewriteArgs })
  await plugin["tool.execute.before"]({ tool: "shell" }, { args: passthroughArgs })
  console.log(rewriteArgs.command)
  console.log(passthroughArgs.command)
})().catch((err) => { console.error(err); process.exit(1) })
`
eval(code)
"#;
        let output = std::process::Command::new("node")
            .arg("-e")
            .arg(script)
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "node smoke failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "rewritten git status\nhtop\n"
        );
    }

    #[test]
    fn write_hermes_plugin_installs_plugin_and_enables_config() {
        let dir = tempdir().unwrap();
        let status = write_hermes_plugin(dir.path(), true).unwrap();
        assert!(matches!(status, McpConfigStatus::Written));

        let plugin_dir = hermes::plugin_dir(dir.path());
        let init = fs::read_to_string(plugin_dir.join("__init__.py")).unwrap();
        let manifest = fs::read_to_string(plugin_dir.join("plugin.yaml")).unwrap();
        let config = fs::read_to_string(hermes::config_path(dir.path())).unwrap();
        assert!(init.contains("Packet28 rewrite"));
        assert!(manifest.contains("packet28-rewrite"));
        assert!(hermes_config_enables_packet28(&config).unwrap());

        let status = write_hermes_plugin(dir.path(), true).unwrap();
        assert!(matches!(status, McpConfigStatus::AlreadyConfigured));
    }

    #[test]
    #[cfg(unix)]
    fn hermes_plugin_smoke_rewrites_and_passes_through_empty_stdout() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }

        let dir = tempdir().unwrap();
        write_hermes_plugin(dir.path(), true).unwrap();
        let init = hermes::plugin_dir(dir.path()).join("__init__.py");
        let script = r#"
import importlib.util
import subprocess
import sys
spec = importlib.util.spec_from_file_location("packet28_rewrite", sys.argv[1])
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
class FakeResult:
    def __init__(self, stdout="", stderr="", returncode=0):
        self.stdout = stdout
        self.stderr = stderr
        self.returncode = returncode
def fake_run(argv, **kwargs):
    assert argv[0:2] == ["Packet28", "rewrite"]
    if argv[2] == "git status --short":
        return FakeResult("rewritten git status\n")
    return FakeResult("")
mod.subprocess.run = fake_run
rewrite_args = {"command": "git status --short"}
mod._pre_tool_call(tool_name="terminal", args=rewrite_args)
passthrough_args = {"command": "htop"}
mod._pre_tool_call(tool_name="terminal", args=passthrough_args)
print(rewrite_args["command"])
print(passthrough_args["command"])
"#;
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(init)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "python smoke failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "rewritten git status\nhtop\n"
        );
    }

    #[test]
    fn patch_hermes_config_preserves_existing_enabled_plugins() {
        let config = patch_hermes_config(
            r#"
theme: dark
plugins:
  enabled:
    - existing-plugin
"#,
        )
        .unwrap();
        assert!(config.contains("existing-plugin"));
        assert!(hermes_config_enables_packet28(&config).unwrap());
    }

    #[test]
    fn write_windsurf_hook_config_installs_packet28_hooks() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".windsurf").join("hooks.json");
        let status = write_windsurf_hook_config(&path, dir.path(), true).unwrap();
        assert!(matches!(status, McpConfigStatus::Written));
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value["hooks"]["pre_user_prompt"].is_array());
        assert!(value["hooks"]["pre_run_command"].is_array());
        assert!(value["hooks"]["post_run_command"].is_array());
        assert!(value["hooks"]["post_cascade_response"].is_array());
    }

    #[test]
    fn generated_relaunch_is_disabled_when_packet28_agent_is_missing() {
        let mut config = packet28_daemon_core::HookRuntimeConfig {
            relaunch_preference: RelaunchPreference::DaemonManaged,
            relaunch_command: vec![
                "packet28-agent".to_string(),
                "--wait-for-handoff".to_string(),
            ],
            ..packet28_daemon_core::HookRuntimeConfig::default()
        };
        let changed = apply_generated_relaunch_command(&mut config, Path::new("/tmp/repo"), None);
        assert!(changed);
        assert_eq!(config.relaunch_preference, RelaunchPreference::HostManaged);
        assert!(config.relaunch_command.is_empty());
    }

    #[test]
    fn generated_relaunch_preserves_custom_commands() {
        let original = vec!["custom-agent-runner".to_string(), "--resume".to_string()];
        let mut config = packet28_daemon_core::HookRuntimeConfig {
            relaunch_preference: RelaunchPreference::DaemonManaged,
            relaunch_command: original.clone(),
            ..packet28_daemon_core::HookRuntimeConfig::default()
        };
        let changed = apply_generated_relaunch_command(&mut config, Path::new("/tmp/repo"), None);
        assert!(!changed);
        assert_eq!(config.relaunch_command, original);
    }

    #[test]
    fn generated_relaunch_uses_packet28_agent_when_available() {
        let mut config = packet28_daemon_core::HookRuntimeConfig::default();
        let changed = apply_generated_relaunch_command(
            &mut config,
            Path::new("/tmp/repo"),
            Some("/usr/local/bin/packet28-agent".to_string()),
        );
        assert!(changed);
        assert_eq!(
            config.relaunch_preference,
            RelaunchPreference::DaemonManaged
        );
        assert_eq!(
            config.relaunch_command,
            generated_relaunch_command("/usr/local/bin/packet28-agent", Path::new("/tmp/repo"))
        );
    }

    fn setup_index_status(
        status: &str,
        regex_status: Option<&str>,
        ready: bool,
    ) -> DaemonIndexStatusResponse {
        DaemonIndexStatusResponse {
            manifest: DaemonIndexManifest {
                status: status.to_string(),
                generation: 7,
                regex_generation: regex_status.map(|_| 7),
                regex_status: regex_status.map(str::to_string),
                regex_weight_table_version: regex_status.map(|_| 1),
                ..DaemonIndexManifest::default()
            },
            ready,
            ..DaemonIndexStatusResponse::default()
        }
    }

    #[test]
    fn classify_setup_index_status_reports_ready_when_regex_index_is_usable() {
        let dir = tempdir().unwrap();
        let regex_dir = dir.path().join(".packet28").join("index").join("regex-v1");
        fs::create_dir_all(&regex_dir).unwrap();
        fs::write(regex_dir.join("manifest.json"), "{}").unwrap();
        let response = setup_index_status("ready", Some("ready"), true);

        assert!(matches!(
            classify_setup_index_status(dir.path(), &response, false),
            SetupIndexVerification::Ready(_)
        ));
    }

    #[test]
    fn classify_setup_index_status_reports_building_while_index_is_in_progress() {
        let dir = tempdir().unwrap();
        let response = setup_index_status("building", Some("building"), false);

        assert!(matches!(
            classify_setup_index_status(dir.path(), &response, false),
            SetupIndexVerification::Building(_)
        ));
    }

    #[test]
    fn classify_setup_index_status_reports_failure_when_regex_artifacts_are_missing_after_timeout()
    {
        let dir = tempdir().unwrap();
        let response = setup_index_status("building", Some("building"), false);

        match classify_setup_index_status(dir.path(), &response, true) {
            SetupIndexVerification::Failed { reason, .. } => {
                assert!(reason.contains("regex trigram index artifacts are missing"));
            }
            other => panic!("expected failed setup classification, got {other:?}"),
        }
    }

    #[test]
    fn classify_setup_index_status_reports_failure_when_repo_index_claims_ready_without_regex() {
        let dir = tempdir().unwrap();
        let response = setup_index_status("ready", Some("building"), false);

        match classify_setup_index_status(dir.path(), &response, false) {
            SetupIndexVerification::Failed { reason, .. } => {
                assert!(reason.contains("regex trigram index is not ready"));
            }
            other => panic!("expected failed setup classification, got {other:?}"),
        }
    }
}
