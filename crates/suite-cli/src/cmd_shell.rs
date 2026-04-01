#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{anyhow, Result};
use clap::Args;

#[derive(Args)]
#[command(trailing_var_arg = true)]
pub struct ShellArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(allow_hyphen_values = true)]
    pub command: Vec<String>,
}

#[cfg(target_os = "linux")]
pub fn run(args: ShellArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    launch_linux_preload(&root, &args.command, "linux_preload")
}

#[cfg(not(target_os = "linux"))]
pub fn run(_args: ShellArgs) -> Result<i32> {
    Err(anyhow!(
        "Packet28 shell is only supported on Linux in Phase A; macOS validation remains experimental"
    ))
}

#[cfg(target_os = "linux")]
pub(crate) fn resolve_root(root: &str) -> Result<PathBuf> {
    let cwd = crate::cmd_common::caller_cwd()?;
    let root = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(root, &cwd));
    Ok(packet28_daemon_core::resolve_workspace_root(&root))
}

#[cfg(target_os = "linux")]
pub(crate) fn launch_linux_preload(
    root: &Path,
    argv: &[String],
    runtime_backend: &str,
) -> Result<i32> {
    crate::cmd_daemon::ensure_daemon(root)?;
    let shim_path = resolve_shim_library()?;
    let mut command = build_command(argv)?;
    command.env("LD_PRELOAD", merge_ld_preload(&shim_path)?);
    command.env("PACKET28_DAEMON_ROOT", root);
    command.env("PACKET28_RUNTIME_BACKEND", runtime_backend);
    command.env("PACKET28_AGENT_FAMILY", detect_agent_family(argv));
    let status = command.status().context("failed to launch shell command")?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(target_os = "linux")]
pub(crate) fn build_command(argv: &[String]) -> Result<Command> {
    if let Some(program) = argv.first() {
        let mut command = Command::new(program);
        command.args(&argv[1..]);
        return Ok(command);
    }

    let shell = std::env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string());
    let mut command = Command::new(shell);
    command.arg("-i");
    Ok(command)
}

#[cfg(target_os = "linux")]
pub(crate) fn merge_ld_preload(shim_path: &Path) -> Result<String> {
    let shim = shim_path
        .to_str()
        .ok_or_else(|| anyhow!("shim library path is not valid UTF-8"))?;
    let merged = match std::env::var("LD_PRELOAD") {
        Ok(existing) if !existing.trim().is_empty() => format!("{shim}:{existing}"),
        _ => shim.to_string(),
    };
    Ok(merged)
}

#[cfg(target_os = "linux")]
pub(crate) fn resolve_shim_library() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("failed to resolve current executable")?;
    let mut candidates = Vec::new();
    if let Some(parent) = exe.parent() {
        candidates.push(parent.join("libcontext_instruct_shim.so"));
        candidates.push(parent.join("deps").join("libcontext_instruct_shim.so"));
        if let Some(grandparent) = parent.parent() {
            candidates.push(grandparent.join("libcontext_instruct_shim.so"));
            candidates.push(grandparent.join("deps").join("libcontext_instruct_shim.so"));
        }
    }
    for ancestor in exe.ancestors() {
        if matches!(
            ancestor.file_name().and_then(|value| value.to_str()),
            Some("debug" | "release")
        ) {
            candidates.push(ancestor.join("libcontext_instruct_shim.so"));
            candidates.push(ancestor.join("deps").join("libcontext_instruct_shim.so"));
        }
    }

    candidates
        .into_iter()
        .find(|candidate| candidate.exists())
        .ok_or_else(|| {
            anyhow!(
                "could not locate libcontext_instruct_shim.so next to the current Packet28 build artifacts"
            )
        })
}

#[cfg(target_os = "linux")]
pub(crate) fn detect_agent_family(argv: &[String]) -> String {
    let Some(program) = argv.first() else {
        return "generic".to_string();
    };
    let lower = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    if lower.contains("claude") {
        "claude".to_string()
    } else if lower.contains("codex") {
        "codex".to_string()
    } else if lower.contains("cursor") {
        "cursor".to_string()
    } else if lower.contains("opencode") {
        "opencode".to_string()
    } else {
        "generic".to_string()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn merges_existing_ld_preload() {
        let path = PathBuf::from("/tmp/libcontext_instruct_shim.so");
        unsafe {
            std::env::set_var("LD_PRELOAD", "/tmp/other.so");
        }
        let merged = merge_ld_preload(&path).unwrap();
        assert!(merged.starts_with("/tmp/libcontext_instruct_shim.so:/tmp/other.so"));
        unsafe {
            std::env::remove_var("LD_PRELOAD");
        }
    }
}
