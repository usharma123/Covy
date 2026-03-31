use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use packet28_daemon_core::{
    DaemonIndexRebuildRequest, DaemonIndexStatusRequest, DaemonIndexStatusResponse, DaemonRequest,
    DaemonResponse, RelaunchPreference,
};
use serde_json::{json, Value};
use toml::value::Table as TomlTable;

use crate::agent_surface;

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

    /// Specific runtime to configure (claude, cursor, codex, all)
    #[arg(long, default_value = "all")]
    pub runtime: String,
}

struct RuntimeInfo {
    kind: RuntimeKind,
    name: &'static str,
    slug: &'static str,
    prompt_targets: Vec<PromptTarget>,
    detected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeKind {
    Claude,
    Cursor,
    Codex,
    Windsurf,
}

#[derive(Clone, Debug)]
struct PromptTarget {
    path: PathBuf,
    format: agent_surface::AgentPromptFormat,
}

enum McpConfigStatus {
    Written,
    AlreadyConfigured,
    Declined,
}

#[derive(Debug)]
enum SetupIndexVerification {
    Ready(DaemonIndexStatusResponse),
    Building(DaemonIndexStatusResponse),
    Failed {
        response: Option<DaemonIndexStatusResponse>,
        reason: String,
    },
}

const SETUP_INDEX_VERIFY_TIMEOUT: Duration = Duration::from_secs(10);
const SETUP_INDEX_VERIFY_POLL: Duration = Duration::from_millis(100);
const REGEX_INDEX_DIR_NAME: &str = "regex-v1";
const REGEX_INDEX_MANIFEST_FILE: &str = "manifest.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupMode {
    Recommended,
    Custom,
    GuidanceOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SetupRuntimeScope {
    Detected,
    All,
    Single(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SetupPlanChoice {
    mode: SetupMode,
    runtime_scope: SetupRuntimeScope,
    fallback_only: bool,
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

    println!("  {}", "Step 1/4 Configure Runtime Integrations".bold());
    if !setup_choice.fallback_only {
        if !selected_runtimes
            .iter()
            .copied()
            .any(|runtime| runtime_supports_mcp(runtime.kind))
        {
            println!(
                "  {} No MCP-capable runtimes selected. Generating fallback files.",
                "→".yellow()
            );
            println!();
        } else {
            println!("    {}", "MCP servers:".bold());
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
        println!("    {}", "Runtime hooks:".bold());
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

    println!("  {}", "Step 2/4 Write Instruction Files".bold());
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
                    "    {} {} → {}",
                    "✓".green().bold(),
                    "generic",
                    generic_path.display().to_string().dimmed()
                );
            } else {
                println!("    {} {} (already up to date)", "·".dimmed(), "generic");
            }
        } else {
            println!("    {} no runtime instruction files selected", "·".dimmed());
        }
    }
    println!();

    println!("  {}", "Step 3/4 Start Daemon And Build Indexes".bold());
    println!("    {}", "Daemon:".bold());
    match crate::cmd_daemon::ensure_daemon(&root) {
        Ok(_) => {
            println!("    {} daemon running", "✓".green().bold());
            println!("    {}", "Repo index:".bold());
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

    println!("  {}", "Step 4/4 Summary".bold());
    if exit_code == 0 {
        println!("    {}", "Setup complete.".green().bold());
    } else {
        println!("    {}", "Setup finished with errors.".red().bold());
    }
    println!();
    println!("  {}", "What Changed".bold());

    if mcp_configured && !setup_choice.fallback_only {
        println!("    Your agent runtimes are configured to use Packet28 control-plane MCP tools.");
        if hook_configured {
            println!(
                "    Selected runtime hooks will capture tool activity directly into Packet28."
            );
        }
        println!("    Start a new session and Packet28 intent/handoff tools will be available.");
    } else if agent_files_ready {
        println!("    Agent instruction files have been written.");
        println!("    Include them in your agent's context or system prompt.");
    } else {
        println!("    No runtime artifacts were written.");
        println!("    Re-run setup and select a runtime to configure.");
    }

    println!();
    println!("  {}", "Verify With".dimmed().bold());
    println!("    packet28 --version");
    println!("    packet28 daemon status --root {root_display}");
    println!("    packet28 doctor --root {root_display}");
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
            "unknown runtime '{}'. Use: claude, cursor, codex, windsurf, or all",
            args.runtime
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

fn prompt_setup_choice(runtimes: &[RuntimeInfo]) -> Result<Option<SetupPlanChoice>> {
    println!("  {}", "Choose Setup Path".bold());
    println!("    1. Recommended   Configure detected runtimes with Packet28 defaults.");
    println!("    2. Advanced      Pick the runtime scope before applying setup.");
    println!("    3. Guidance only Write instruction files without MCP integration.");
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
    println!("  {}", "Advanced Runtime Scope".bold());
    println!("    1. Detected runtimes only");
    println!("    2. All supported runtimes");
    println!("    3. Single runtime");
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
    println!("  {}", "Choose A Runtime".bold());
    for (idx, runtime) in runtimes.iter().enumerate() {
        let availability = if runtime.detected {
            "detected".green().bold()
        } else {
            "not found".dimmed()
        };
        println!("    {}. {:<12} {}", idx + 1, runtime.name, availability);
    }
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
            "  {} enter a number between 1 and {max}, or 'q' to cancel.",
            "hint:".cyan().bold()
        );
    }
}

fn render_setup_intro(
    root_display: &str,
    runtimes: &[RuntimeInfo],
    selected_runtimes: &[&RuntimeInfo],
    setup_choice: &SetupPlanChoice,
    auto_yes: bool,
) -> Result<bool> {
    println!();
    println!(
        "{}",
        "  Packet28 Setup Wizard  ".bold().white().on_bright_blue()
    );
    println!();
    println!("  Workspace  {}", root_display.cyan());
    println!(
        "  Mode       {}",
        setup_mode_title(setup_choice.mode).bold()
    );
    println!(
        "  Scope      {}",
        setup_runtime_scope_label(setup_choice).bold()
    );
    println!("  {}", setup_mode_summary(setup_choice).dimmed());
    println!();

    println!("  {}", "Detected Agents".bold());
    for rt in runtimes {
        let availability = if rt.detected {
            "detected".green().bold()
        } else {
            "not found".dimmed()
        };
        let selection = if selected_runtimes
            .iter()
            .any(|candidate| candidate.slug == rt.slug)
        {
            Some("selected".cyan().bold())
        } else if rt.detected {
            Some("available".dimmed())
        } else {
            None
        };
        if let Some(selection) = selection {
            println!("    {:<12} {}  {}", rt.name, availability, selection);
        } else {
            println!("    {:<12} {}", rt.name, availability);
        }
    }
    println!();

    println!("  {}", "This Setup Will".bold());
    for item in build_setup_plan(selected_runtimes, setup_choice.fallback_only) {
        println!("    {item}");
    }
    println!();

    if auto_yes {
        println!(
            "  {}",
            "Running non-interactively with the planned defaults.".dimmed()
        );
        println!();
        return Ok(true);
    }

    eprint!("  {} ", setup_mode_prompt(setup_choice.mode));
    io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).ok();
    let trimmed = input.trim().to_lowercase();
    println!();
    Ok(trimmed.is_empty() || trimmed == "y" || trimmed == "yes")
}

fn setup_mode_title(mode: SetupMode) -> &'static str {
    match mode {
        SetupMode::Recommended => "Recommended",
        SetupMode::Custom => "Custom",
        SetupMode::GuidanceOnly => "Guidance Only",
    }
}

fn setup_mode_summary(choice: &SetupPlanChoice) -> &'static str {
    match choice.mode {
        SetupMode::Recommended => {
            "Opinionated default setup for the detected runtimes in this workspace."
        }
        SetupMode::Custom => {
            "Advanced path with explicit runtime targeting before Packet28 writes any files."
        }
        SetupMode::GuidanceOnly => {
            "Skip MCP integration and only write the instruction files needed for manual setup."
        }
    }
}

fn setup_runtime_scope_label(choice: &SetupPlanChoice) -> String {
    match &choice.runtime_scope {
        SetupRuntimeScope::Detected => "Detected runtimes".to_string(),
        SetupRuntimeScope::All => "All supported runtimes".to_string(),
        SetupRuntimeScope::Single(slug) => format!("Single runtime ({slug})"),
    }
}

fn setup_mode_prompt(mode: SetupMode) -> &'static str {
    match mode {
        SetupMode::Recommended => {
            "Continue with the recommended setup for detected runtimes? [Y/n]"
        }
        SetupMode::Custom => "Continue with this custom setup plan? [Y/n]",
        SetupMode::GuidanceOnly => "Continue with the guidance-only setup plan? [Y/n]",
    }
}

fn build_setup_plan(selected_runtimes: &[&RuntimeInfo], fallback_only: bool) -> Vec<String> {
    let mut items = Vec::new();
    let mut step_number = 1usize;
    let mcp_targets = selected_runtimes
        .iter()
        .copied()
        .filter(|runtime| runtime_supports_mcp(runtime.kind))
        .collect::<Vec<_>>();
    let hook_targets = selected_runtimes
        .iter()
        .copied()
        .filter(|runtime| runtime_supports_hooks(runtime.kind))
        .collect::<Vec<_>>();

    if fallback_only {
        items.push(format!("{step_number}. Skip MCP and hook writes."));
    } else if mcp_targets.is_empty() {
        items.push(format!(
            "{step_number}. No MCP-capable runtimes are selected, so setup will fall back to agent files."
        ));
    } else {
        items.push(format!(
            "{step_number}. Configure Packet28 MCP for {}.",
            format_runtime_names(&mcp_targets)
        ));
    }
    step_number += 1;

    if !fallback_only {
        if hook_targets.is_empty() {
            items.push(format!(
                "{step_number}. Skip runtime hook installation because no supported runtimes are selected."
            ));
        } else {
            items.push(format!(
                "{step_number}. Install Packet28 runtime hooks for {}.",
                format_runtime_names(&hook_targets)
            ));
        }
        step_number += 1;
    }

    if selected_runtimes.is_empty() {
        items.push(format!(
            "{step_number}. Write a generic agent instruction file into the workspace."
        ));
    } else {
        items.push(format!(
            "{step_number}. Write agent instruction files for {}.",
            format_runtime_names(selected_runtimes)
        ));
    }
    step_number += 1;
    items.push(format!(
        "{step_number}. Ensure packet28d is running and build the repo and regex indexes."
    ));
    items
}

fn format_runtime_names(runtimes: &[&RuntimeInfo]) -> String {
    if runtimes.is_empty() {
        return "no runtimes".to_string();
    }
    runtimes
        .iter()
        .map(|runtime| runtime.name)
        .collect::<Vec<_>>()
        .join(", ")
}

fn verify_setup_index(root: &Path) -> Result<SetupIndexVerification> {
    let start = Instant::now();
    let mut rendered_progress = None::<String>;
    let first_response = fetch_index_status(root)?;
    match classify_setup_index_status(root, &first_response, false) {
        SetupIndexVerification::Ready(_) | SetupIndexVerification::Failed { .. } => {
            return Ok(classify_setup_index_status(root, &first_response, false));
        }
        SetupIndexVerification::Building(_) => {
            render_setup_index_progress_line(&first_response, &mut rendered_progress)?;
        }
    }
    let mut last_response = first_response;
    loop {
        if start.elapsed() >= SETUP_INDEX_VERIFY_TIMEOUT {
            break;
        }
        std::thread::sleep(SETUP_INDEX_VERIFY_POLL);
        let response = fetch_index_status(root)?;
        match classify_setup_index_status(root, &response, false) {
            SetupIndexVerification::Ready(_) | SetupIndexVerification::Failed { .. } => {
                finish_setup_index_progress_line(&mut rendered_progress)?;
                return Ok(classify_setup_index_status(root, &response, false));
            }
            SetupIndexVerification::Building(_) => {
                render_setup_index_progress_line(&response, &mut rendered_progress)?;
                last_response = response;
            }
        }
    }

    finish_setup_index_progress_line(&mut rendered_progress)?;
    Ok(classify_setup_index_status(root, &last_response, true))
}

fn render_setup_index_progress_line(
    response: &DaemonIndexStatusResponse,
    rendered_progress: &mut Option<String>,
) -> Result<()> {
    let line = format!(
        "    {} {}",
        "·".yellow().bold(),
        render_setup_index_progress(response)
    );
    if rendered_progress.as_deref() == Some(line.as_str()) {
        return Ok(());
    }

    let padding = rendered_progress
        .as_ref()
        .map(|previous| " ".repeat(previous.len().saturating_sub(line.len())))
        .unwrap_or_default();
    print!("\r{line}{padding}");
    io::stdout().flush()?;
    *rendered_progress = Some(line);
    Ok(())
}

fn finish_setup_index_progress_line(rendered_progress: &mut Option<String>) -> Result<()> {
    if rendered_progress.is_some() {
        println!();
        io::stdout().flush()?;
    }
    *rendered_progress = None;
    Ok(())
}

fn render_setup_index_progress(response: &DaemonIndexStatusResponse) -> String {
    format!(
        "repo {}  regex {}",
        render_index_stage(
            response.manifest.status.as_str(),
            response.manifest.indexed_files,
            response.manifest.total_files,
        ),
        render_index_stage(
            response
                .manifest
                .regex_status
                .as_deref()
                .unwrap_or("missing"),
            response.manifest.regex_indexed_files,
            response.manifest.regex_total_files,
        ),
    )
}

fn render_index_stage(status: &str, indexed_files: usize, total_files: usize) -> String {
    match status {
        "queued" => "queued".to_string(),
        "missing" => "missing".to_string(),
        "ready" => {
            if total_files > 0 {
                format!("ready {}", render_progress_bar(total_files, total_files))
            } else {
                "ready".to_string()
            }
        }
        other => {
            if total_files > 0 {
                format!(
                    "{other} {}",
                    render_progress_bar(indexed_files.min(total_files), total_files)
                )
            } else {
                other.to_string()
            }
        }
    }
}

fn render_progress_bar(indexed_files: usize, total_files: usize) -> String {
    if total_files == 0 {
        return "[----------------] --% (0/0)".to_string();
    }
    let width = 16usize;
    let completed = indexed_files.min(total_files);
    let filled = completed.saturating_mul(width) / total_files;
    let percent = completed.saturating_mul(100) / total_files;
    format!(
        "[{}{}] {:>3}% ({}/{})",
        "#".repeat(filled),
        "-".repeat(width.saturating_sub(filled)),
        percent,
        completed,
        total_files
    )
}

fn fetch_index_status(root: &Path) -> Result<DaemonIndexStatusResponse> {
    match crate::cmd_daemon::send_request(
        root,
        &DaemonRequest::DaemonIndexStatus {
            request: DaemonIndexStatusRequest {
                root: root.display().to_string(),
            },
        },
    )? {
        DaemonResponse::DaemonIndexStatus { response } => Ok(response),
        DaemonResponse::Error { message } => anyhow::bail!(message),
        other => anyhow::bail!("unexpected index status response: {other:?}"),
    }
}

fn classify_setup_index_status(
    root: &Path,
    response: &DaemonIndexStatusResponse,
    timed_out: bool,
) -> SetupIndexVerification {
    if setup_index_ready(root, response) {
        return SetupIndexVerification::Ready(response.clone());
    }

    if let Some(reason) = setup_index_failure_reason(root, response, timed_out) {
        return SetupIndexVerification::Failed {
            response: Some(response.clone()),
            reason,
        };
    }

    SetupIndexVerification::Building(response.clone())
}

fn setup_index_ready(root: &Path, response: &DaemonIndexStatusResponse) -> bool {
    response.ready
        && response.manifest.status == "ready"
        && response.manifest.regex_status.as_deref() == Some("ready")
        && response.manifest.regex_generation.is_some()
        && response
            .manifest
            .regex_weight_table_version
            .unwrap_or_default()
            > 0
        && regex_index_artifacts_present(root)
}

fn setup_index_failure_reason(
    root: &Path,
    response: &DaemonIndexStatusResponse,
    timed_out: bool,
) -> Option<String> {
    let manifest = &response.manifest;
    if manifest.status == "corrupt" {
        return Some(
            manifest
                .last_error
                .clone()
                .unwrap_or_else(|| "repo index manifest is corrupt".to_string()),
        );
    }
    if manifest.regex_status.as_deref() == Some("corrupt") {
        return Some(
            manifest
                .regex_stale_reason
                .clone()
                .or_else(|| manifest.last_error.clone())
                .unwrap_or_else(|| "regex trigram index is corrupt".to_string()),
        );
    }
    if manifest.status == "ready" && manifest.regex_status.as_deref() != Some("ready") {
        return Some(
            manifest
                .regex_stale_reason
                .clone()
                .or_else(|| manifest.last_error.clone())
                .unwrap_or_else(|| "regex trigram index is not ready".to_string()),
        );
    }
    if timed_out && !regex_index_artifacts_present(root) {
        return Some("regex trigram index artifacts are missing".to_string());
    }
    if timed_out && manifest.status == "missing" && manifest.regex_status.is_none() {
        return Some("setup did not create the repo or regex search index".to_string());
    }
    None
}

fn regex_index_artifacts_present(root: &Path) -> bool {
    let dir = root
        .join(".packet28")
        .join("index")
        .join(REGEX_INDEX_DIR_NAME);
    dir.join(REGEX_INDEX_MANIFEST_FILE).is_file()
}

fn detect_runtimes(root: &Path) -> Vec<RuntimeInfo> {
    let home = dirs_home();
    vec![
        RuntimeInfo {
            kind: RuntimeKind::Claude,
            name: "Claude Code",
            slug: "claude",
            prompt_targets: vec![PromptTarget {
                path: root.join("CLAUDE.md"),
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
                path: root.join("AGENTS.md"),
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
    ]
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn detect_claude(home: &Path) -> bool {
    // Claude Code: ~/.claude/ directory or `claude` on PATH
    home.join(".claude").is_dir() || which_exists("claude")
}

fn detect_cursor(home: &Path) -> bool {
    // Cursor: ~/.cursor/ directory or cursor on PATH
    home.join(".cursor").is_dir() || which_exists("cursor")
}

fn detect_codex() -> bool {
    which_exists("codex")
}

fn detect_windsurf(home: &Path) -> bool {
    home.join(".codeium").join("windsurf").is_dir() || which_exists("windsurf")
}

fn which_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn find_claude_mcp_config(_home: &Path, root: &Path) -> Option<PathBuf> {
    // Claude Code uses project-level .mcp.json
    Some(root.join(".mcp.json"))
}

fn find_cursor_mcp_config(root: &Path) -> Option<PathBuf> {
    // Cursor uses project-level .cursor/mcp.json
    Some(root.join(".cursor").join("mcp.json"))
}

fn runtime_supports_mcp(kind: RuntimeKind) -> bool {
    matches!(
        kind,
        RuntimeKind::Claude | RuntimeKind::Cursor | RuntimeKind::Codex | RuntimeKind::Windsurf
    )
}

fn runtime_supports_hooks(kind: RuntimeKind) -> bool {
    matches!(
        kind,
        RuntimeKind::Claude | RuntimeKind::Cursor | RuntimeKind::Windsurf
    )
}

fn runtime_needs_hook_runtime_config(kind: RuntimeKind) -> bool {
    matches!(kind, RuntimeKind::Claude)
}

fn select_setup_runtimes<'a>(
    runtimes: &'a [RuntimeInfo],
    choice: &SetupPlanChoice,
) -> Vec<&'a RuntimeInfo> {
    match &choice.runtime_scope {
        SetupRuntimeScope::Detected => runtimes.iter().filter(|runtime| runtime.detected).collect(),
        SetupRuntimeScope::All => runtimes.iter().collect(),
        SetupRuntimeScope::Single(slug) => runtimes
            .iter()
            .filter(|runtime| runtime.slug == slug)
            .collect(),
    }
}

fn push_prompt_targets(targets: &mut Vec<PromptTarget>, additions: &[PromptTarget]) {
    for addition in additions {
        if targets.iter().any(|target| target.path == addition.path) {
            continue;
        }
        targets.push(addition.clone());
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
        RuntimeKind::Windsurf => {
            write_windsurf_hook_config(&hook_config_path(runtime.kind, root), root, auto_yes)
        }
        RuntimeKind::Codex => unreachable!("codex hooks are disabled in packet28 setup"),
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
    }
}

fn runtime_hook_status(runtime: &RuntimeInfo, root: &Path) -> String {
    match runtime.kind {
        RuntimeKind::Claude | RuntimeKind::Cursor | RuntimeKind::Windsurf => {
            hook_config_path(runtime.kind, root).display().to_string()
        }
        RuntimeKind::Codex => unreachable!("codex hooks are disabled in packet28 setup"),
    }
}

fn mcp_config_path(kind: RuntimeKind, root: &Path) -> PathBuf {
    match kind {
        RuntimeKind::Claude => find_claude_mcp_config(&dirs_home(), root).expect("claude mcp path"),
        RuntimeKind::Cursor => find_cursor_mcp_config(root).expect("cursor mcp path"),
        RuntimeKind::Codex => codex_config_path(&dirs_home()),
        RuntimeKind::Windsurf => windsurf_mcp_config_path(&dirs_home()),
    }
}

fn hook_config_path(kind: RuntimeKind, root: &Path) -> PathBuf {
    match kind {
        RuntimeKind::Claude => root.join(".claude").join("settings.json"),
        RuntimeKind::Cursor => root.join(".cursor").join("hooks.json"),
        RuntimeKind::Windsurf => root.join(".windsurf").join("hooks.json"),
        RuntimeKind::Codex => unreachable!("codex hooks are disabled in packet28 setup"),
    }
}

fn codex_config_path(home: &Path) -> PathBuf {
    home.join(".codex").join("config.toml")
}

fn windsurf_mcp_config_path(home: &Path) -> PathBuf {
    home.join(".codeium")
        .join("windsurf")
        .join("mcp_config.json")
}

fn cursor_prompt_targets(root: &Path) -> Vec<PromptTarget> {
    let mut targets = vec![PromptTarget {
        path: root.join(".cursor").join("rules").join("packet28.mdc"),
        format: agent_surface::AgentPromptFormat::CursorRule,
    }];
    if root.join(".cursorrules").exists() {
        targets.push(PromptTarget {
            path: root.join(".cursorrules"),
            format: agent_surface::AgentPromptFormat::Cursor,
        });
    }
    targets
}

fn windsurf_prompt_targets(root: &Path) -> Vec<PromptTarget> {
    vec![PromptTarget {
        path: root.join(".windsurf").join("rules").join("packet28.md"),
        format: agent_surface::AgentPromptFormat::WindsurfRule,
    }]
}

fn prompt_target_label(target: &PromptTarget) -> String {
    target
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| target.path.to_str().unwrap_or("prompt"))
        .to_string()
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
        "args": ["--root", root_arg]
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

fn write_claude_hook_config(path: &Path, root: &Path, auto_yes: bool) -> Result<McpConfigStatus> {
    let command = resolve_packet28_cli_command();
    let root_arg = shell_escape(root.display().to_string());
    let hook_command = format!("{command} hook claude --root \"{root_arg}\"");
    let packet28_hooks = json!({
        "SessionStart": [{
            "matcher": "startup|resume|clear|compact",
            "hooks": [{"type": "command", "command": hook_command}]
        }],
        "UserPromptSubmit": [{
            "matcher": ".*",
            "hooks": [{"type": "command", "command": hook_command}]
        }],
        "PreToolUse": [{
            "matcher": ".*",
            "hooks": [{"type": "command", "command": hook_command}]
        }],
        "PostToolUse": [{
            "matcher": ".*",
            "hooks": [{"type": "command", "command": hook_command}]
        }],
        "Stop": [{
            "matcher": ".*",
            "hooks": [{"type": "command", "command": hook_command}]
        }],
        "SubagentStop": [{
            "matcher": ".*",
            "hooks": [{"type": "command", "command": hook_command}]
        }],
        "PreCompact": [{
            "matcher": "manual|auto",
            "hooks": [{"type": "command", "command": hook_command}]
        }],
        "SessionEnd": [{
            "matcher": ".*",
            "hooks": [{"type": "command", "command": hook_command}]
        }]
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
    if !auto_yes {
        eprint!(
            "    Write Claude hook config to {}? [Y/n] ",
            path.display().to_string().dimmed()
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let trimmed = input.trim().to_lowercase();
        if !trimmed.is_empty() && trimmed != "y" && trimmed != "yes" {
            return Ok(McpConfigStatus::Declined);
        }
    }
    let mut hooks = config
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    // Claude Code expects hook event names (PreToolUse, Stop, etc.) as
    // direct keys under `hooks`. Merge our entries into each event key
    // rather than nesting under a "packet28" grouping key.
    let packet28_events = packet28_hooks.as_object().cloned().unwrap_or_default();
    let mut already_configured = true;
    for (event_name, entries) in &packet28_events {
        let existing = hooks
            .get(event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let new_entries = entries.as_array().cloned().unwrap_or_default();
        // Check if our hook command is already present in this event.
        let hook_present = new_entries
            .iter()
            .all(|new_entry| existing.iter().any(|ex| ex == new_entry));
        if !hook_present {
            already_configured = false;
            // Append our entries (don't overwrite user's existing hooks).
            let mut merged = existing;
            merged.extend(new_entries);
            hooks.insert(event_name.clone(), Value::Array(merged));
        }
    }
    // Remove legacy "packet28" grouping key if present.
    if hooks.contains_key("packet28") {
        hooks.remove("packet28");
        already_configured = false;
    }
    if already_configured {
        return Ok(McpConfigStatus::AlreadyConfigured);
    }
    config.insert("hooks".to_string(), Value::Object(hooks));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&config)?),
    )?;
    Ok(McpConfigStatus::Written)
}

fn write_cursor_hook_config(path: &Path, root: &Path, auto_yes: bool) -> Result<McpConfigStatus> {
    let command = resolve_packet28_cli_command();
    let root_arg = shell_escape(root.display().to_string());
    let hook_command = format!("{command} hook cursor --root \"{root_arg}\"");
    let packet28_hooks = json!({
        "beforeSubmitPrompt": [{
            "command": hook_command
        }],
        "beforeShellExecution": [{
            "command": hook_command
        }],
        "afterShellExecution": [{
            "command": hook_command
        }],
        "stop": [{
            "command": hook_command
        }]
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
    if !auto_yes {
        eprint!(
            "    Write Cursor hook config to {}? [Y/n] ",
            path.display().to_string().dimmed()
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let trimmed = input.trim().to_lowercase();
        if !trimmed.is_empty() && trimmed != "y" && trimmed != "yes" {
            return Ok(McpConfigStatus::Declined);
        }
    }
    let mut hooks = config
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let packet28_events = packet28_hooks.as_object().cloned().unwrap_or_default();
    let mut already_configured = true;
    for (event_name, entries) in &packet28_events {
        let existing = hooks
            .get(event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let new_entries = entries.as_array().cloned().unwrap_or_default();
        let hook_present = new_entries
            .iter()
            .all(|new_entry| existing.iter().any(|entry| entry == new_entry));
        if hook_present {
            continue;
        }
        already_configured = false;
        let mut merged = existing;
        merged.extend(new_entries);
        hooks.insert(event_name.clone(), Value::Array(merged));
    }
    if already_configured {
        return Ok(McpConfigStatus::AlreadyConfigured);
    }
    config.insert("hooks".to_string(), Value::Object(hooks));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&config)?),
    )?;
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
                == vec!["--root", expected_root.as_str()]
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
        "args": ["--root", root_arg]
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

fn write_windsurf_hook_config(path: &Path, root: &Path, auto_yes: bool) -> Result<McpConfigStatus> {
    let command = resolve_packet28_cli_command();
    let root_arg = shell_escape(root.display().to_string());
    let hook_command = format!("{command} hook windsurf --root \"{root_arg}\"");
    let packet28_hooks = json!({
        "pre_user_prompt": [{
            "command": hook_command
        }],
        "pre_run_command": [{
            "command": hook_command
        }],
        "post_run_command": [{
            "command": hook_command
        }],
        "post_cascade_response": [{
            "command": hook_command
        }]
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
    if !auto_yes {
        eprint!(
            "    Write Windsurf hook config to {}? [Y/n] ",
            path.display().to_string().dimmed()
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let trimmed = input.trim().to_lowercase();
        if !trimmed.is_empty() && trimmed != "y" && trimmed != "yes" {
            return Ok(McpConfigStatus::Declined);
        }
    }
    let mut hooks = config
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let packet28_events = packet28_hooks.as_object().cloned().unwrap_or_default();
    let mut already_configured = true;
    for (event_name, entries) in &packet28_events {
        let existing = hooks
            .get(event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let new_entries = entries.as_array().cloned().unwrap_or_default();
        let hook_present = new_entries
            .iter()
            .all(|new_entry| existing.iter().any(|entry| entry == new_entry));
        if hook_present {
            continue;
        }
        already_configured = false;
        let mut merged = existing;
        merged.extend(new_entries);
        hooks.insert(event_name.clone(), Value::Array(merged));
    }
    if already_configured {
        return Ok(McpConfigStatus::AlreadyConfigured);
    }
    config.insert("hooks".to_string(), Value::Object(hooks));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&config)?),
    )?;
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

fn write_hook_runtime_config(root: &Path, any_hooks_configured: bool) -> Result<McpConfigStatus> {
    if !any_hooks_configured {
        return Ok(McpConfigStatus::Declined);
    }
    let path = packet28_daemon_core::hook_runtime_config_path(root);
    let existed = path.exists();
    let mut config = if existed {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read '{}'", path.display()))?;
        serde_json::from_str::<packet28_daemon_core::HookRuntimeConfig>(&content).with_context(
            || format!("refusing to overwrite invalid JSON in '{}'", path.display()),
        )?
    } else {
        packet28_daemon_core::HookRuntimeConfig::default()
    };
    let changed =
        apply_generated_relaunch_command(&mut config, root, resolve_packet28_agent_command());
    if existed && !changed {
        return Ok(McpConfigStatus::AlreadyConfigured);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&config)?),
    )?;
    Ok(McpConfigStatus::Written)
}

fn apply_generated_relaunch_command(
    config: &mut packet28_daemon_core::HookRuntimeConfig,
    root: &Path,
    packet28_agent: Option<String>,
) -> bool {
    let should_manage_existing = config.relaunch_command.is_empty()
        || is_generated_relaunch_command(&config.relaunch_command);
    if !should_manage_existing {
        return false;
    }
    match packet28_agent {
        Some(packet28_agent) => {
            let desired_command = generated_relaunch_command(&packet28_agent, root);
            if config.relaunch_preference == RelaunchPreference::DaemonManaged
                && config.relaunch_command == desired_command
            {
                return false;
            }
            config.relaunch_preference = RelaunchPreference::DaemonManaged;
            config.relaunch_command = desired_command;
            true
        }
        None => {
            if config.relaunch_preference == RelaunchPreference::HostManaged
                && config.relaunch_command.is_empty()
            {
                return false;
            }
            config.relaunch_preference = RelaunchPreference::HostManaged;
            config.relaunch_command.clear();
            true
        }
    }
}

fn generated_relaunch_command(packet28_agent: &str, root: &Path) -> Vec<String> {
    vec![
        packet28_agent.to_string(),
        "--wait-for-handoff".to_string(),
        "--root".to_string(),
        root.display().to_string(),
        "--".to_string(),
        "claude".to_string(),
        "--continue".to_string(),
    ]
}

fn is_generated_relaunch_command(command: &[String]) -> bool {
    command
        .first()
        .map(|value| {
            Path::new(value)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(value)
                == "packet28-agent"
        })
        .unwrap_or(false)
}

fn resolve_packet28_agent_command() -> Option<String> {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_packet28-agent") {
        if !path.trim().is_empty() {
            return Some(path);
        }
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(parent) = current.parent() {
            let sibling = parent.join("packet28-agent");
            if sibling.exists() {
                return Some(sibling.display().to_string());
            }
        }
    }
    let output = std::process::Command::new("which")
        .arg("packet28-agent")
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            let command = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !command.is_empty() {
                return Some(command);
            }
        }
    }
    None
}

fn shell_escape(value: String) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
}

fn resolve_packet28_mcp_command() -> String {
    let output = std::process::Command::new("which")
        .arg("packet28-mcp")
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            let command = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !command.is_empty() {
                return command;
            }
        }
    }
    "packet28-mcp".to_string()
}

fn resolve_packet28_cli_command() -> String {
    for candidate in ["Packet28", "packet28"] {
        let output = std::process::Command::new("which").arg(candidate).output();
        if let Ok(output) = output {
            if output.status.success() {
                let command = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !command.is_empty() {
                    return command;
                }
            }
        }
    }
    "Packet28".to_string()
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
    fn write_claude_hook_config_installs_packet28_hooks() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude").join("settings.json");
        let status = write_claude_hook_config(&path, dir.path(), true).unwrap();
        assert!(matches!(status, McpConfigStatus::Written));
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        // Hooks should be at top-level event keys, not nested under "packet28".
        assert!(value["hooks"]["SessionStart"].is_array());
        assert!(value["hooks"]["PostToolUse"].is_array());
        assert!(value["hooks"].get("packet28").is_none());
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
