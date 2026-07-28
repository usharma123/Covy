use std::path::Path;

use packet28_daemon_core::RelaunchPreference;

pub(super) fn apply_generated_relaunch_command(
    config: &mut packet28_daemon_core::HookRuntimeConfig,
    _root: &Path,
    packet28_agent: Option<String>,
) -> bool {
    let should_manage_existing = config.relaunch_command.is_empty()
        || is_generated_relaunch_command(&config.relaunch_command);
    if !should_manage_existing {
        return false;
    }
    match packet28_agent {
        Some(_) => {
            let desired_command = generated_relaunch_command();
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

pub(super) fn generated_relaunch_command() -> Vec<String> {
    // The daemon's task_launch_agent path already prepares and consumes the
    // handoff before spawning this command. Launch the delegated runtime
    // directly so packet28-agent does not wait for and consume it a second time.
    vec!["claude".to_string(), "--continue".to_string()]
}

fn is_generated_relaunch_command(command: &[String]) -> bool {
    command == generated_relaunch_command()
        || command
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

pub(super) fn resolve_packet28_agent_command() -> Option<String> {
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

pub(super) fn shell_escape(value: String) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
}

pub(super) fn generated_packet28_hook_command(runtime: &str, root: &Path) -> String {
    guarded_packet28_hook_command(&resolve_packet28_cli_command(), runtime, root)
}

pub(super) fn guarded_packet28_hook_command(
    packet28_command: &str,
    runtime: &str,
    root: &Path,
) -> String {
    let command_arg = shell_escape(packet28_command.to_string());
    let root_arg = shell_escape(root.display().to_string());
    format!(
        "sh -c 'if [ -x \"$1\" ] || command -v \"$1\" >/dev/null 2>&1; then exec \"$1\" hook {runtime} --root \"$2\"; fi; exit 0' packet28-hook \"{command_arg}\" \"{root_arg}\""
    )
}

pub(crate) fn resolve_packet28_mcp_command() -> String {
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

pub(super) fn resolve_packet28_cli_command() -> String {
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
