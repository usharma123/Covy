use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::{Args, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum, Default, PartialEq, Eq)]
pub enum RuntimeBackend {
    #[default]
    Auto,
    LinuxPreload,
    LinuxOci,
    MacosSwap,
    MacosFuse,
    WindowsFuse,
    ProxyOnly,
}

impl RuntimeBackend {
    fn as_env_value(self) -> &'static str {
        match self {
            RuntimeBackend::Auto => "auto",
            RuntimeBackend::LinuxPreload => "linux_preload",
            RuntimeBackend::LinuxOci => "linux_oci",
            RuntimeBackend::MacosSwap => "macos_swap",
            RuntimeBackend::MacosFuse => "macos_fuse",
            RuntimeBackend::WindowsFuse => "windows_fuse",
            RuntimeBackend::ProxyOnly => "proxy_only",
        }
    }
}

#[derive(Args)]
#[command(trailing_var_arg = true)]
pub struct RunArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long, value_enum, default_value_t)]
    pub backend: RuntimeBackend,
    #[arg(allow_hyphen_values = true, required = true)]
    pub command: Vec<String>,
}

pub fn run(args: RunArgs) -> Result<i32> {
    let cwd = crate::cmd_common::caller_cwd()?;
    let root = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.root, &cwd));
    let root = packet28_daemon_core::resolve_workspace_root(&root);
    let backend = match args.backend {
        RuntimeBackend::Auto => auto_backend(),
        other => other,
    };

    match backend {
        RuntimeBackend::LinuxPreload => run_linux_preload(&root, &args.command),
        RuntimeBackend::LinuxOci => run_linux_oci(&root, &args.command),
        RuntimeBackend::MacosSwap => run_macos_swap(&root, &args.command),
        RuntimeBackend::MacosFuse => run_macos_fuse(&root, &args.command),
        RuntimeBackend::WindowsFuse => run_windows_fuse(&root, &args.command),
        RuntimeBackend::ProxyOnly => run_proxy_only(&root, &args.command),
        RuntimeBackend::Auto => unreachable!("auto backend should be resolved before execution"),
    }
}

#[cfg(target_os = "linux")]
fn auto_backend() -> RuntimeBackend {
    RuntimeBackend::LinuxPreload
}

#[cfg(target_os = "macos")]
fn auto_backend() -> RuntimeBackend {
    RuntimeBackend::MacosSwap
}

#[cfg(target_os = "windows")]
fn auto_backend() -> RuntimeBackend {
    RuntimeBackend::LinuxOci
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn auto_backend() -> RuntimeBackend {
    RuntimeBackend::ProxyOnly
}

#[cfg(target_os = "linux")]
fn run_linux_preload(root: &std::path::Path, argv: &[String]) -> Result<i32> {
    crate::cmd_shell::launch_linux_preload(root, argv, RuntimeBackend::LinuxPreload.as_env_value())
}

#[cfg(not(target_os = "linux"))]
fn run_linux_preload(_root: &std::path::Path, _argv: &[String]) -> Result<i32> {
    Err(anyhow!(
        "Packet28 run --backend linux-preload is only available on Linux"
    ))
}

#[cfg(target_os = "macos")]
fn run_macos_swap(root: &std::path::Path, argv: &[String]) -> Result<i32> {
    crate::cmd_macos_swap::launch_macos_swap(root, argv, RuntimeBackend::MacosSwap.as_env_value())
}

#[cfg(not(target_os = "macos"))]
fn run_macos_swap(_root: &std::path::Path, _argv: &[String]) -> Result<i32> {
    Err(anyhow!(
        "Packet28 run --backend macos-swap is only available on macOS"
    ))
}

fn run_linux_oci(root: &std::path::Path, argv: &[String]) -> Result<i32> {
    crate::cmd_daemon::ensure_daemon(root)?;
    if std::env::var_os("PACKET28_ENABLE_EXPERIMENTAL_OCI").is_none() {
        return Err(anyhow!(
            "Packet28 run --backend linux-oci is not implemented yet; use a Linux host with --backend linux-preload today"
        ));
    }
    run_passthrough(root, argv, RuntimeBackend::LinuxOci)
}

fn run_macos_fuse(root: &std::path::Path, argv: &[String]) -> Result<i32> {
    crate::cmd_daemon::ensure_daemon(root)?;
    if std::env::var_os("PACKET28_ENABLE_EXPERIMENTAL_MACOS_FUSE").is_none() {
        return Err(anyhow!(
            "Packet28 run --backend macos-fuse is not implemented yet; use --backend macos-swap as the current macOS backend"
        ));
    }
    run_passthrough(root, argv, RuntimeBackend::MacosFuse)
}

fn run_windows_fuse(root: &std::path::Path, argv: &[String]) -> Result<i32> {
    crate::cmd_daemon::ensure_daemon(root)?;
    if std::env::var_os("PACKET28_ENABLE_EXPERIMENTAL_WINDOWS_FUSE").is_none() {
        return Err(anyhow!(
            "Packet28 run --backend windows-fuse is not implemented yet; use --backend linux-oci when container fallback is available"
        ));
    }
    run_passthrough(root, argv, RuntimeBackend::WindowsFuse)
}

fn run_proxy_only(root: &std::path::Path, argv: &[String]) -> Result<i32> {
    crate::cmd_daemon::ensure_daemon(root)?;
    if std::env::var_os("PACKET28_ENABLE_EXPERIMENTAL_PROXY_ONLY").is_none() {
        return Err(anyhow!(
            "Packet28 run --backend proxy-only is not implemented yet; the local HTTP proxy backend has not been added"
        ));
    }
    run_passthrough(root, argv, RuntimeBackend::ProxyOnly)
}

fn run_passthrough(
    root: &std::path::Path,
    argv: &[String],
    backend: RuntimeBackend,
) -> Result<i32> {
    let mut command = build_command(argv)?;
    command.env("PACKET28_DAEMON_ROOT", root);
    command.env("PACKET28_RUNTIME_BACKEND", backend.as_env_value());
    command.env(
        "PACKET28_AGENT_FAMILY",
        detect_agent_family(argv.first().map(String::as_str)),
    );
    let status = command.status().with_context(|| {
        format!(
            "failed to launch runtime backend {}",
            backend.as_env_value()
        )
    })?;
    Ok(status.code().unwrap_or(1))
}

fn build_command(argv: &[String]) -> Result<Command> {
    let Some(program) = argv.first() else {
        return Err(anyhow!("Packet28 run requires a command after --"));
    };
    let mut command = Command::new(program);
    command.args(&argv[1..]);
    Ok(command)
}

fn detect_agent_family(program: Option<&str>) -> String {
    let Some(program) = program else {
        return "generic".to_string();
    };
    let lower = std::path::Path::new(program)
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
