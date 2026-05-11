use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use clap::{Args, ValueEnum};
use packet28_reducer_core::{classify_command_argv, reduce_command_output};
use serde_json::json;

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
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    #[arg(allow_hyphen_values = true, required = true)]
    pub command: Vec<String>,
}

pub fn run(args: RunArgs) -> Result<i32> {
    let cwd = crate::cmd_common::caller_cwd()?;
    let root = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.root, &cwd));
    let root = packet28_daemon_core::resolve_workspace_root(&root);
    if args.backend == RuntimeBackend::Auto {
        return run_reducer_aware(&root, &cwd, &args);
    }
    let backend = args.backend;

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

fn run_reducer_aware(
    _root: &std::path::Path,
    cwd: &std::path::Path,
    args: &RunArgs,
) -> Result<i32> {
    let command_text = command_text(&args.command);
    let Some(spec) = classify_command_argv(&command_text, &args.command) else {
        return run_plain_command(&args.command, args.json, args.pretty, "unsupported");
    };
    if !matches!(
        spec.family.as_str(),
        "git" | "rust" | "javascript" | "python" | "fs" | "infra" | "github"
    ) {
        return run_plain_command(&args.command, args.json, args.pretty, "unsupported_family");
    }

    let output = build_command(&args.command)?
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run `{command_text}`"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(1);
    let reduction = reduce_command_output(&spec, &stdout, &stderr, exit_code)?;
    let raw_est_tokens = estimate_tokens(&(stdout.clone() + &stderr));
    let reduced_est_tokens = estimate_tokens(&reduction.compact_preview);
    let saved = raw_est_tokens.saturating_sub(reduced_est_tokens);
    let savings_pct = if raw_est_tokens == 0 {
        0.0
    } else {
        (saved as f64 / raw_est_tokens as f64) * 100.0
    };
    let payload = json!({
        "command": {
            "original": command_text,
            "cwd": cwd.display().to_string(),
            "exit_code": exit_code,
            "timestamp_unix_ms": timestamp_unix_ms(),
        },
        "reduction": reduction,
        "raw_est_tokens": raw_est_tokens,
        "reduced_est_tokens": reduced_est_tokens,
        "savings_percent": savings_pct,
        "fallback_reason": null,
        "provenance": {
            "original_command": command_text,
            "cwd": cwd.display().to_string(),
            "exit_code": exit_code,
            "timestamp_unix_ms": timestamp_unix_ms(),
        }
    });
    if args.json {
        crate::cmd_common::emit_json(&payload, args.pretty)?;
    } else {
        println!("{}", reduction.compact_preview);
        println!(
            "tokens: raw={} reduced={} saved={} ({:.1}%)",
            raw_est_tokens, reduced_est_tokens, saved, savings_pct
        );
    }
    Ok(exit_code)
}

fn run_plain_command(
    argv: &[String],
    json: bool,
    pretty: bool,
    fallback_reason: &str,
) -> Result<i32> {
    let command_text = command_text(argv);
    let output = build_command(argv)?
        .output()
        .with_context(|| format!("failed to run `{command_text}`"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(1);
    if json {
        crate::cmd_common::emit_json(
            &json!({
                "command": {
                    "original": command_text,
                    "exit_code": exit_code,
                    "timestamp_unix_ms": timestamp_unix_ms(),
                },
                "stdout": stdout,
                "stderr": stderr,
                "raw_est_tokens": estimate_tokens(&(stdout.clone() + &stderr)),
                "reduced_est_tokens": estimate_tokens(&(stdout.clone() + &stderr)),
                "savings_percent": 0.0,
                "fallback_reason": fallback_reason,
            }),
            pretty,
        )?;
    } else {
        print!("{stdout}");
        eprint!("{stderr}");
    }
    Ok(exit_code)
}

fn command_text(argv: &[String]) -> String {
    shell_words::join(argv.iter().map(String::as_str))
}

fn estimate_tokens(value: &str) -> u64 {
    ((value.len() as f64) / 4.0).ceil() as u64
}

fn timestamp_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
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
