use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use colored::Colorize;

use super::McpConfigStatus;
use crate::runtime_integrations::hermes;

fn opencode_plugin_content() -> &'static str {
    r#"import type { Plugin } from "@opencode-ai/plugin"

// Packet28 OpenCode plugin - rewrites shell commands through Packet28.
// Requires: Packet28 in PATH.
//
// This is a thin delegating plugin. Rewrite policy lives in Packet28's
// route registry, so runtime behavior stays consistent with hooks and MCP.

export const Packet28OpenCodePlugin: Plugin = async ({ $ }) => {
  try {
    await $`Packet28 --version`.quiet()
  } catch {
    console.warn("[packet28] Packet28 binary not found in PATH - plugin disabled")
    return {}
  }

  return {
    "tool.execute.before": async (input, output) => {
      const tool = String(input?.tool ?? "").toLowerCase()
      if (tool !== "bash" && tool !== "shell") return
      const args = output?.args
      if (!args || typeof args !== "object") return

      const command = (args as Record<string, unknown>).command
      if (typeof command !== "string" || !command) return

      try {
        const result = await $`Packet28 rewrite ${command}`.quiet().nothrow()
        const rewritten = String(result.stdout).trim()
        if (rewritten && rewritten !== command) {
          ;(args as Record<string, unknown>).command = rewritten
        }
      } catch {
        // Packet28 rewrite failed - pass through unchanged.
      }
    },
  }
}
"#
}

pub(crate) fn write_opencode_plugin(path: &Path, auto_yes: bool) -> Result<McpConfigStatus> {
    let content = opencode_plugin_content();
    if path.exists() {
        let existing = fs::read_to_string(path)
            .with_context(|| format!("failed to read '{}'", path.display()))?;
        if existing == content || existing == format!("{content}\n") {
            return Ok(McpConfigStatus::AlreadyConfigured);
        }
    }
    if !auto_yes {
        eprint!(
            "    Write OpenCode plugin to {}? [Y/n] ",
            path.display().to_string().dimmed()
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let trimmed = input.trim().to_lowercase();
        if !trimmed.is_empty() && trimmed != "y" && trimmed != "yes" {
            return Ok(McpConfigStatus::Declined);
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(McpConfigStatus::Written)
}

fn hermes_plugin_init_content() -> &'static str {
    r#""""Hermes plugin adapter for Packet28 command rewriting."""

import shutil
import subprocess
import sys


ACCEPTED_REWRITE_RETURN_CODES = {0, 3}
EXPECTED_PASSTHROUGH_RETURN_CODES = {1, 2}
_packet28_available = None
_packet28_missing_warned = False


def register(ctx):
    """Register the Hermes pre-tool callback."""
    if not _check_packet28():
        return

    ctx.register_hook("pre_tool_call", _pre_tool_call)


def _check_packet28():
    """Return whether Packet28 is in PATH, warning once when missing."""
    global _packet28_available, _packet28_missing_warned

    if _packet28_available is None:
        _packet28_available = shutil.which("Packet28") is not None

    if not _packet28_available and not _packet28_missing_warned:
        _warn("Packet28 binary not found in PATH; Hermes hook not registered")
        _packet28_missing_warned = True

    return _packet28_available


def _pre_tool_call(tool_name=None, args=None, **_kwargs):
    """Rewrite mutable Hermes terminal command args when Packet28 provides a change."""
    try:
        if tool_name != "terminal" or not isinstance(args, dict):
            return

        command = args.get("command")
        if not isinstance(command, str) or not command.strip():
            return

        try:
            result = subprocess.run(
                ["Packet28", "rewrite", command],
                shell=False,
                timeout=2,
                capture_output=True,
                text=True,
            )
        except subprocess.TimeoutExpired:
            _warn("Packet28 rewrite timed out")
            return

        if result.returncode not in ACCEPTED_REWRITE_RETURN_CODES:
            if result.returncode not in EXPECTED_PASSTHROUGH_RETURN_CODES:
                details = f"Packet28 rewrite failed with exit {result.returncode}"
                stderr = result.stderr.strip()
                if stderr:
                    details = f"{details}: {stderr}"
                _warn(details)
            return

        rewritten = result.stdout.strip()
        if rewritten and rewritten != command:
            args["command"] = rewritten
    except Exception as e:
        _warn(str(e))
        return


def _warn(message):
    print(f"packet28: hermes plugin warning: {message}", file=sys.stderr)
"#
}

fn hermes_plugin_manifest_content() -> &'static str {
    r#"name: packet28-rewrite
version: "0.1.0"
description: Rewrite Hermes terminal commands through Packet28 before execution.
author: Packet28
hooks:
  - pre_tool_call
provides_hooks:
  - pre_tool_call
"#
}

pub(crate) fn write_hermes_plugin(home: &Path, auto_yes: bool) -> Result<McpConfigStatus> {
    let plugin_dir = hermes::plugin_dir(home);
    let init_path = plugin_dir.join("__init__.py");
    let manifest_path = plugin_dir.join("plugin.yaml");
    let config_path = hermes::config_path(home);
    if hermes_plugin_is_configured(home)? {
        return Ok(McpConfigStatus::AlreadyConfigured);
    }
    if !auto_yes {
        eprint!(
            "    Write Hermes plugin to {}? [Y/n] ",
            plugin_dir.display().to_string().dimmed()
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let trimmed = input.trim().to_lowercase();
        if !trimmed.is_empty() && trimmed != "y" && trimmed != "yes" {
            return Ok(McpConfigStatus::Declined);
        }
    }
    let existing = if config_path.exists() {
        fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read '{}'", config_path.display()))?
    } else {
        String::new()
    };
    let patched = patch_hermes_config(&existing)
        .with_context(|| format!("failed to patch '{}'", config_path.display()))?;
    fs::create_dir_all(&plugin_dir)?;
    fs::write(&init_path, hermes_plugin_init_content())?;
    fs::write(&manifest_path, hermes_plugin_manifest_content())?;
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&config_path, patched)?;
    Ok(McpConfigStatus::Written)
}

fn hermes_plugin_is_configured(home: &Path) -> Result<bool> {
    let plugin_dir = hermes::plugin_dir(home);
    let init_path = plugin_dir.join("__init__.py");
    let manifest_path = plugin_dir.join("plugin.yaml");
    let config_path = hermes::config_path(home);
    if !init_path.exists() || !manifest_path.exists() || !config_path.exists() {
        return Ok(false);
    }
    let init = fs::read_to_string(&init_path)
        .with_context(|| format!("failed to read '{}'", init_path.display()))?;
    let manifest = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read '{}'", manifest_path.display()))?;
    let config = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read '{}'", config_path.display()))?;
    Ok(init.contains("Packet28 rewrite")
        && manifest.contains("packet28-rewrite")
        && hermes_config_enables_packet28(&config).unwrap_or(false))
}

pub(crate) fn patch_hermes_config(existing: &str) -> Result<String> {
    let mut value = if existing.trim().is_empty() {
        yaml_serde::Value::Mapping(Default::default())
    } else {
        yaml_serde::from_str::<yaml_serde::Value>(existing)?
    };
    let root = value
        .as_mapping_mut()
        .ok_or_else(|| anyhow!("Hermes config root must be a YAML mapping"))?;
    let plugins_key = yaml_serde::Value::String("plugins".to_string());
    let enabled_key = yaml_serde::Value::String("enabled".to_string());
    let plugins = root
        .entry(plugins_key)
        .or_insert_with(|| yaml_serde::Value::Mapping(Default::default()))
        .as_mapping_mut()
        .ok_or_else(|| anyhow!("Hermes config 'plugins' must be a YAML mapping"))?;
    let enabled = plugins
        .entry(enabled_key)
        .or_insert_with(|| yaml_serde::Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or_else(|| anyhow!("Hermes config 'plugins.enabled' must be a YAML sequence"))?;
    let plugin = yaml_serde::Value::String("packet28-rewrite".to_string());
    if !enabled.iter().any(|entry| entry == &plugin) {
        enabled.push(plugin);
    }
    Ok(yaml_serde::to_string(&value)?)
}

pub(crate) fn hermes_config_enables_packet28(content: &str) -> Result<bool> {
    let value = yaml_serde::from_str::<yaml_serde::Value>(content)?;
    Ok(value
        .get("plugins")
        .and_then(|plugins| plugins.get("enabled"))
        .and_then(yaml_serde::Value::as_sequence)
        .is_some_and(|enabled| {
            enabled
                .iter()
                .any(|entry| entry.as_str() == Some("packet28-rewrite"))
        }))
}
