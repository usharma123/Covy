use std::path::Path;

#[cfg(target_os = "macos")]
use std::collections::HashSet;
#[cfg(target_os = "macos")]
use std::ffi::OsStr;
#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::{Child, Command};
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicI32, Ordering};
#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::thread::JoinHandle;
#[cfg(target_os = "macos")]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
use anyhow::{anyhow, Context, Result};
#[cfg(target_os = "macos")]
use context_kernel_core::INSTRUCTION_SUMMARY_SCHEMA_VERSION;
#[cfg(target_os = "macos")]
use packet28_daemon_core::{
    now_unix, ContextBackendKind, ContextResolveOutcome, ContextResolveRequest, ContextSourceKind,
};
#[cfg(target_os = "macos")]
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
#[cfg(target_os = "macos")]
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
#[cfg(target_os = "macos")]
use signal_hook::iterator::{Handle as SignalHandle, Signals};

#[cfg(target_os = "macos")]
const DEFAULT_BUDGET_TOKENS: u64 = 512;
#[cfg(target_os = "macos")]
const TARGET_FILES: [&str; 3] = ["AGENTS.md", "AGENTS.MD", "CLAUDE.md"];

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SessionState {
    Active,
    Restored,
    RolledBack,
    RecoveryFailed,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FileDecision {
    Rewrite,
    Passthrough,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionFileEntry {
    path: String,
    decision: FileDecision,
    reason: Option<String>,
    content_sha256: Option<String>,
    task_label: Option<String>,
    original_bytes: Option<usize>,
    rewritten_bytes: Option<usize>,
    backup_path: Option<String>,
    temp_path: Option<String>,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionReport {
    session_id: String,
    workspace_root: String,
    command: Vec<String>,
    agent_family: String,
    backend_kind: String,
    pid: u32,
    started_at: u64,
    state: SessionState,
    files: Vec<SessionFileEntry>,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
struct StagedRewrite {
    original_path: PathBuf,
    backup_path: PathBuf,
    temp_path: PathBuf,
}

#[cfg(target_os = "macos")]
struct SignalRelay {
    seen_signal: Arc<AtomicI32>,
    handle: SignalHandle,
    thread: Option<JoinHandle<()>>,
}

#[cfg(target_os = "macos")]
pub(crate) fn launch_macos_swap(
    root: &Path,
    argv: &[String],
    runtime_backend: &str,
) -> Result<i32> {
    let Some(program) = argv.first() else {
        return Err(anyhow!("Packet28 run requires a command after --"));
    };
    recover_stale_sessions(root)?;

    let agent_family = detect_agent_family(argv);
    let session_id = session_id();
    let session_path = session_report_path(root, &session_id);
    let mut report = SessionReport {
        session_id: session_id.clone(),
        workspace_root: root.display().to_string(),
        command: argv.to_vec(),
        agent_family: agent_family.clone(),
        backend_kind: runtime_backend.to_string(),
        pid: 0,
        started_at: now_unix(),
        state: SessionState::RolledBack,
        files: Vec::new(),
    };

    let staged = match stage_instruction_swaps(root, &session_id, &agent_family, &mut report) {
        Ok(staged) => staged,
        Err(err) => {
            report.state = SessionState::RolledBack;
            write_session_report(&session_path, &report)?;
            return Err(err);
        }
    };

    let mut command = Command::new(program);
    command.args(&argv[1..]);
    command.env("PACKET28_DAEMON_ROOT", root);
    command.env("PACKET28_RUNTIME_BACKEND", runtime_backend);
    command.env("PACKET28_AGENT_FAMILY", &agent_family);

    let mut child = match command
        .spawn()
        .with_context(|| format!("failed to launch command '{}'", program))
    {
        Ok(child) => child,
        Err(err) => {
            restore_staged_files(&staged)?;
            report.state = SessionState::RolledBack;
            write_session_report(&session_path, &report)?;
            return Err(err);
        }
    };

    report.pid = child.id();
    report.state = SessionState::Active;
    write_session_report(&session_path, &report)?;

    let relay = install_signal_forwarders(child.id())?;
    let status = wait_for_child(&mut child)?;
    let signal = relay.seen_signal.load(Ordering::SeqCst);
    drop_signal_relay(relay);

    if let Err(err) = restore_staged_files(&staged) {
        report.state = SessionState::RecoveryFailed;
        write_session_report(&session_path, &report)?;
        return Err(err);
    }

    report.state = SessionState::Restored;
    write_session_report(&session_path, &report)?;

    if signal != 0 {
        return Ok(128 + signal);
    }
    Ok(status.code().unwrap_or(1))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn launch_macos_swap(
    _root: &Path,
    _argv: &[String],
    _runtime_backend: &str,
) -> Result<i32> {
    Err(anyhow!(
        "Packet28 run --backend macos-swap is only available on macOS"
    ))
}

#[cfg(target_os = "macos")]
fn stage_instruction_swaps(
    root: &Path,
    session_id: &str,
    agent_family: &str,
    report: &mut SessionReport,
) -> Result<Vec<StagedRewrite>> {
    let mut staged = Vec::new();
    for path in target_instruction_paths(root) {
        let file_name = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
            anyhow!(
                "instruction file path is not valid UTF-8: {}",
                path.display()
            )
        })?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                report.files.push(SessionFileEntry {
                    path: file_name.to_string(),
                    decision: FileDecision::Passthrough,
                    reason: Some(format!("read_failed:{err}")),
                    content_sha256: None,
                    task_label: None,
                    original_bytes: None,
                    rewritten_bytes: None,
                    backup_path: None,
                    temp_path: None,
                });
                debug_log_passthrough(file_name, &format!("read_failed:{err}"));
                continue;
            }
        };
        let content_sha256 = format!("{:x}", Sha256::digest(&bytes));
        let content = match String::from_utf8(bytes.clone()) {
            Ok(content) => content,
            Err(_) => {
                report.files.push(SessionFileEntry {
                    path: file_name.to_string(),
                    decision: FileDecision::Passthrough,
                    reason: Some("non_utf8_content".to_string()),
                    content_sha256: Some(content_sha256),
                    task_label: None,
                    original_bytes: Some(bytes.len()),
                    rewritten_bytes: None,
                    backup_path: None,
                    temp_path: None,
                });
                debug_log_passthrough(file_name, "non_utf8_content");
                continue;
            }
        };
        let response = match crate::cmd_daemon::execute_context_resolve(
            root,
            ContextResolveRequest {
                workspace_root: root.display().to_string(),
                source_kind: ContextSourceKind::InstructionFile,
                source_path: Some(file_name.to_string()),
                source_sha256: content_sha256.clone(),
                source_content: content,
                task_id: None,
                task_label: None,
                budget_tokens: Some(DEFAULT_BUDGET_TOKENS),
                schema_version: INSTRUCTION_SUMMARY_SCHEMA_VERSION,
                agent_family: Some(agent_family.to_string()),
                backend_kind: ContextBackendKind::MacosSwap,
            },
        ) {
            Ok(response) => response,
            Err(err) => {
                report.files.push(SessionFileEntry {
                    path: file_name.to_string(),
                    decision: FileDecision::Passthrough,
                    reason: Some(format!("daemon_error:{err}")),
                    content_sha256: Some(content_sha256),
                    task_label: None,
                    original_bytes: Some(bytes.len()),
                    rewritten_bytes: None,
                    backup_path: None,
                    temp_path: None,
                });
                debug_log_passthrough(file_name, &format!("daemon_error:{err}"));
                continue;
            }
        };

        match response.outcome {
            ContextResolveOutcome::Rewrite {
                content,
                content_sha256,
                task_label,
                original_bytes,
                rewritten_bytes,
                ..
            } => {
                let staged_file = match stage_rewritten_file(&path, session_id, content.as_bytes())
                {
                    Ok(staged_file) => staged_file,
                    Err(err) => {
                        let _ = restore_staged_files(&staged);
                        return Err(err);
                    }
                };
                debug_log_rewrite(file_name, original_bytes, rewritten_bytes);
                report.files.push(SessionFileEntry {
                    path: file_name.to_string(),
                    decision: FileDecision::Rewrite,
                    reason: None,
                    content_sha256: Some(content_sha256),
                    task_label: Some(task_label),
                    original_bytes: Some(original_bytes),
                    rewritten_bytes: Some(rewritten_bytes),
                    backup_path: Some(staged_file.backup_path.display().to_string()),
                    temp_path: Some(staged_file.temp_path.display().to_string()),
                });
                staged.push(staged_file);
            }
            ContextResolveOutcome::Passthrough {
                reason,
                content_sha256,
                task_label,
                original_bytes,
            } => {
                debug_log_passthrough(file_name, &reason);
                report.files.push(SessionFileEntry {
                    path: file_name.to_string(),
                    decision: FileDecision::Passthrough,
                    reason: Some(reason),
                    content_sha256,
                    task_label,
                    original_bytes,
                    rewritten_bytes: None,
                    backup_path: None,
                    temp_path: None,
                });
            }
        }
    }
    Ok(staged)
}

#[cfg(target_os = "macos")]
fn stage_rewritten_file(path: &Path, session_id: &str, rewritten: &[u8]) -> Result<StagedRewrite> {
    let file_name = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
        anyhow!(
            "instruction file path is not valid UTF-8: {}",
            path.display()
        )
    })?;
    let temp_path = path.with_file_name(format!("{file_name}.p28-rewrite.{session_id}.tmp"));
    let backup_path = path.with_file_name(format!("{file_name}.p28-backup.{session_id}"));

    fs::write(&temp_path, rewritten).with_context(|| {
        format!(
            "failed to write rewritten temp file '{}'",
            temp_path.display()
        )
    })?;
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read metadata for '{}'", path.display()))?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(metadata.permissions().mode());
    fs::set_permissions(&temp_path, permissions).with_context(|| {
        format!(
            "failed to copy file permissions onto rewritten temp file '{}'",
            temp_path.display()
        )
    })?;

    if let Err(err) = fs::rename(path, &backup_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(anyhow!(
            "failed to back up '{}' before swap: {err}",
            path.display()
        ));
    }
    if let Err(err) = fs::rename(&temp_path, path) {
        let _ = fs::rename(&backup_path, path);
        let _ = fs::remove_file(&temp_path);
        return Err(anyhow!(
            "failed to install rewritten instruction file '{}': {err}",
            path.display()
        ));
    }

    Ok(StagedRewrite {
        original_path: path.to_path_buf(),
        backup_path,
        temp_path,
    })
}

#[cfg(target_os = "macos")]
fn restore_staged_files(staged: &[StagedRewrite]) -> Result<()> {
    for entry in staged.iter().rev() {
        if entry.original_path.exists() {
            fs::remove_file(&entry.original_path).with_context(|| {
                format!(
                    "failed to remove swapped instruction file '{}'",
                    entry.original_path.display()
                )
            })?;
        }
        if !entry.backup_path.exists() {
            return Err(manual_repair_error(
                &entry.original_path,
                &entry.backup_path,
                &entry.temp_path,
                "backup missing during restore",
            ));
        }
        fs::rename(&entry.backup_path, &entry.original_path).with_context(|| {
            format!(
                "failed to restore original instruction file '{}' from '{}'",
                entry.original_path.display(),
                entry.backup_path.display()
            )
        })?;
        if entry.temp_path.exists() {
            fs::remove_file(&entry.temp_path).with_context(|| {
                format!(
                    "failed to remove rewritten temp file '{}'",
                    entry.temp_path.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn recover_stale_sessions(root: &Path) -> Result<()> {
    let dir = session_dir(root);
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(&dir)
        .with_context(|| format!("failed to scan session dir '{}'", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(OsStr::to_str) != Some("json") {
            continue;
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read session report '{}'", path.display()))?;
        let mut report: SessionReport = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse session report '{}'", path.display()))?;
        if report.state != SessionState::Active {
            continue;
        }
        if process_is_running(report.pid) {
            continue;
        }

        if let Err(err) = recover_report_files(&report) {
            report.state = SessionState::RecoveryFailed;
            write_session_report(&path, &report)?;
            return Err(anyhow!(
                "failed to recover stale macOS swap session '{}': {err}",
                path.display()
            ));
        }
        report.state = SessionState::Restored;
        write_session_report(&path, &report)?;
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn recover_report_files(report: &SessionReport) -> Result<()> {
    for file in report.files.iter().rev() {
        if file.decision != FileDecision::Rewrite {
            continue;
        }
        let original = PathBuf::from(&report.workspace_root).join(&file.path);
        let backup = file
            .backup_path
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| {
                manual_repair_error(
                    &original,
                    Path::new(""),
                    Path::new(""),
                    "backup_path missing from session report",
                )
            })?;
        let temp = file
            .temp_path
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| {
                manual_repair_error(
                    &original,
                    &backup,
                    Path::new(""),
                    "temp_path missing from session report",
                )
            })?;
        if original.exists() {
            fs::remove_file(&original).with_context(|| {
                format!(
                    "failed to remove swapped instruction file '{}' during recovery",
                    original.display()
                )
            })?;
        }
        if !backup.exists() {
            return Err(manual_repair_error(
                &original,
                &backup,
                &temp,
                "backup missing during stale-session recovery",
            ));
        }
        fs::rename(&backup, &original).with_context(|| {
            format!(
                "failed to restore original instruction file '{}' from '{}'",
                original.display(),
                backup.display()
            )
        })?;
        if temp.exists() {
            fs::remove_file(&temp).with_context(|| {
                format!(
                    "failed to remove rewritten temp file '{}' during recovery",
                    temp.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_signal_forwarders(child_pid: u32) -> Result<SignalRelay> {
    let seen_signal = Arc::new(AtomicI32::new(0));
    let seen_signal_clone = Arc::clone(&seen_signal);
    let mut signals =
        Signals::new([SIGINT, SIGTERM, SIGHUP]).context("failed to install signal handlers")?;
    let handle = signals.handle();
    let thread = std::thread::spawn(move || {
        for signal in signals.forever() {
            seen_signal_clone.store(signal, Ordering::SeqCst);
            unsafe {
                libc::kill(child_pid as i32, signal);
            }
        }
    });
    Ok(SignalRelay {
        seen_signal,
        handle,
        thread: Some(thread),
    })
}

#[cfg(target_os = "macos")]
fn drop_signal_relay(mut relay: SignalRelay) {
    relay.handle.close();
    if let Some(thread) = relay.thread.take() {
        let _ = thread.join();
    }
}

#[cfg(target_os = "macos")]
fn wait_for_child(child: &mut Child) -> Result<std::process::ExitStatus> {
    child.wait().context("failed to wait for child process")
}

#[cfg(target_os = "macos")]
fn target_instruction_paths(root: &Path) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for name in TARGET_FILES {
        let path = root.join(name);
        if !path.is_file() {
            continue;
        }
        let key = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .to_string_lossy()
            .to_string();
        if seen.insert(key) {
            paths.push(path);
        }
    }
    paths
}

#[cfg(target_os = "macos")]
fn session_dir(root: &Path) -> PathBuf {
    root.join(".packet28").join("runtime").join("macos-swap")
}

#[cfg(target_os = "macos")]
fn session_report_path(root: &Path, session_id: &str) -> PathBuf {
    session_dir(root).join(format!("{session_id}.json"))
}

#[cfg(target_os = "macos")]
fn write_session_report(path: &Path, report: &SessionReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create session dir '{}'", parent.display()))?;
    }
    let payload = serde_json::to_vec_pretty(report)?;
    fs::write(path, payload)
        .with_context(|| format!("failed to write session report '{}'", path.display()))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn process_is_running(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(target_os = "macos")]
fn detect_agent_family(argv: &[String]) -> String {
    let Some(program) = argv.first() else {
        return "generic".to_string();
    };
    let lower = Path::new(program)
        .file_name()
        .and_then(OsStr::to_str)
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

#[cfg(target_os = "macos")]
fn session_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis();
    format!("{millis}-{}", std::process::id())
}

#[cfg(target_os = "macos")]
fn debug_enabled() -> bool {
    matches!(
        std::env::var("P28_DEBUG").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

#[cfg(target_os = "macos")]
fn debug_log_rewrite(path: &str, original_bytes: usize, rewritten_bytes: usize) {
    if !debug_enabled() {
        return;
    }
    let reduction = if original_bytes == 0 {
        0.0
    } else {
        ((original_bytes.saturating_sub(rewritten_bytes)) as f64 / original_bytes as f64) * 100.0
    };
    eprintln!(
        "p28 virtualized path={} original_bytes={} rewritten_bytes={} reduction_pct={:.1}",
        path, original_bytes, rewritten_bytes, reduction
    );
}

#[cfg(target_os = "macos")]
fn debug_log_passthrough(path: &str, reason: &str) {
    if !debug_enabled() {
        return;
    }
    eprintln!("p28 passthrough path={} reason={}", path, reason);
}

#[cfg(target_os = "macos")]
fn manual_repair_error(original: &Path, backup: &Path, temp: &Path, reason: &str) -> anyhow::Error {
    anyhow!(
        "{}. Manual repair: if '{}' exists, move it back to '{}'; then remove '{}' if it still exists.",
        reason,
        backup.display(),
        original.display(),
        temp.display()
    )
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn target_instruction_paths_only_include_existing_root_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("docs")).unwrap();
        fs::write(dir.path().join("AGENTS.md"), "root").unwrap();
        fs::write(dir.path().join("CLAUDE.md"), "root").unwrap();
        fs::write(dir.path().join("docs").join("AGENTS.md"), "nested").unwrap();

        let files = target_instruction_paths(dir.path());
        let names = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec!["AGENTS.md".to_string(), "CLAUDE.md".to_string()]
        );
    }

    #[test]
    fn session_report_round_trips_json() {
        let report = SessionReport {
            session_id: "demo".to_string(),
            workspace_root: "/tmp/demo".to_string(),
            command: vec!["claude".to_string()],
            agent_family: "claude".to_string(),
            backend_kind: "macos_swap".to_string(),
            pid: 42,
            started_at: 1,
            state: SessionState::Active,
            files: vec![SessionFileEntry {
                path: "AGENTS.md".to_string(),
                decision: FileDecision::Rewrite,
                reason: None,
                content_sha256: Some("abc".to_string()),
                task_label: Some("default".to_string()),
                original_bytes: Some(100),
                rewritten_bytes: Some(50),
                backup_path: Some("/tmp/backup".to_string()),
                temp_path: Some("/tmp/temp".to_string()),
            }],
        };

        let encoded = serde_json::to_vec(&report).unwrap();
        let decoded: SessionReport = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.backend_kind, "macos_swap");
        assert_eq!(decoded.files.len(), 1);
        assert_eq!(decoded.state, SessionState::Active);
    }

    #[test]
    fn restore_staged_files_restores_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("AGENTS.md");
        let backup = dir.path().join("AGENTS.md.p28-backup.demo");
        let temp = dir.path().join("AGENTS.md.p28-rewrite.demo.tmp");

        fs::write(&original, "rewritten").unwrap();
        fs::write(&backup, "original").unwrap();

        restore_staged_files(&[StagedRewrite {
            original_path: original.clone(),
            backup_path: backup,
            temp_path: temp,
        }])
        .unwrap();

        assert_eq!(fs::read_to_string(&original).unwrap(), "original");
    }
}
