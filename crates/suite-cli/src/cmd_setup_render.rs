use std::io::{self, Write};

use anyhow::Result;
use colored::Colorize;

use crate::cmd_setup::{SetupMode, SetupPlanChoice, SetupRuntimeScope};
use crate::cmd_setup_runtime::RuntimeInfo;

const SETUP_BANNER_MIN_WIDTH: usize = 58;

#[derive(Clone, Copy)]
pub(crate) enum SetupBadgeStyle {
    Primary,
    Success,
    Warning,
    Muted,
}

pub(crate) fn render_setup_intro(
    root_display: &str,
    runtimes: &[RuntimeInfo],
    selected_runtimes: &[&RuntimeInfo],
    setup_choice: &SetupPlanChoice,
    auto_yes: bool,
) -> Result<bool> {
    render_setup_banner(
        "Packet28 Setup Plan",
        "Review the detected runtimes and the changes Packet28 is about to make.",
    );
    println!();
    render_setup_summary_row("Workspace", root_display.cyan().to_string());
    render_setup_summary_row(
        "Mode",
        setup_mode_title(setup_choice.mode).bold().to_string(),
    );
    render_setup_summary_row(
        "Scope",
        setup_runtime_scope_label(setup_choice).bold().to_string(),
    );
    render_setup_summary_row(
        "Detected",
        format!(
            "{}/{} runtimes on this machine",
            runtimes.iter().filter(|runtime| runtime.detected).count(),
            runtimes.len()
        )
        .to_string(),
    );
    println!("  {}", setup_mode_summary(setup_choice).dimmed());
    println!();

    render_setup_section("Runtime status");
    for rt in runtimes {
        let availability = format_runtime_detection_badge(rt.detected);
        let selection = if selected_runtimes
            .iter()
            .any(|candidate| candidate.slug == rt.slug)
        {
            Some(format_setup_badge("selected", SetupBadgeStyle::Primary))
        } else if rt.detected {
            Some(format_setup_badge("detected", SetupBadgeStyle::Muted))
        } else {
            None
        };
        let detail = runtime_capability_summary(rt).dimmed();
        let mut status_bits = vec![availability];
        if let Some(selection) = selection {
            status_bits.push(selection);
        }
        println!(
            "    {:<12} {}  {}",
            rt.name.bold(),
            status_bits.join(" "),
            detail
        );
    }
    println!();

    render_setup_section("Planned changes");
    for (idx, item) in build_setup_plan(selected_runtimes, setup_choice.fallback_only)
        .into_iter()
        .enumerate()
    {
        println!("    {}. {item}", idx + 1);
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

pub(crate) fn render_setup_banner(title: &str, subtitle: &str) {
    let brand = "Packet28 Setup";
    let content_width = [
        brand.len(),
        title.len(),
        subtitle.len(),
        SETUP_BANNER_MIN_WIDTH,
    ]
    .into_iter()
    .max()
    .unwrap_or(SETUP_BANNER_MIN_WIDTH);
    let border = format!("  +{}+", "-".repeat(content_width + 2)).cyan();
    println!();
    println!("{border}");
    render_setup_box_line(brand, content_width, Some("brand"));
    render_setup_box_line(title, content_width, None);
    render_setup_box_line(subtitle, content_width, Some("subtitle"));
    println!("{border}");
}

pub(crate) fn render_setup_menu_option(
    index: usize,
    title: &str,
    badge: &str,
    description: &str,
    is_default: bool,
) {
    let mut meta = vec![format_setup_badge(badge, SetupBadgeStyle::Warning)];
    if is_default {
        meta.push(format_setup_badge("default", SetupBadgeStyle::Success));
    }
    println!("    {} {}", setup_menu_index_badge(index), title.bold());
    println!("        {}", meta.join(" "));
    println!("        {}", description.dimmed());
    println!();
}

pub(crate) fn render_setup_menu_hint() {
    println!(
        "  {}",
        "Press Enter to accept the default option, or type 'q' to cancel.".dimmed()
    );
}

pub(crate) fn setup_menu_index_badge(index: usize) -> String {
    format_setup_badge(&index.to_string(), SetupBadgeStyle::Primary)
}

fn render_setup_summary_row(label: &str, value: String) {
    println!("  {:<10} {}", format!("{label}:").bold(), value);
}

pub(crate) fn format_runtime_detection_badge(detected: bool) -> String {
    if detected {
        format_setup_badge("detected", SetupBadgeStyle::Success)
    } else {
        format_setup_badge("not found", SetupBadgeStyle::Muted)
    }
}

fn render_setup_box_line(text: &str, width: usize, tone: Option<&str>) {
    let padding = " ".repeat(width.saturating_sub(text.len()));
    let styled = match tone {
        Some("brand") => text.bold().white().on_bright_blue().to_string(),
        Some("subtitle") => text.dimmed().to_string(),
        _ => text.bold().to_string(),
    };
    println!("  | {}{} |", styled, padding);
}

pub(crate) fn render_setup_detection_overview(runtimes: &[RuntimeInfo]) {
    let detected: Vec<&str> = runtimes
        .iter()
        .filter(|runtime| runtime.detected)
        .map(|runtime| runtime.name)
        .collect();
    let missing_count = runtimes.len().saturating_sub(detected.len());
    println!(
        "  {} {}",
        format_setup_badge(
            &format!("{} detected", detected.len()),
            if detected.is_empty() {
                SetupBadgeStyle::Muted
            } else {
                SetupBadgeStyle::Success
            }
        ),
        format_setup_badge(
            &format!("{missing_count} not found"),
            if missing_count == 0 {
                SetupBadgeStyle::Muted
            } else {
                SetupBadgeStyle::Warning
            }
        )
    );
    if !detected.is_empty() {
        println!(
            "  {}",
            format!("Available now: {}", detected.join(", ")).dimmed()
        );
    }
    println!();
}

pub(crate) fn render_setup_step(step: usize, total: usize, title: &str, subtitle: &str) {
    println!(
        "  {} {}",
        format_setup_badge(&format!("{step}/{total}"), SetupBadgeStyle::Primary),
        title.bold()
    );
    println!("      {}", subtitle.dimmed());
}

pub(crate) fn render_setup_section(title: &str) {
    println!("    {}", title.bold());
}

pub(crate) fn format_setup_badge(label: &str, style: SetupBadgeStyle) -> String {
    let badge = format!("[{label}]");
    match style {
        SetupBadgeStyle::Primary => badge.cyan().bold().to_string(),
        SetupBadgeStyle::Success => badge.green().bold().to_string(),
        SetupBadgeStyle::Warning => badge.yellow().bold().to_string(),
        SetupBadgeStyle::Muted => badge.dimmed().to_string(),
    }
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
        SetupMode::Recommended => "Apply the recommended setup plan? [Y/n]",
        SetupMode::Custom => "Apply this custom setup plan? [Y/n]",
        SetupMode::GuidanceOnly => "Apply this guidance-only setup plan? [Y/n]",
    }
}

fn build_setup_plan(selected_runtimes: &[&RuntimeInfo], fallback_only: bool) -> Vec<String> {
    let mut items = Vec::new();
    let mcp_targets = selected_runtimes
        .iter()
        .copied()
        .filter(|runtime| runtime.adapter.mcp.is_some())
        .collect::<Vec<_>>();
    let hook_targets = selected_runtimes
        .iter()
        .copied()
        .filter(|runtime| runtime.adapter.hooks.is_some())
        .collect::<Vec<_>>();

    if fallback_only {
        items.push("Skip MCP server configuration and runtime hook installation.".to_string());
    } else if mcp_targets.is_empty() {
        items.push(
            "No MCP-capable runtimes are selected, so setup will fall back to instruction files."
                .to_string(),
        );
    } else {
        items.push(format!(
            "Configure Packet28 MCP for {}.",
            format_runtime_names(&mcp_targets)
        ));
    }

    if !fallback_only {
        if hook_targets.is_empty() {
            items.push(
                "Skip runtime hook installation because no supported runtimes were selected."
                    .to_string(),
            );
        } else {
            items.push(format!(
                "Install Packet28 runtime hooks for {}.",
                format_runtime_names(&hook_targets)
            ));
        }
    }

    if selected_runtimes.is_empty() {
        items.push("Write a generic agent instruction file into the workspace.".to_string());
    } else {
        items.push(format!(
            "Write agent instruction files for {}.",
            format_runtime_names(selected_runtimes)
        ));
    }
    items.push("Ensure packet28d is running and build the repo and regex indexes.".to_string());
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

pub(crate) fn runtime_capability_summary(runtime: &RuntimeInfo) -> String {
    match (
        runtime.adapter.mcp.is_some(),
        runtime.adapter.hooks.is_some(),
    ) {
        (true, true) => "MCP + runtime hooks".to_string(),
        (true, false) => "MCP only".to_string(),
        (false, true) => "runtime hooks only".to_string(),
        (false, false) => "instruction files only".to_string(),
    }
}
