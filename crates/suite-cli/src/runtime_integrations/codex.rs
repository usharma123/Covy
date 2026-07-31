use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use colored::Colorize;
use toml::value::Table as TomlTable;

use super::{
    AgentPromptFormat, IntegrationAction, McpConfigStatus, PromptTarget, RuntimeAdapter,
    RuntimeEnvironment,
};
use crate::cmd_setup::setup_commands::resolve_packet28_mcp_command;
use crate::cmd_setup::{
    read_toml_config, read_toml_config_or_default, toml_table_entry, write_toml_config,
};

pub(crate) const ADAPTER: RuntimeAdapter = RuntimeAdapter {
    name: "Codex",
    slug: "codex",
    prompt_targets,
    detect,
    mcp: Some(IntegrationAction::new(
        configure_mcp,
        mcp_artifacts,
        mcp_status,
    )),
    hooks: None,
    writes_hook_runtime_config: false,
};

pub(crate) fn config_path(home: &Path) -> PathBuf {
    home.join(".codex").join("config.toml")
}

pub(crate) fn prompt_path(root: &Path) -> PathBuf {
    root.join("AGENTS.md")
}

fn prompt_targets(environment: &RuntimeEnvironment<'_>) -> Vec<PromptTarget> {
    vec![PromptTarget {
        path: prompt_path(environment.root()),
        format: AgentPromptFormat::Agents,
    }]
}

fn detect(environment: &RuntimeEnvironment<'_>) -> bool {
    environment.command_exists("codex")
}

fn configure_mcp(environment: &RuntimeEnvironment<'_>, auto_yes: bool) -> Result<McpConfigStatus> {
    let path = config_path(environment.home());
    if mcp_entry_matches(&path, environment.root())? {
        return Ok(McpConfigStatus::AlreadyConfigured);
    }
    if !auto_yes {
        eprint!(
            "    Register Packet28 MCP in Codex via {}? [Y/n] ",
            path.display().to_string().dimmed()
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let trimmed = input.trim().to_lowercase();
        if !trimmed.is_empty() && trimmed != "y" && trimmed != "yes" {
            return Ok(McpConfigStatus::Declined);
        }
    }
    if environment.command_exists("codex") {
        let args = vec![
            "mcp".to_string(),
            "add".to_string(),
            "packet28".to_string(),
            "--".to_string(),
            resolve_packet28_mcp_command(),
            "--root".to_string(),
            environment.root().display().to_string(),
            "--toolset".to_string(),
            "core".to_string(),
        ];
        if environment.run_command("codex", &args).unwrap_or(false)
            && mcp_entry_matches(&path, environment.root())?
        {
            return Ok(McpConfigStatus::Written);
        }
    }
    write_mcp_config(&path, environment.root())
}

fn mcp_artifacts(environment: &RuntimeEnvironment<'_>) -> Vec<PathBuf> {
    vec![config_path(environment.home())]
}

fn mcp_status(environment: &RuntimeEnvironment<'_>) -> String {
    format!(
        "{} → {}",
        ADAPTER.name,
        config_path(environment.home()).display()
    )
}

fn mcp_entry_matches(path: &Path, root: &Path) -> Result<bool> {
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

fn write_mcp_config(path: &Path, root: &Path) -> Result<McpConfigStatus> {
    let mut config = read_toml_config_or_default(path)?;
    let table = config
        .as_table_mut()
        .context("Codex config must be a TOML table")?;
    let servers = toml_table_entry(table, "mcp_servers", path)?;
    let desired_command = resolve_packet28_mcp_command();
    let desired_args = vec![
        toml::Value::String("--root".to_string()),
        toml::Value::String(root.display().to_string()),
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
