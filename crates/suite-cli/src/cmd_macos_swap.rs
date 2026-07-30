use std::path::Path;

use anyhow::{anyhow, Result};

#[cfg(target_os = "macos")]
use std::collections::HashSet;
#[cfg(target_os = "macos")]
use std::ffi::{CString, OsStr};
#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use std::io::{Read, Write};
#[cfg(target_os = "macos")]
use std::net::Shutdown;
#[cfg(target_os = "macos")]
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "macos")]
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "macos")]
use std::os::unix::net::UnixStream;
#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::{Child, Command, ExitStatus};
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::thread::{self, JoinHandle};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
use anyhow::Context;
#[cfg(target_os = "macos")]
use context_kernel_core::INSTRUCTION_SUMMARY_SCHEMA_VERSION;
#[cfg(target_os = "macos")]
use packet28_daemon_core::storage::now_unix;
#[cfg(target_os = "macos")]
use packet28_daemon_protocol::message::{
    ContextBackendKind, ContextResolveOutcome, ContextResolveRequest, ContextSourceKind,
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
const INTERNAL_LAUNCH_GATE_ARG: &str = "__packet28-macos-swap-launch-gate";
#[cfg(target_os = "macos")]
const LAUNCH_GATE_READY: u8 = b'R';
#[cfg(target_os = "macos")]
const LAUNCH_GATE_RELEASE: u8 = b'G';
#[cfg(target_os = "macos")]
const LAUNCH_GATE_ERROR: u8 = b'E';
#[cfg(target_os = "macos")]
const MAX_LAUNCH_GATE_ERROR_BYTES: u64 = 8 * 1024;
#[cfg(all(test, target_os = "macos"))]
const TEST_LAUNCH_GATE_FD_ENV: &str = "PACKET28_TEST_MACOS_SWAP_GATE_FD";
#[cfg(all(test, target_os = "macos"))]
const TEST_LAUNCH_GATE_COMMAND_ENV: &str = "PACKET28_TEST_MACOS_SWAP_GATE_COMMAND";
#[cfg(target_os = "macos")]
static JOURNAL_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "macos")]
static RECOVERY_QUARANTINE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SessionState {
    Staging,
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
    #[serde(default)]
    original_sha256: Option<String>,
    content_sha256: Option<String>,
    task_label: Option<String>,
    original_bytes: Option<usize>,
    rewritten_bytes: Option<usize>,
    backup_path: Option<String>,
    temp_path: Option<String>,
}

// The journal reaches `Staging` before any instruction path is replaced. The
// target remains behind its launch gate until an `Active` report containing
// the child identity is durable. Fields added after the original journal
// format use serde defaults so old sessions remain recoverable without
// trusting unverifiable process identities.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionReport {
    session_id: String,
    workspace_root: String,
    command: Vec<String>,
    agent_family: String,
    backend_kind: String,
    #[serde(default)]
    owner_pid: u32,
    #[serde(default)]
    owner_start_time_micros: Option<u64>,
    pid: u32,
    #[serde(default)]
    child_pgid: Option<i32>,
    #[serde(default)]
    child_start_time_micros: Option<u64>,
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
    original_sha256: String,
    rewritten_sha256: String,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PathIdentity {
    device: u64,
    inode: u64,
}

#[cfg(target_os = "macos")]
struct SignalRelay {
    seen_signal: Arc<AtomicI32>,
    child_pgid: Arc<AtomicI32>,
    handle: SignalHandle,
    thread: Option<JoinHandle<()>>,
}

#[cfg(target_os = "macos")]
impl SignalRelay {
    fn set_child_process_group(&self, child_pgid: i32) {
        self.child_pgid.store(child_pgid, Ordering::SeqCst);
        let signal = self.seen_signal.load(Ordering::SeqCst);
        if signal != 0 {
            let _ = signal_process_group(child_pgid, signal);
        }
    }

    fn record_signal(&self, signal: i32) {
        self.seen_signal.store(signal, Ordering::SeqCst);
        let child_pgid = self.child_pgid.load(Ordering::SeqCst);
        if child_pgid > 0 {
            let _ = signal_process_group(child_pgid, signal);
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for SignalRelay {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(target_os = "macos")]
struct PreparedChildLaunch {
    command: Command,
    parent_stream: UnixStream,
    child_stream: UnixStream,
}

#[cfg(target_os = "macos")]
impl PreparedChildLaunch {
    fn new(target: &Command) -> Result<Self> {
        let (parent_stream, child_stream) =
            UnixStream::pair().context("failed to create macOS child launch gate")?;
        let child_fd = child_stream.as_raw_fd();
        let mut command = launch_gate_command(target, child_fd)?;
        command.process_group(0);
        // SAFETY: The closure performs only async-signal-safe `fcntl` calls
        // after fork. The descriptor is owned by `child_stream`, which stays
        // alive until `spawn` returns.
        unsafe {
            command.pre_exec(move || set_close_on_exec(child_fd, false));
        }
        Ok(Self {
            command,
            parent_stream,
            child_stream,
        })
    }

    fn spawn(mut self) -> Result<(Child, ChildLaunchGate)> {
        let child = self
            .command
            .spawn()
            .context("failed to launch macOS swap command gate")?;
        drop(self.child_stream);
        Ok((
            child,
            ChildLaunchGate {
                stream: self.parent_stream,
            },
        ))
    }
}

#[cfg(target_os = "macos")]
struct ChildLaunchGate {
    stream: UnixStream,
}

#[cfg(target_os = "macos")]
impl ChildLaunchGate {
    fn wait_ready(&mut self) -> Result<()> {
        let mut message = [0_u8; 1];
        self.stream
            .read_exact(&mut message)
            .context("macOS child launch gate exited before readiness")?;
        match message[0] {
            LAUNCH_GATE_READY => Ok(()),
            LAUNCH_GATE_ERROR => Err(self.read_error()),
            other => Err(anyhow!(
                "macOS child launch gate sent unexpected readiness byte {other}"
            )),
        }
    }

    fn release_target(&mut self) -> Result<()> {
        self.stream
            .write_all(&[LAUNCH_GATE_RELEASE])
            .context("failed to release macOS child launch gate")?;
        self.stream
            .shutdown(Shutdown::Write)
            .context("failed to close macOS child launch gate release channel")?;

        let mut response = Vec::new();
        (&mut self.stream)
            .take(MAX_LAUNCH_GATE_ERROR_BYTES + 2)
            .read_to_end(&mut response)
            .context("failed to confirm macOS swap command launch")?;
        if response.is_empty() {
            return Ok(());
        }
        if response.len() > usize::try_from(MAX_LAUNCH_GATE_ERROR_BYTES).unwrap_or(usize::MAX) + 1 {
            return Err(anyhow!(
                "macOS child launch gate error exceeded {} bytes",
                MAX_LAUNCH_GATE_ERROR_BYTES
            ));
        }
        if response[0] != LAUNCH_GATE_ERROR {
            return Err(anyhow!(
                "macOS child launch gate sent unexpected launch byte {}",
                response[0]
            ));
        }
        let detail = String::from_utf8_lossy(&response[1..]);
        Err(anyhow!("failed to launch macOS swap command: {detail}"))
    }

    fn read_error(&mut self) -> anyhow::Error {
        let mut detail = Vec::new();
        let result = (&mut self.stream)
            .take(MAX_LAUNCH_GATE_ERROR_BYTES + 1)
            .read_to_end(&mut detail);
        match result {
            Ok(_) if detail.len() <= MAX_LAUNCH_GATE_ERROR_BYTES as usize => anyhow!(
                "macOS child launch gate failed before readiness: {}",
                String::from_utf8_lossy(&detail)
            ),
            Ok(_) => anyhow!(
                "macOS child launch gate readiness error exceeded {} bytes",
                MAX_LAUNCH_GATE_ERROR_BYTES
            ),
            Err(error) => anyhow!("failed to read macOS child launch gate error: {error}"),
        }
    }
}

#[cfg(target_os = "macos")]
fn command_argv(command: &Command) -> Result<Vec<String>> {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|argument| {
            argument
                .to_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| anyhow!("macOS swap command arguments must be valid UTF-8"))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn apply_command_context(source: &Command, destination: &mut Command) {
    for (key, value) in source.get_envs() {
        match value {
            Some(value) => {
                destination.env(key, value);
            }
            None => {
                destination.env_remove(key);
            }
        }
    }
    if let Some(directory) = source.get_current_dir() {
        destination.current_dir(directory);
    }
}

#[cfg(all(not(test), target_os = "macos"))]
fn launch_gate_command(target: &Command, child_fd: RawFd) -> Result<Command> {
    let argv = command_argv(target)?;
    let executable =
        std::env::current_exe().context("failed to locate Packet28 child launch gate")?;
    let mut command = Command::new(executable);
    command
        .arg(INTERNAL_LAUNCH_GATE_ARG)
        .arg(child_fd.to_string())
        .arg("--")
        .args(argv);
    apply_command_context(target, &mut command);
    Ok(command)
}

#[cfg(all(test, target_os = "macos"))]
fn launch_gate_command(target: &Command, child_fd: RawFd) -> Result<Command> {
    let argv = command_argv(target)?;
    let executable =
        std::env::current_exe().context("failed to locate macOS launch-gate test process")?;
    let mut command = Command::new(executable);
    command
        .arg("--exact")
        .arg("cmd_macos_swap::tests::launch_gate_process")
        .arg("--nocapture")
        .env(TEST_LAUNCH_GATE_FD_ENV, child_fd.to_string())
        .env(
            TEST_LAUNCH_GATE_COMMAND_ENV,
            serde_json::to_string(&argv).context("failed to encode launch-gate test command")?,
        );
    apply_command_context(target, &mut command);
    Ok(command)
}

#[cfg(target_os = "macos")]
fn set_close_on_exec(fd: RawFd, close_on_exec: bool) -> std::io::Result<()> {
    // SAFETY: `fcntl(F_GETFD)` only inspects the caller-supplied descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let updated = if close_on_exec {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    // SAFETY: `updated` changes only the close-on-exec descriptor flag.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, updated) } == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn write_launch_gate_error(stream: &mut UnixStream, error: impl std::fmt::Display) {
    let _ = stream.write_all(&[LAUNCH_GATE_ERROR]);
    let _ = write!(stream, "{error}");
}

#[cfg(target_os = "macos")]
fn run_launch_gate(fd: RawFd, argv: &[String]) -> i32 {
    if fd < 0 || set_close_on_exec(fd, false).is_err() {
        return 126;
    }
    // SAFETY: The gate descriptor is deliberately inherited by the helper
    // process and ownership is transferred exactly once in that process.
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
    let Some(program) = argv.first() else {
        write_launch_gate_error(&mut stream, "target command is empty");
        return 126;
    };
    if stream.write_all(&[LAUNCH_GATE_READY]).is_err() {
        return 125;
    }

    let mut release = [0_u8; 1];
    if stream.read_exact(&mut release).is_err() {
        return 125;
    }
    if release[0] != LAUNCH_GATE_RELEASE {
        write_launch_gate_error(
            &mut stream,
            format!("unexpected release byte {}", release[0]),
        );
        return 126;
    }
    if let Err(error) = set_close_on_exec(stream.as_raw_fd(), true) {
        write_launch_gate_error(&mut stream, format!("failed to arm close-on-exec: {error}"));
        return 126;
    }

    let mut command = Command::new(program);
    command.args(&argv[1..]);
    #[cfg(test)]
    command
        .env_remove(TEST_LAUNCH_GATE_FD_ENV)
        .env_remove(TEST_LAUNCH_GATE_COMMAND_ENV);
    let error = command.exec();
    let exit_code = if error.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    };
    write_launch_gate_error(&mut stream, error);
    exit_code
}

#[cfg(target_os = "macos")]
pub(crate) fn internal_launch_gate_exit_code(raw_args: &[String]) -> Option<i32> {
    if raw_args.get(1).map(String::as_str) != Some(INTERNAL_LAUNCH_GATE_ARG) {
        return None;
    }
    let Some(fd) = raw_args
        .get(2)
        .and_then(|value| value.parse::<RawFd>().ok())
    else {
        return Some(126);
    };
    if raw_args.get(3).map(String::as_str) != Some("--") {
        return Some(126);
    }
    Some(run_launch_gate(fd, &raw_args[4..]))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn internal_launch_gate_exit_code(_raw_args: &[String]) -> Option<i32> {
    None
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecyclePoint {
    BeforeStage,
    Stage(usize),
    Spawn,
    AfterSpawn,
    ActiveReport,
    AfterActiveReport,
    Signal,
    Wait,
    Restore(usize),
    BeforeTempQuarantine,
    AfterTempQuarantine,
}

#[cfg(target_os = "macos")]
trait LifecycleHooks {
    fn check(&self, _point: LifecyclePoint) -> Result<()> {
        Ok(())
    }

    fn injected_signal(&self, _point: LifecyclePoint) -> Option<i32> {
        None
    }
}

#[cfg(target_os = "macos")]
struct NoopLifecycleHooks;

#[cfg(target_os = "macos")]
impl LifecycleHooks for NoopLifecycleHooks {}

#[cfg(target_os = "macos")]
static NOOP_LIFECYCLE_HOOKS: NoopLifecycleHooks = NoopLifecycleHooks;

#[cfg(target_os = "macos")]
trait JournalStore {
    fn write(&self, path: &Path, report: &SessionReport) -> Result<()>;
}

#[cfg(target_os = "macos")]
struct FileJournalStore;

#[cfg(target_os = "macos")]
impl JournalStore for FileJournalStore {
    fn write(&self, path: &Path, report: &SessionReport) -> Result<()> {
        write_session_report(path, report)
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct WorkspaceSwapLock {
    file: fs::File,
}

#[cfg(target_os = "macos")]
impl WorkspaceSwapLock {
    fn acquire(root: &Path) -> Result<Self> {
        let dir = session_dir(root);
        create_dir_all_durable(&dir)?;
        let path = dir.join(".workspace.lock");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| {
                format!(
                    "failed to open macOS swap workspace lock '{}'",
                    path.display()
                )
            })?;
        // SAFETY: `file` owns a valid descriptor for the lifetime of the lock.
        // LOCK_NB avoids blocking a second Packet28 invocation indefinitely.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Err(anyhow!(
                    "another macOS swap session currently owns workspace '{}'",
                    root.display()
                ));
            }
            return Err(error).with_context(|| {
                format!("failed to lock macOS swap workspace '{}'", root.display())
            });
        }
        Ok(Self { file })
    }
}

#[cfg(target_os = "macos")]
impl Drop for WorkspaceSwapLock {
    fn drop(&mut self) {
        // SAFETY: The descriptor remains valid until `file` is dropped after
        // this method. Unlock failure cannot be recovered during Drop.
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(target_os = "macos")]
struct SwapSession {
    report_path: PathBuf,
    report: SessionReport,
    staged: Vec<StagedRewrite>,
    child: Option<Child>,
    relay: Option<SignalRelay>,
    files_restored: bool,
    ever_spawned: bool,
    target_started: bool,
    cleanup_failures: Vec<String>,
    finalized: bool,
    process_group_cleaned: bool,
    journal_store: Box<dyn JournalStore>,
    _workspace_lock: Option<WorkspaceSwapLock>,
}

#[cfg(target_os = "macos")]
impl SwapSession {
    fn new(report_path: PathBuf, report: SessionReport) -> Self {
        Self {
            report_path,
            report,
            staged: Vec::new(),
            child: None,
            relay: None,
            files_restored: false,
            ever_spawned: false,
            target_started: false,
            cleanup_failures: Vec::new(),
            finalized: false,
            process_group_cleaned: false,
            journal_store: Box::new(FileJournalStore),
            _workspace_lock: None,
        }
    }

    fn persist(&self) -> Result<()> {
        self.journal_store.write(&self.report_path, &self.report)
    }

    fn arm_signal_relay(&mut self, hooks: &dyn LifecycleHooks) -> Result<()> {
        if self.relay.is_none() {
            hooks.check(LifecyclePoint::Signal)?;
            self.relay = Some(install_signal_forwarders()?);
        }
        Ok(())
    }

    fn inject_signal(&self, hooks: &dyn LifecycleHooks, point: LifecyclePoint) -> Result<()> {
        if let Some(signal) = hooks.injected_signal(point) {
            self.relay
                .as_ref()
                .ok_or_else(|| anyhow!("signal relay is not armed at {point:?}"))?
                .record_signal(signal);
        }
        Ok(())
    }

    fn interrupt_if_signalled(&self) -> Result<()> {
        let signal = self.seen_signal();
        if signal == 0 {
            Ok(())
        } else {
            Err(anyhow!(
                "received signal {signal} while preparing macOS instruction swap"
            ))
        }
    }

    fn track_rewrite(
        &mut self,
        staged: StagedRewrite,
        report_entry: SessionFileEntry,
    ) -> Result<()> {
        self.staged.push(staged);
        self.report.files.push(report_entry);
        self.persist()
    }

    fn adopt_child(&mut self, child: Child) -> Result<()> {
        let child_pid = child.id();
        self.report.pid = child_pid;
        self.ever_spawned = true;
        self.child = Some(child);

        let child_pgid = i32::try_from(child_pid)
            .context("spawned child PID does not fit the platform process-group type")?;
        self.report.child_pgid = Some(child_pgid);
        self.relay
            .as_ref()
            .ok_or_else(|| anyhow!("signal relay was not armed before child launch"))?
            .set_child_process_group(child_pgid);
        let child_start_time_micros = process_start_time_micros(child_pid)?
            .ok_or_else(|| anyhow!("spawned child exited before its identity could be recorded"))?;
        self.report.child_start_time_micros = Some(child_start_time_micros);
        self.report.state = SessionState::Active;
        Ok(())
    }

    fn wait_child(&mut self) -> Result<ExitStatus> {
        loop {
            if self.seen_signal() != 0 {
                let child = self
                    .child
                    .take()
                    .ok_or_else(|| anyhow!("macOS swap child is not available to terminate"))?;
                let child_pgid = self.report.child_pgid.unwrap_or_default();
                let status = terminate_and_reap_child(child, child_pgid)?;
                self.process_group_cleaned = true;
                return Ok(status);
            }
            let status = self
                .child
                .as_mut()
                .ok_or_else(|| anyhow!("macOS swap child is not available to wait"))?
                .try_wait()
                .context("failed to inspect child process while waiting")?;
            if let Some(status) = status {
                self.child.take();
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn seen_signal(&self) -> i32 {
        self.relay
            .as_ref()
            .map(|relay| relay.seen_signal.load(Ordering::SeqCst))
            .unwrap_or(0)
    }

    fn finish(&mut self) -> Result<()> {
        self.cleanup(SessionState::Restored)
    }

    fn rollback(&mut self) -> Result<()> {
        let state = if self.target_started {
            SessionState::Restored
        } else {
            SessionState::RolledBack
        };
        self.cleanup(state)
    }

    fn fail<T>(&mut self, primary: anyhow::Error) -> Result<T> {
        match self.rollback() {
            Ok(()) => Err(primary),
            Err(cleanup) => {
                Err(primary.context(format!("macOS swap cleanup also failed: {cleanup:#}")))
            }
        }
    }

    fn cleanup(&mut self, success_state: SessionState) -> Result<()> {
        self.relay.take();

        let mut errors = Vec::new();
        if let Some(child) = self.child.take() {
            let child_pgid = self.report.child_pgid.unwrap_or_default();
            if let Err(err) = terminate_and_reap_child(child, child_pgid) {
                errors.push(format!("failed to terminate/reap child: {err:#}"));
            } else {
                self.process_group_cleaned = true;
            }
        } else if self.ever_spawned && !self.process_group_cleaned {
            if let Some(child_pgid) = self.report.child_pgid {
                if let Err(err) = terminate_process_group(child_pgid) {
                    errors.push(format!(
                        "failed to terminate child process-group descendants: {err:#}"
                    ));
                } else {
                    self.process_group_cleaned = true;
                }
            }
        }

        if !self.files_restored {
            match restore_staged_files(&self.staged) {
                Ok(()) => self.files_restored = true,
                Err(err) => errors.push(format!("failed to restore staged files: {err:#}")),
            }
        }

        self.cleanup_failures.extend(errors);
        self.report.state = if self.cleanup_failures.is_empty() {
            success_state
        } else {
            SessionState::RecoveryFailed
        };
        let journal_persisted = match self.persist() {
            Ok(()) => true,
            Err(err) => {
                self.cleanup_failures
                    .push(format!("failed to persist final swap journal: {err:#}"));
                false
            }
        };

        if self.files_restored
            && self.child.is_none()
            && self.relay.is_none()
            && (!self.ever_spawned || self.process_group_cleaned)
            && journal_persisted
        {
            self.finalized = true;
        }
        if self.cleanup_failures.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(self.cleanup_failures.join("; ")))
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for SwapSession {
    fn drop(&mut self) {
        if !self.finalized {
            let _ = self.rollback();
        }
    }
}

#[cfg(target_os = "macos")]
fn run_child_lifecycle(
    session: &mut SwapSession,
    command: &mut Command,
    hooks: &dyn LifecycleHooks,
) -> Result<(ExitStatus, i32)> {
    session.arm_signal_relay(hooks)?;
    session.interrupt_if_signalled()?;
    hooks.check(LifecyclePoint::Spawn)?;
    let (child, mut launch_gate) = PreparedChildLaunch::new(command)?.spawn()?;
    session.adopt_child(child)?;
    if let Err(error) = launch_gate.wait_ready() {
        session.interrupt_if_signalled()?;
        return Err(error);
    }

    hooks.check(LifecyclePoint::ActiveReport)?;
    session.persist()?;
    session.inject_signal(hooks, LifecyclePoint::AfterActiveReport)?;
    session.interrupt_if_signalled()?;

    if let Err(error) = launch_gate.release_target() {
        session.interrupt_if_signalled()?;
        return Err(error);
    }
    session.target_started = true;
    hooks.check(LifecyclePoint::AfterSpawn)?;
    session.inject_signal(hooks, LifecyclePoint::AfterSpawn)?;
    session.interrupt_if_signalled()?;

    hooks.check(LifecyclePoint::Wait)?;
    let status = session.wait_child()?;
    let signal = session.seen_signal();
    Ok((status, signal))
}

#[cfg(target_os = "macos")]
fn terminate_and_reap_child(mut child: Child, child_pgid: i32) -> Result<ExitStatus> {
    let mut errors = Vec::new();
    let mut child_status = match child.try_wait() {
        Ok(status) => status,
        Err(err) => {
            errors.push(format!("failed to inspect child before cleanup: {err}"));
            None
        }
    };
    if let Err(err) = signal_process_group(child_pgid, SIGTERM) {
        if !error_has_raw_os_error(&err, libc::EPERM) {
            errors.push(format!("failed to terminate child process group: {err:#}"));
        }
    }

    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        if child_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => child_status = Some(status),
                Ok(None) => {}
                Err(err) => {
                    errors.push(format!("failed to inspect child during cleanup: {err}"));
                    break;
                }
            }
        }
        match process_group_is_running(child_pgid) {
            Ok(false) if child_status.is_some() => break,
            Ok(_) => {}
            Err(err) => {
                errors.push(format!("failed to inspect child process group: {err:#}"));
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }

    match process_group_is_running(child_pgid) {
        Ok(true) => {
            if let Err(err) = signal_process_group(child_pgid, libc::SIGKILL) {
                errors.push(format!("failed to kill child process group: {err:#}"));
            }
        }
        Ok(false) => {}
        Err(err) => errors.push(format!("failed to inspect child process group: {err:#}")),
    }
    if child_status.is_none() {
        if let Err(err) = child.kill() {
            if err.raw_os_error() != Some(libc::ESRCH) {
                errors.push(format!(
                    "failed to kill direct child after process-group cleanup: {err}"
                ));
            }
        }
        match child.wait() {
            Ok(status) => child_status = Some(status),
            Err(err) => {
                errors.push(format!(
                    "failed to reap child after process-group cleanup: {err}"
                ));
            }
        }
    }

    if !errors.is_empty() {
        Err(anyhow!(errors.join("; ")))
    } else {
        child_status.ok_or_else(|| anyhow!("child cleanup completed without a reaped exit status"))
    }
}

#[cfg(target_os = "macos")]
fn terminate_process_group(pgid: i32) -> Result<()> {
    signal_process_group(pgid, SIGTERM)?;
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        if !process_group_is_running(pgid)? {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    signal_process_group(pgid, libc::SIGKILL)
}

#[cfg(target_os = "macos")]
fn signal_process_group(pgid: i32, signal: i32) -> Result<()> {
    if pgid <= 0 {
        return Err(anyhow!("invalid child process group {pgid}"));
    }
    // SAFETY: A negative PID targets the process group created for the child.
    // The signal is one of the platform signal constants supplied by callers.
    let rc = unsafe { libc::kill(-pgid, signal) };
    if rc == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(error).with_context(|| format!("failed to signal child process group {pgid}"))
}

#[cfg(target_os = "macos")]
pub(crate) fn launch_macos_swap(
    root: &Path,
    argv: &[String],
    runtime_backend: &str,
) -> Result<i32> {
    launch_macos_swap_with_hooks(root, argv, runtime_backend, &NOOP_LIFECYCLE_HOOKS)
}

#[cfg(target_os = "macos")]
fn launch_macos_swap_with_hooks(
    root: &Path,
    argv: &[String],
    runtime_backend: &str,
    hooks: &dyn LifecycleHooks,
) -> Result<i32> {
    let Some(program) = argv.first() else {
        return Err(anyhow!("Packet28 run requires a command after --"));
    };
    let workspace_lock = WorkspaceSwapLock::acquire(root)?;
    recover_stale_sessions(root)?;

    let agent_family = detect_agent_family(argv);
    let session_id = session_id();
    let session_path = session_report_path(root, &session_id);
    let report = SessionReport {
        session_id: session_id.clone(),
        workspace_root: root.display().to_string(),
        command: argv.to_vec(),
        agent_family: agent_family.clone(),
        backend_kind: runtime_backend.to_string(),
        owner_pid: std::process::id(),
        owner_start_time_micros: process_start_time_micros(std::process::id())?,
        pid: 0,
        child_pgid: None,
        child_start_time_micros: None,
        started_at: now_unix(),
        state: SessionState::Staging,
        files: Vec::new(),
    };

    let mut session = SwapSession::new(session_path, report);
    session._workspace_lock = Some(workspace_lock);
    session.persist()?;
    session.arm_signal_relay(hooks)?;

    let outcome = (|| {
        session.inject_signal(hooks, LifecyclePoint::BeforeStage)?;
        session.interrupt_if_signalled()?;
        stage_instruction_swaps(root, &session_id, &agent_family, &mut session, hooks)?;
        session.interrupt_if_signalled()?;
        session.persist()?;

        let mut command = Command::new(program);
        command.args(&argv[1..]);
        command.env("PACKET28_DAEMON_ROOT", root);
        command.env("PACKET28_RUNTIME_BACKEND", runtime_backend);
        command.env("PACKET28_AGENT_FAMILY", &agent_family);
        run_child_lifecycle(&mut session, &mut command, hooks)
    })();
    let (status, signal) = match outcome {
        Ok(outcome) => outcome,
        Err(err) => return session.fail(err),
    };
    session.finish()?;

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
    session: &mut SwapSession,
    hooks: &dyn LifecycleHooks,
) -> Result<()> {
    for (stage_index, path) in target_instruction_paths(root).into_iter().enumerate() {
        let file_name = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
            anyhow!(
                "instruction file path is not valid UTF-8: {}",
                path.display()
            )
        })?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                session.report.files.push(SessionFileEntry {
                    path: file_name.to_string(),
                    decision: FileDecision::Passthrough,
                    reason: Some(format!("read_failed:{err}")),
                    original_sha256: None,
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
        let original_sha256 = format!("{:x}", Sha256::digest(&bytes));
        let content = match String::from_utf8(bytes.clone()) {
            Ok(content) => content,
            Err(_) => {
                session.report.files.push(SessionFileEntry {
                    path: file_name.to_string(),
                    decision: FileDecision::Passthrough,
                    reason: Some("non_utf8_content".to_string()),
                    original_sha256: Some(original_sha256.clone()),
                    content_sha256: Some(original_sha256),
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
                source_sha256: original_sha256.clone(),
                source_content: content,
                render_mode: None,
                stable_config: None,
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
                session.report.files.push(SessionFileEntry {
                    path: file_name.to_string(),
                    decision: FileDecision::Passthrough,
                    reason: Some(format!("daemon_error:{err}")),
                    original_sha256: Some(original_sha256.clone()),
                    content_sha256: Some(original_sha256),
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
                content_sha256: rewritten_sha256,
                task_label,
                original_bytes,
                rewritten_bytes,
                ..
            } => {
                debug_log_rewrite(file_name, original_bytes, rewritten_bytes);
                let report_entry = SessionFileEntry {
                    path: file_name.to_string(),
                    decision: FileDecision::Rewrite,
                    reason: None,
                    original_sha256: Some(original_sha256.clone()),
                    content_sha256: Some(rewritten_sha256),
                    task_label: Some(task_label),
                    original_bytes: Some(original_bytes),
                    rewritten_bytes: Some(rewritten_bytes),
                    backup_path: None,
                    temp_path: None,
                };
                stage_rewritten_file(
                    session,
                    &path,
                    session_id,
                    content.as_bytes(),
                    original_sha256,
                    report_entry,
                    stage_index,
                    hooks,
                )?;
            }
            ContextResolveOutcome::Passthrough {
                reason,
                content_sha256,
                task_label,
                original_bytes,
            } => {
                debug_log_passthrough(file_name, &reason);
                session.report.files.push(SessionFileEntry {
                    path: file_name.to_string(),
                    decision: FileDecision::Passthrough,
                    reason: Some(reason),
                    original_sha256: Some(original_sha256),
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
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn stage_rewritten_file(
    session: &mut SwapSession,
    path: &Path,
    session_id: &str,
    rewritten: &[u8],
    original_sha256: String,
    mut report_entry: SessionFileEntry,
    stage_index: usize,
    hooks: &dyn LifecycleHooks,
) -> Result<()> {
    session.interrupt_if_signalled()?;
    let locally_computed_sha256 = format!("{:x}", Sha256::digest(rewritten));
    // Recovery trusts only the bytes received by this process. The daemon
    // digest may describe a pre-render representation, so it is not an
    // on-disk restoration invariant.
    report_entry.content_sha256 = Some(locally_computed_sha256);
    let staged = prepare_rewritten_file(path, session_id, rewritten, original_sha256)?;
    report_entry.backup_path = Some(staged.backup_path.display().to_string());
    report_entry.temp_path = Some(staged.temp_path.display().to_string());
    let install = staged.clone();
    session.track_rewrite(staged, report_entry)?;

    install_rewritten_file(&install)?;
    hooks.check(LifecyclePoint::Stage(stage_index))?;
    session.interrupt_if_signalled()
}

#[cfg(target_os = "macos")]
fn prepare_rewritten_file(
    path: &Path,
    session_id: &str,
    rewritten: &[u8],
    original_sha256: String,
) -> Result<StagedRewrite> {
    let file_name = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
        anyhow!(
            "instruction file path is not valid UTF-8: {}",
            path.display()
        )
    })?;
    let temp_path = path.with_file_name(format!("{file_name}.p28-rewrite.{session_id}.tmp"));
    let backup_path = path.with_file_name(format!("{file_name}.p28-backup.{session_id}"));

    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read metadata for '{}'", path.display()))?;
    if backup_path.exists() {
        return Err(anyhow!(
            "refusing to overwrite existing macOS swap backup '{}'",
            backup_path.display()
        ));
    }

    let prepared = (|| {
        let mut temp = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| {
                format!(
                    "failed to create rewritten temp file '{}'",
                    temp_path.display()
                )
            })?;
        temp.write_all(rewritten).with_context(|| {
            format!(
                "failed to write rewritten temp file '{}'",
                temp_path.display()
            )
        })?;
        temp.set_permissions(metadata.permissions())
            .with_context(|| {
                format!(
                    "failed to copy file permissions onto rewritten temp file '{}'",
                    temp_path.display()
                )
            })?;
        temp.sync_all().with_context(|| {
            format!(
                "failed to sync rewritten temp file '{}'",
                temp_path.display()
            )
        })
    })();
    if let Err(err) = prepared {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }

    Ok(StagedRewrite {
        original_path: path.to_path_buf(),
        backup_path,
        temp_path,
        original_sha256,
        rewritten_sha256: format!("{:x}", Sha256::digest(rewritten)),
    })
}

#[cfg(target_os = "macos")]
fn install_rewritten_file(staged: &StagedRewrite) -> Result<()> {
    let original_identity = path_identity(&staged.original_path)?.ok_or_else(|| {
        anyhow!(
            "instruction file '{}' disappeared before replacement",
            staged.original_path.display()
        )
    })?;
    let temp_identity = path_identity(&staged.temp_path)?.ok_or_else(|| {
        anyhow!(
            "prepared instruction file '{}' disappeared before replacement",
            staged.temp_path.display()
        )
    })?;
    fs::File::open(&staged.original_path)
        .with_context(|| {
            format!(
                "failed to open original instruction file '{}' before replacement",
                staged.original_path.display()
            )
        })?
        .sync_all()
        .with_context(|| {
            format!(
                "failed to sync original instruction file '{}' before backup",
                staged.original_path.display()
            )
        })?;
    if !path_matches_sha256(&staged.original_path, &staged.original_sha256)? {
        return Err(anyhow!(
            "instruction file '{}' changed while Packet28 resolved its replacement; preserving the user's newer content",
            staged.original_path.display()
        ));
    }
    fs::hard_link(&staged.original_path, &staged.backup_path).with_context(|| {
        format!(
            "failed to create exclusive backup '{}' for '{}'",
            staged.backup_path.display(),
            staged.original_path.display()
        )
    })?;
    if !path_matches_sha256(&staged.backup_path, &staged.original_sha256)? {
        return Err(anyhow!(
            "exclusive backup '{}' does not match the instruction content Packet28 resolved",
            staged.backup_path.display()
        ));
    }
    if path_identity(&staged.backup_path)? != Some(original_identity)
        || path_identity(&staged.original_path)? != Some(original_identity)
        || !path_matches_sha256(&staged.original_path, &staged.original_sha256)?
    {
        return Err(anyhow!(
            "instruction file '{}' changed while Packet28 created its backup; preserving all recovery artifacts",
            staged.original_path.display()
        ));
    }
    sync_parent_directory(&staged.original_path)?;
    exchange_paths(&staged.temp_path, &staged.original_path).with_context(|| {
        format!(
            "failed to atomically install rewritten instruction file '{}'",
            staged.original_path.display()
        )
    })?;
    sync_parent_directory(&staged.original_path)?;

    let identities_swapped = path_identity(&staged.original_path)? == Some(temp_identity)
        && path_identity(&staged.temp_path)? == Some(original_identity);
    let contents_swapped = path_matches_sha256(&staged.original_path, &staged.rewritten_sha256)?
        && path_matches_sha256(&staged.temp_path, &staged.original_sha256)?;
    if !identities_swapped || !contents_swapped {
        let rollback = if identities_swapped {
            exchange_paths(&staged.temp_path, &staged.original_path)
                .and_then(|()| sync_parent_directory(&staged.original_path))
        } else {
            Err(anyhow!(
                "path identity changed after atomic installation; refusing an unsafe rollback"
            ))
        };
        let suffix = if rollback.is_ok() {
            "the exchange was rolled back"
        } else {
            "the exchange could not be safely rolled back"
        };
        return Err(anyhow!(
            "instruction or prepared content changed during atomic installation; {suffix}"
        ));
    }
    fs::remove_file(&staged.temp_path).with_context(|| {
        format!(
            "failed to remove displaced original temp link '{}'",
            staged.temp_path.display()
        )
    })?;
    sync_parent_directory(&staged.temp_path)
}

#[cfg(target_os = "macos")]
fn restore_staged_files(staged: &[StagedRewrite]) -> Result<()> {
    restore_staged_files_with_hooks(staged, &NOOP_LIFECYCLE_HOOKS)
}

#[cfg(target_os = "macos")]
fn restore_staged_files_with_hooks(
    staged: &[StagedRewrite],
    hooks: &dyn LifecycleHooks,
) -> Result<()> {
    let mut errors = Vec::new();
    for (restore_index, entry) in staged.iter().rev().enumerate() {
        if let Err(err) = hooks
            .check(LifecyclePoint::Restore(restore_index))
            .and_then(|()| restore_staged_file_with_hooks(entry, hooks))
        {
            errors.push(format!("{}: {err:#}", entry.original_path.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(errors.join("; ")))
    }
}

#[cfg(all(target_os = "macos", test))]
fn restore_staged_file(entry: &StagedRewrite) -> Result<()> {
    restore_staged_file_with_hooks(entry, &NOOP_LIFECYCLE_HOOKS)
}

#[cfg(target_os = "macos")]
fn restore_staged_file_with_hooks(entry: &StagedRewrite, hooks: &dyn LifecycleHooks) -> Result<()> {
    if entry.original_sha256.is_empty() {
        return Err(manual_repair_error(
            &entry.original_path,
            &entry.backup_path,
            &entry.temp_path,
            "legacy swap journal does not record an authenticated original digest",
        ));
    }

    if entry.backup_path.exists() {
        let backup_identity = path_identity(&entry.backup_path)?;
        let current_identity = path_identity(&entry.original_path)?;
        let backup_sha256 = path_sha256(&entry.backup_path)?.ok_or_else(|| {
            manual_repair_error(
                &entry.original_path,
                &entry.backup_path,
                &entry.temp_path,
                "backup disappeared during authenticated recovery",
            )
        })?;
        let current_sha256 = path_sha256(&entry.original_path)?;

        if current_sha256.as_deref() == Some(entry.original_sha256.as_str()) {
            let displaced_is_known = backup_sha256 == entry.original_sha256
                || (!entry.rewritten_sha256.is_empty() && backup_sha256 == entry.rewritten_sha256);
            if !displaced_is_known {
                return Err(manual_repair_error(
                    &entry.original_path,
                    &entry.backup_path,
                    &entry.temp_path,
                    "original is already restored but the remaining backup contains unknown content",
                ));
            }
            fs::remove_file(&entry.backup_path).with_context(|| {
                format!(
                    "failed to remove completed swap artifact '{}'",
                    entry.backup_path.display()
                )
            })?;
            sync_parent_directory(&entry.backup_path)?;
        } else if backup_sha256 != entry.original_sha256 {
            return Err(manual_repair_error(
                &entry.original_path,
                &entry.backup_path,
                &entry.temp_path,
                "backup content does not match the recorded original content",
            ));
        } else if let Some(current_sha256) = current_sha256 {
            if entry.rewritten_sha256.is_empty() || current_sha256 != entry.rewritten_sha256 {
                return Err(manual_repair_error(
                    &entry.original_path,
                    &entry.backup_path,
                    &entry.temp_path,
                    "current instruction file changed while the swap was active",
                ));
            }
            exchange_paths(&entry.backup_path, &entry.original_path)?;
            sync_parent_directory(&entry.original_path)?;

            let restored_matches =
                path_matches_sha256(&entry.original_path, &entry.original_sha256)?;
            let displaced_matches =
                path_matches_sha256(&entry.backup_path, &entry.rewritten_sha256)?;
            let identities_swapped = path_identity(&entry.original_path)? == backup_identity
                && path_identity(&entry.backup_path)? == current_identity;
            if !restored_matches || !displaced_matches || !identities_swapped {
                let rollback = if identities_swapped {
                    exchange_paths(&entry.backup_path, &entry.original_path)
                        .and_then(|()| sync_parent_directory(&entry.original_path))
                } else {
                    Err(anyhow!(
                        "path identity changed after atomic restoration; refusing an unsafe rollback"
                    ))
                };
                let reason = if rollback.is_ok() {
                    "instruction or backup changed during atomic restoration; the exchange was rolled back"
                } else {
                    "instruction or backup changed during atomic restoration and the exchange could not be rolled back"
                };
                return Err(manual_repair_error(
                    &entry.original_path,
                    &entry.backup_path,
                    &entry.temp_path,
                    reason,
                ));
            }
            fs::remove_file(&entry.backup_path).with_context(|| {
                format!(
                    "failed to remove displaced rewritten instruction file '{}'",
                    entry.backup_path.display()
                )
            })?;
            sync_parent_directory(&entry.backup_path)?;
        } else {
            rename_path_exclusive(&entry.backup_path, &entry.original_path).with_context(|| {
                format!(
                    "failed to restore missing instruction file '{}' from '{}'",
                    entry.original_path.display(),
                    entry.backup_path.display()
                )
            })?;
            sync_parent_directory(&entry.original_path)?;
            if !path_matches_sha256(&entry.original_path, &entry.original_sha256)? {
                return Err(manual_repair_error(
                    &entry.original_path,
                    &entry.backup_path,
                    &entry.temp_path,
                    "restored instruction does not match the authenticated original digest",
                ));
            }
        }
    } else if !path_matches_sha256(&entry.original_path, &entry.original_sha256)? {
        return Err(manual_repair_error(
            &entry.original_path,
            &entry.backup_path,
            &entry.temp_path,
            "backup missing and current file does not match the original content",
        ));
    }

    remove_authenticated_temp_artifact(entry, hooks)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn remove_authenticated_temp_artifact(
    entry: &StagedRewrite,
    hooks: &dyn LifecycleHooks,
) -> Result<()> {
    let Some(initial_identity) = path_identity(&entry.temp_path)? else {
        return Ok(());
    };
    let initial_metadata = fs::symlink_metadata(&entry.temp_path).with_context(|| {
        format!(
            "failed to inspect rewritten temp artifact '{}'",
            entry.temp_path.display()
        )
    })?;
    let initial_sha256 = path_sha256(&entry.temp_path)?.ok_or_else(|| {
        manual_repair_error(
            &entry.original_path,
            &entry.backup_path,
            &entry.temp_path,
            "temp artifact disappeared during authenticated cleanup",
        )
    })?;
    let is_known_digest = |digest: &str| {
        digest == entry.original_sha256
            || (!entry.rewritten_sha256.is_empty() && digest == entry.rewritten_sha256)
    };
    if !initial_metadata.file_type().is_file() || !is_known_digest(&initial_sha256) {
        return Err(manual_repair_error(
            &entry.original_path,
            &entry.backup_path,
            &entry.temp_path,
            "temp artifact contains unknown content; refusing to remove it",
        ));
    }

    hooks.check(LifecyclePoint::BeforeTempQuarantine)?;
    let quarantine_path = quarantine_temp_artifact(&entry.temp_path)?;
    hooks.check(LifecyclePoint::AfterTempQuarantine)?;

    let quarantined_identity = path_identity(&quarantine_path)?;
    let quarantined_metadata = fs::symlink_metadata(&quarantine_path).with_context(|| {
        format!(
            "failed to inspect quarantined temp artifact '{}'",
            quarantine_path.display()
        )
    })?;
    let quarantined_sha256 = path_sha256(&quarantine_path)?;
    if quarantined_identity != Some(initial_identity)
        || !quarantined_metadata.file_type().is_file()
        || quarantined_sha256
            .as_deref()
            .is_none_or(|digest| !is_known_digest(digest))
    {
        return Err(preserve_quarantined_temp_artifact(
            entry,
            &quarantine_path,
            "temp artifact changed before atomic quarantine; refusing to remove it",
        ));
    }

    // The unique quarantine name structurally separates this authenticated
    // inode from a concurrent recreation of the well-known temp path. Re-read
    // identity and content there before unlinking as a final corruption check.
    if path_identity(&quarantine_path)? != Some(initial_identity)
        || path_sha256(&quarantine_path)?
            .as_deref()
            .is_none_or(|digest| !is_known_digest(digest))
    {
        return Err(preserve_quarantined_temp_artifact(
            entry,
            &quarantine_path,
            "quarantined temp artifact changed during authenticated cleanup",
        ));
    }

    fs::remove_file(&quarantine_path).with_context(|| {
        format!(
            "failed to remove authenticated rewritten temp artifact '{}'",
            quarantine_path.display()
        )
    })?;
    sync_parent_directory(&quarantine_path)?;

    if path_identity(&entry.temp_path)?.is_some() {
        return Err(manual_repair_error(
            &entry.original_path,
            &entry.backup_path,
            &entry.temp_path,
            "temp path was recreated during authenticated cleanup; preserved the concurrent artifact",
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn quarantine_temp_artifact(temp_path: &Path) -> Result<PathBuf> {
    let file_name = temp_path
        .file_name()
        .ok_or_else(|| anyhow!("temp artifact '{}' has no file name", temp_path.display()))?
        .to_string_lossy();
    for _ in 0..64 {
        let sequence = RECOVERY_QUARANTINE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let quarantine_path = temp_path.with_file_name(format!(
            ".{file_name}.p28-quarantine.{}.{sequence}",
            std::process::id()
        ));
        match rename_path_exclusive(temp_path, &quarantine_path) {
            Ok(()) => return Ok(quarantine_path),
            Err(err) if error_has_raw_os_error(&err, libc::EEXIST) => {}
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "failed to atomically quarantine temp artifact '{}'",
                        temp_path.display()
                    )
                });
            }
        }
    }
    Err(anyhow!(
        "failed to allocate a unique quarantine path for temp artifact '{}'",
        temp_path.display()
    ))
}

#[cfg(target_os = "macos")]
fn preserve_quarantined_temp_artifact(
    entry: &StagedRewrite,
    quarantine_path: &Path,
    reason: &str,
) -> anyhow::Error {
    match rename_path_exclusive(quarantine_path, &entry.temp_path) {
        Ok(()) => manual_repair_error(
            &entry.original_path,
            &entry.backup_path,
            &entry.temp_path,
            &format!(
                "{reason}; restored the unknown artifact to '{}'",
                entry.temp_path.display()
            ),
        ),
        Err(restore_error) => manual_repair_error(
            &entry.original_path,
            &entry.backup_path,
            quarantine_path,
            &format!(
                "{reason}; preserved it at '{}' because '{}' could not be restored: {restore_error:#}",
                quarantine_path.display(),
                entry.temp_path.display()
            ),
        ),
    }
}

#[cfg(target_os = "macos")]
fn exchange_paths(left: &Path, right: &Path) -> Result<()> {
    let left_parent = left
        .parent()
        .ok_or_else(|| anyhow!("path '{}' has no parent", left.display()))?
        .canonicalize()
        .with_context(|| format!("failed to resolve parent for '{}'", left.display()))?;
    let right_parent = right
        .parent()
        .ok_or_else(|| anyhow!("path '{}' has no parent", right.display()))?
        .canonicalize()
        .with_context(|| format!("failed to resolve parent for '{}'", right.display()))?;
    if left_parent != right_parent {
        return Err(anyhow!(
            "atomic exchange paths must share one directory: '{}' and '{}'",
            left.display(),
            right.display()
        ));
    }
    let directory = fs::File::open(&left_parent).with_context(|| {
        format!(
            "failed to open atomic exchange directory '{}'",
            left_parent.display()
        )
    })?;
    let left_name = left
        .file_name()
        .ok_or_else(|| anyhow!("path '{}' has no file name", left.display()))?;
    let right_name = right
        .file_name()
        .ok_or_else(|| anyhow!("path '{}' has no file name", right.display()))?;
    let left_name = CString::new(left_name.as_bytes())
        .with_context(|| format!("path '{}' contains an interior NUL", left.display()))?;
    let right_name = CString::new(right_name.as_bytes())
        .with_context(|| format!("path '{}' contains an interior NUL", right.display()))?;
    // SAFETY: The directory descriptor is valid and both C strings contain
    // only one basename. Using one dirfd confines the atomic exchange to the
    // already-opened instruction directory even if an ancestor is renamed.
    let rc = unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            left_name.as_ptr(),
            directory.as_raw_fd(),
            right_name.as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    if rc == 0 {
        directory.sync_all().with_context(|| {
            format!(
                "failed to sync atomic exchange directory '{}'",
                left_parent.display()
            )
        })
    } else {
        Err(std::io::Error::last_os_error()).context("failed to atomically exchange swap files")
    }
}

#[cfg(target_os = "macos")]
fn rename_path_exclusive(source: &Path, destination: &Path) -> Result<()> {
    let source_parent = source
        .parent()
        .ok_or_else(|| anyhow!("path '{}' has no parent", source.display()))?
        .canonicalize()
        .with_context(|| format!("failed to resolve parent for '{}'", source.display()))?;
    let destination_parent = destination
        .parent()
        .ok_or_else(|| anyhow!("path '{}' has no parent", destination.display()))?
        .canonicalize()
        .with_context(|| format!("failed to resolve parent for '{}'", destination.display()))?;
    if source_parent != destination_parent {
        return Err(anyhow!(
            "exclusive rename paths must share one directory: '{}' and '{}'",
            source.display(),
            destination.display()
        ));
    }
    let directory = fs::File::open(&source_parent).with_context(|| {
        format!(
            "failed to open exclusive rename directory '{}'",
            source_parent.display()
        )
    })?;
    let source_name = source
        .file_name()
        .ok_or_else(|| anyhow!("path '{}' has no file name", source.display()))?;
    let destination_name = destination
        .file_name()
        .ok_or_else(|| anyhow!("path '{}' has no file name", destination.display()))?;
    let source_name = CString::new(source_name.as_bytes())
        .with_context(|| format!("path '{}' contains an interior NUL", source.display()))?;
    let destination_name = CString::new(destination_name.as_bytes())
        .with_context(|| format!("path '{}' contains an interior NUL", destination.display()))?;
    // SAFETY: The directory descriptor is valid and both C strings are
    // basenames in that directory. RENAME_EXCL makes the missing-destination
    // recovery branch fail rather than overwrite a concurrently created file.
    let rc = unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            source_name.as_ptr(),
            directory.as_raw_fd(),
            destination_name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if rc == 0 {
        directory.sync_all().with_context(|| {
            format!(
                "failed to sync exclusive rename directory '{}'",
                source_parent.display()
            )
        })
    } else {
        Err(std::io::Error::last_os_error()).context("failed to rename swap file exclusively")
    }
}

#[cfg(target_os = "macos")]
fn path_identity(path: &Path) -> Result<Option<PathIdentity>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(PathIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err)
            .with_context(|| format!("failed to inspect path identity '{}'", path.display())),
    }
}

#[cfg(target_os = "macos")]
fn path_matches_sha256(path: &Path, expected: &str) -> Result<bool> {
    Ok(path_sha256(path)?.as_deref() == Some(expected))
}

#[cfg(target_os = "macos")]
fn path_sha256(path: &Path) -> Result<Option<String>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to hash restored file '{}'", path.display()));
        }
    };
    Ok(Some(format!("{:x}", Sha256::digest(bytes))))
}

#[cfg(target_os = "macos")]
fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("path '{}' has no parent directory", path.display()))?;
    fs::File::open(parent)
        .with_context(|| format!("failed to open parent directory '{}'", parent.display()))?
        .sync_all()
        .with_context(|| format!("failed to sync parent directory '{}'", parent.display()))
}

#[cfg(target_os = "macos")]
fn create_dir_all_durable(path: &Path) -> Result<()> {
    if path.is_dir() {
        return Ok(());
    }

    let mut missing = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor.parent().ok_or_else(|| {
            anyhow!(
                "cannot find an existing ancestor for directory '{}'",
                path.display()
            )
        })?;
    }
    if !cursor.is_dir() {
        return Err(anyhow!(
            "directory ancestor '{}' is not a directory",
            cursor.display()
        ));
    }

    for directory in missing.iter().rev() {
        match fs::create_dir(directory) {
            Ok(()) => sync_parent_directory(directory)?,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if !directory.is_dir() {
                    return Err(err).with_context(|| {
                        format!("path '{}' is not a directory", directory.display())
                    });
                }
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to create directory '{}'", directory.display())
                });
            }
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
        if !matches!(
            report.state,
            SessionState::Staging | SessionState::Active | SessionState::RecoveryFailed
        ) {
            continue;
        }
        let owner_is_running = owner_process_matches(&report)?;
        if owner_is_running {
            return Err(anyhow!(
                "refusing to overlap macOS swap session '{}' while owner process {} is running",
                path.display(),
                if report.owner_pid == 0 {
                    report.pid
                } else {
                    report.owner_pid
                }
            ));
        }

        let mut recovery_errors = Vec::new();
        let child_group_terminated = if let (Some(child_pgid), Some(child_start_time_micros)) =
            (report.child_pgid, report.child_start_time_micros)
        {
            match terminate_orphaned_process_group(child_pgid, child_start_time_micros) {
                Ok(()) => true,
                Err(err) => {
                    recovery_errors.push(format!("failed to terminate orphaned child: {err:#}"));
                    false
                }
            }
        } else {
            true
        };
        if child_group_terminated {
            if let Err(err) = recover_report_files(root, &report) {
                recovery_errors.push(format!("failed to restore instruction files: {err:#}"));
            }
        }
        if !recovery_errors.is_empty() {
            report.state = SessionState::RecoveryFailed;
            write_session_report(&path, &report)?;
            return Err(anyhow!(
                "failed to recover stale macOS swap session '{}': {}",
                path.display(),
                recovery_errors.join("; ")
            ));
        }
        report.state = SessionState::Restored;
        write_session_report(&path, &report)?;
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn owner_process_matches(report: &SessionReport) -> Result<bool> {
    if report.owner_pid == 0 {
        // Legacy reports did not distinguish the Packet28 owner from the
        // child. Conservatively treat a live recorded PID as active.
        return Ok(process_is_running(report.pid));
    }
    let Some(expected_start) = report.owner_start_time_micros else {
        // A legacy owner PID has no reuse-safe identity, so fail closed while
        // that PID exists rather than risking recovery under a live owner.
        return Ok(process_is_running(report.owner_pid));
    };
    Ok(process_start_time_micros(report.owner_pid)? == Some(expected_start))
}

#[cfg(target_os = "macos")]
fn recover_report_files(root: &Path, report: &SessionReport) -> Result<()> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve recovery root '{}'", root.display()))?;
    let recorded_root = Path::new(&report.workspace_root)
        .canonicalize()
        .with_context(|| {
            format!(
                "failed to resolve journal workspace '{}'",
                report.workspace_root
            )
        })?;
    if recorded_root != canonical_root {
        return Err(anyhow!(
            "refusing macOS swap journal for workspace '{}' while recovering '{}'",
            report.workspace_root,
            root.display()
        ));
    }
    let mut staged = Vec::new();
    for file in &report.files {
        if file.decision != FileDecision::Rewrite {
            continue;
        }
        if !TARGET_FILES.contains(&file.path.as_str()) {
            return Err(anyhow!(
                "refusing unsafe macOS swap journal path '{}'",
                file.path
            ));
        }
        let original = root.join(&file.path);
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
        let expected_backup =
            original.with_file_name(format!("{}.p28-backup.{}", file.path, report.session_id));
        let expected_temp = original.with_file_name(format!(
            "{}.p28-rewrite.{}.tmp",
            file.path, report.session_id
        ));
        if canonicalized_parent_path(&backup)? != canonicalized_parent_path(&expected_backup)?
            || canonicalized_parent_path(&temp)? != canonicalized_parent_path(&expected_temp)?
        {
            return Err(manual_repair_error(
                &original,
                &backup,
                &temp,
                "swap journal contains unexpected recovery paths",
            ));
        }
        staged.push(StagedRewrite {
            original_path: original,
            backup_path: backup,
            temp_path: temp,
            original_sha256: file.original_sha256.clone().unwrap_or_default(),
            rewritten_sha256: file.content_sha256.clone().unwrap_or_default(),
        });
    }
    restore_staged_files(&staged)
}

#[cfg(target_os = "macos")]
fn canonicalized_parent_path(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("recovery path '{}' has no parent", path.display()))?
        .canonicalize()
        .with_context(|| {
            format!(
                "failed to resolve recovery path parent for '{}'",
                path.display()
            )
        })?;
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow!("recovery path '{}' has no file name", path.display()))?;
    Ok(parent.join(file_name))
}

#[cfg(target_os = "macos")]
fn terminate_orphaned_process_group(pgid: i32, expected_start_time_micros: u64) -> Result<()> {
    let pid = u32::try_from(pgid)
        .context("recorded child process group is not a positive process identifier")?;
    if !process_group_is_running(pgid)? {
        return Ok(());
    }
    if let Some(actual_start_time_micros) = process_start_time_micros(pid)? {
        if actual_start_time_micros != expected_start_time_micros {
            return Err(anyhow!(
                "recorded child process group {pgid} is still live but its leader identity was reused; refusing to signal it"
            ));
        }
    }
    match signal_process_group(pgid, SIGTERM) {
        Ok(()) => {}
        Err(err)
            if error_has_raw_os_error(&err, libc::EPERM)
                && recorded_process_is_gone_or_zombie(pid, expected_start_time_micros)? =>
        {
            // Darwin can reject a group signal when no signalable member
            // remains and the authenticated group contains only zombies.
            return Ok(());
        }
        Err(err) => return Err(err),
    }
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        if !process_group_is_running(pgid)? {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }

    if let Some(actual_start_time_micros) = process_start_time_micros(pid)? {
        if actual_start_time_micros != expected_start_time_micros {
            return Err(anyhow!(
                "recorded child process group {pgid} changed leader identity during cleanup; refusing to signal it"
            ));
        }
    }
    match signal_process_group(pgid, libc::SIGKILL) {
        Ok(()) => {}
        Err(err)
            if error_has_raw_os_error(&err, libc::EPERM)
                && recorded_process_is_gone_or_zombie(pid, expected_start_time_micros)? =>
        {
            // Darwin can report EPERM when a process group contains only an
            // unreaped zombie. There is no running process left to signal,
            // and a recovery process cannot reap a child it did not spawn.
            return Ok(());
        }
        Err(err) => return Err(err),
    }

    let kill_deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < kill_deadline {
        if !process_group_is_running(pgid)? {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    if recorded_process_is_gone_or_zombie(pid, expected_start_time_micros)? {
        // SIGKILL was delivered to the complete authenticated group above.
        // Any remaining group liveness is an unreaped zombie that this
        // recovery process cannot reap.
        Ok(())
    } else {
        Err(anyhow!(
            "recorded child process group {pgid} remained live after SIGKILL"
        ))
    }
}

#[cfg(target_os = "macos")]
fn process_group_is_running(pgid: i32) -> Result<bool> {
    if pgid <= 0 {
        return Ok(false);
    }
    // SAFETY: Signal zero performs a liveness/permission check only. The
    // negative PID scopes that check to the recorded child process group.
    let rc = unsafe { libc::kill(-pgid, 0) };
    if rc == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error).with_context(|| format!("failed to inspect process group {pgid}")),
    }
}

#[cfg(target_os = "macos")]
fn process_start_time_micros(pid: u32) -> Result<Option<u64>> {
    let Some(info) = process_bsd_info(pid)? else {
        return Ok(None);
    };
    let micros = info
        .pbi_start_tvsec
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(info.pbi_start_tvusec))
        .ok_or_else(|| anyhow!("process {pid} start time overflowed"))?;
    Ok(Some(micros))
}

#[cfg(target_os = "macos")]
fn recorded_process_is_gone_or_zombie(pid: u32, expected_start_time_micros: u64) -> Result<bool> {
    let Some(info) = process_bsd_info(pid)? else {
        return Ok(true);
    };
    let start_time_micros = info
        .pbi_start_tvsec
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(info.pbi_start_tvusec))
        .ok_or_else(|| anyhow!("process {pid} start time overflowed"))?;
    Ok(start_time_micros == expected_start_time_micros && info.pbi_status == libc::SZOMB)
}

#[cfg(target_os = "macos")]
fn process_bsd_info(pid: u32) -> Result<Option<libc::proc_bsdinfo>> {
    let Ok(platform_pid) = i32::try_from(pid) else {
        return Ok(None);
    };
    if platform_pid <= 0 {
        return Ok(None);
    }

    let buffer_size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
        .context("proc_bsdinfo size does not fit the libproc API")?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    // SAFETY: `info` points to writable storage of exactly `buffer_size`
    // bytes. `PROC_PIDTBSDINFO` initializes a `proc_bsdinfo` when it returns
    // that complete size; shorter results are rejected before assume_init.
    let bytes = unsafe {
        libc::proc_pidinfo(
            platform_pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            buffer_size,
        )
    };
    if bytes != buffer_size {
        if bytes <= 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) || !process_is_running(pid) {
                return Ok(None);
            }
            return Err(error).with_context(|| {
                format!(
                    "failed to read stable identity for process {pid}: expected {buffer_size} bytes, got {bytes}"
                )
            });
        }
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "failed to read stable identity for process {pid}: expected {buffer_size} bytes, got {bytes}"
            )
        });
    }
    // SAFETY: The libproc call returned the complete struct size above.
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid {
        return Err(anyhow!(
            "libproc returned PID {} while inspecting process {pid}",
            info.pbi_pid
        ));
    }
    Ok(Some(info))
}

#[cfg(target_os = "macos")]
fn error_has_raw_os_error(error: &anyhow::Error, expected: i32) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|cause| cause.raw_os_error() == Some(expected))
}

#[cfg(target_os = "macos")]
fn install_signal_forwarders() -> Result<SignalRelay> {
    let seen_signal = Arc::new(AtomicI32::new(0));
    let seen_signal_clone = Arc::clone(&seen_signal);
    let child_pgid = Arc::new(AtomicI32::new(0));
    let child_pgid_clone = Arc::clone(&child_pgid);
    let mut signals =
        Signals::new([SIGINT, SIGTERM, SIGHUP]).context("failed to install signal handlers")?;
    let handle = signals.handle();
    let thread = std::thread::spawn(move || {
        for signal in signals.forever() {
            seen_signal_clone.store(signal, Ordering::SeqCst);
            let child_pgid = child_pgid_clone.load(Ordering::SeqCst);
            if child_pgid > 0 {
                let _ = signal_process_group(child_pgid, signal);
            }
        }
    });
    Ok(SignalRelay {
        seen_signal,
        child_pgid,
        handle,
        thread: Some(thread),
    })
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
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("session report '{}' has no parent", path.display()))?;
    create_dir_all_durable(parent)?;
    let payload = serde_json::to_vec_pretty(report)?;
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| anyhow!("session report path is not valid UTF-8: {}", path.display()))?;
    let sequence = JOURNAL_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_path = path.with_file_name(format!(
        ".{file_name}.tmp.{}.{sequence}",
        std::process::id()
    ));
    let result = (|| {
        let mut temp = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| {
                format!(
                    "failed to create temporary session report '{}'",
                    temp_path.display()
                )
            })?;
        temp.write_all(&payload).with_context(|| {
            format!(
                "failed to write temporary session report '{}'",
                temp_path.display()
            )
        })?;
        temp.sync_all().with_context(|| {
            format!(
                "failed to sync temporary session report '{}'",
                temp_path.display()
            )
        })?;
        fs::rename(&temp_path, path).with_context(|| {
            format!(
                "failed to atomically replace session report '{}'",
                path.display()
            )
        })?;
        sync_parent_directory(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(target_os = "macos")]
fn process_is_running(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: Signal zero performs a liveness/permission check only for the
    // validated positive PID.
    let rc = unsafe { libc::kill(pid, 0) };
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
    use std::os::unix::fs::PermissionsExt;
    use std::panic::{self, AssertUnwindSafe};
    use std::process::Stdio;
    use std::sync::atomic::AtomicUsize;

    const ORIGINAL: &[u8] = b"original instruction\n";
    const REWRITTEN: &[u8] = b"rewritten instruction\n";
    const TEST_HARD_EXIT_ROOT_ENV: &str = "PACKET28_TEST_MACOS_SWAP_HARD_EXIT_ROOT";
    const TEST_HARD_EXIT_COMMAND_ENV: &str = "PACKET28_TEST_MACOS_SWAP_HARD_EXIT_COMMAND";
    const HARD_EXIT_STATUS: i32 = 86;

    #[test]
    fn launch_gate_process() {
        let Some(fd) = std::env::var(TEST_LAUNCH_GATE_FD_ENV)
            .ok()
            .and_then(|value| value.parse::<RawFd>().ok())
        else {
            return;
        };
        let argv: Vec<String> = serde_json::from_str(
            &std::env::var(TEST_LAUNCH_GATE_COMMAND_ENV)
                .expect("launch-gate test command is present"),
        )
        .expect("launch-gate test command is valid JSON");
        std::process::exit(run_launch_gate(fd, &argv));
    }

    #[test]
    fn hard_exit_after_spawn_process() {
        let Some(root) = std::env::var_os(TEST_HARD_EXIT_ROOT_ENV).map(PathBuf::from) else {
            return;
        };
        let command_path = PathBuf::from(
            std::env::var_os(TEST_HARD_EXIT_COMMAND_ENV)
                .expect("hard-exit test command is present"),
        );
        let heartbeat_path = root.join("child-heartbeat");
        let termination_observation = root.join("termination-observation");
        let (mut session, _) = staged_session(&root, "after-spawn-hard-exit");
        let mut command = Command::new(command_path);
        command
            .current_dir(&root)
            .env("P28_TEST_HEARTBEAT", &heartbeat_path)
            .env("P28_TEST_TERMINATION_OBSERVATION", &termination_observation);
        let hooks = HardExitAfterSpawn { heartbeat_path };

        let outcome = run_child_lifecycle(&mut session, &mut command, &hooks);
        panic!("hard-exit lifecycle unexpectedly returned: {outcome:?}");
    }

    #[derive(Debug)]
    struct FailAt(LifecyclePoint);

    impl LifecycleHooks for FailAt {
        fn check(&self, point: LifecyclePoint) -> Result<()> {
            if point == self.0 {
                Err(anyhow!("injected {point:?} failure"))
            } else {
                Ok(())
            }
        }
    }

    #[derive(Debug)]
    struct InjectSignalAt(LifecyclePoint);

    impl LifecycleHooks for InjectSignalAt {
        fn injected_signal(&self, point: LifecyclePoint) -> Option<i32> {
            (point == self.0).then_some(SIGTERM)
        }
    }

    struct HardExitAfterSpawn {
        heartbeat_path: PathBuf,
    }

    impl LifecycleHooks for HardExitAfterSpawn {
        fn check(&self, point: LifecyclePoint) -> Result<()> {
            if point == LifecyclePoint::AfterSpawn {
                assert!(
                    wait_until(Duration::from_secs(2), || self
                        .heartbeat_path
                        .metadata()
                        .map(|metadata| metadata.len() > 0)
                        .unwrap_or(false)),
                    "child did not publish its heartbeat before the hard-exit checkpoint"
                );
                // SAFETY: This subprocess exists only to simulate an owner
                // hard crash without running Rust destructors.
                unsafe { libc::_exit(HARD_EXIT_STATUS) }
            }
            Ok(())
        }
    }

    struct ReplaceTempAt {
        point: LifecyclePoint,
        temp_path: PathBuf,
        replacement: &'static [u8],
    }

    impl LifecycleHooks for ReplaceTempAt {
        fn check(&self, point: LifecyclePoint) -> Result<()> {
            if point == self.point {
                let file_name = self.temp_path.file_name().unwrap().to_string_lossy();
                let replacement_path = self
                    .temp_path
                    .with_file_name(format!(".{file_name}.concurrent-replacement"));
                fs::write(&replacement_path, self.replacement)?;
                fs::rename(replacement_path, &self.temp_path)?;
            }
            Ok(())
        }
    }

    struct FailOnceJournalStore {
        fail_at: usize,
        writes: AtomicUsize,
    }

    impl FailOnceJournalStore {
        fn new(fail_at: usize) -> Self {
            Self {
                fail_at,
                writes: AtomicUsize::new(0),
            }
        }
    }

    impl JournalStore for FailOnceJournalStore {
        fn write(&self, path: &Path, report: &SessionReport) -> Result<()> {
            let write_index = self.writes.fetch_add(1, Ordering::SeqCst);
            if write_index == self.fail_at {
                Err(anyhow!("injected journal write failure"))
            } else {
                write_session_report(path, report)
            }
        }
    }

    struct PanicAtWait {
        pid_file: PathBuf,
    }

    impl LifecycleHooks for PanicAtWait {
        fn check(&self, point: LifecyclePoint) -> Result<()> {
            if point == LifecyclePoint::Wait {
                assert!(
                    wait_until(Duration::from_secs(2), || self
                        .pid_file
                        .metadata()
                        .map(|metadata| metadata.len() > 0)
                        .unwrap_or(false)),
                    "child did not publish its PID before the unwind checkpoint"
                );
                panic!("injected wait unwind");
            }
            Ok(())
        }
    }

    struct TestChildGuard(Option<Child>);

    impl TestChildGuard {
        fn new(child: Child) -> Self {
            Self(Some(child))
        }

        fn id(&self) -> u32 {
            self.0.as_ref().expect("test child is present").id()
        }

        fn wait(mut self) -> std::io::Result<ExitStatus> {
            let result = self.0.as_mut().expect("test child is present").wait();
            self.0.take();
            result
        }
    }

    impl Drop for TestChildGuard {
        fn drop(&mut self) {
            if let Some(child) = self.0.take() {
                let child_pgid = i32::try_from(child.id()).unwrap_or_default();
                let _ = terminate_and_reap_child(child, child_pgid);
            }
        }
    }

    struct TestProcessGroupGuard {
        pgid: i32,
        armed: bool,
    }

    impl TestProcessGroupGuard {
        fn new(pgid: i32) -> Self {
            Self { pgid, armed: true }
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for TestProcessGroupGuard {
        fn drop(&mut self) {
            if self.armed {
                let _ = signal_process_group(self.pgid, libc::SIGKILL);
            }
        }
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn test_report(root: &Path, session_id: &str, state: SessionState) -> SessionReport {
        SessionReport {
            session_id: session_id.to_string(),
            workspace_root: root.display().to_string(),
            command: vec!["test-child".to_string()],
            agent_family: "test".to_string(),
            backend_kind: "macos_swap".to_string(),
            owner_pid: std::process::id(),
            owner_start_time_micros: process_start_time_micros(std::process::id()).unwrap(),
            pid: 0,
            child_pgid: None,
            child_start_time_micros: None,
            started_at: 1,
            state,
            files: Vec::new(),
        }
    }

    fn rewrite_entry(file_name: &str, original_sha256: &str) -> SessionFileEntry {
        SessionFileEntry {
            path: file_name.to_string(),
            decision: FileDecision::Rewrite,
            reason: None,
            original_sha256: Some(original_sha256.to_string()),
            content_sha256: Some(sha256(REWRITTEN)),
            task_label: Some("test".to_string()),
            original_bytes: Some(ORIGINAL.len()),
            rewritten_bytes: Some(REWRITTEN.len()),
            backup_path: None,
            temp_path: None,
        }
    }

    fn staged_session(root: &Path, session_id: &str) -> (SwapSession, PathBuf) {
        let original = root.join("AGENTS.md");
        fs::write(&original, ORIGINAL).unwrap();
        let report = test_report(root, session_id, SessionState::Staging);
        let mut session = SwapSession::new(session_report_path(root, session_id), report);
        session.persist().unwrap();
        stage_rewritten_file(
            &mut session,
            &original,
            session_id,
            REWRITTEN,
            sha256(ORIGINAL),
            rewrite_entry("AGENTS.md", &sha256(ORIGINAL)),
            0,
            &NOOP_LIFECYCLE_HOOKS,
        )
        .unwrap();
        assert_eq!(fs::read(&original).unwrap(), REWRITTEN);
        (session, original)
    }

    fn installed_rewrite(root: &Path, file_name: &str, session_id: &str) -> StagedRewrite {
        let original = root.join(file_name);
        fs::write(&original, ORIGINAL).unwrap();
        let staged =
            prepare_rewritten_file(&original, session_id, REWRITTEN, sha256(ORIGINAL)).unwrap();
        install_rewritten_file(&staged).unwrap();
        assert_eq!(fs::read(&original).unwrap(), REWRITTEN);
        staged
    }

    fn report_entry_for_staged(staged: &StagedRewrite) -> SessionFileEntry {
        SessionFileEntry {
            path: staged
                .original_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            decision: FileDecision::Rewrite,
            reason: None,
            original_sha256: Some(staged.original_sha256.clone()),
            content_sha256: Some(sha256(REWRITTEN)),
            task_label: Some("test".to_string()),
            original_bytes: Some(ORIGINAL.len()),
            rewritten_bytes: Some(REWRITTEN.len()),
            backup_path: Some(staged.backup_path.display().to_string()),
            temp_path: Some(staged.temp_path.display().to_string()),
        }
    }

    fn read_report(path: &Path) -> SessionReport {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        condition()
    }

    fn assert_child_lifecycle_failure_restores(point: LifecyclePoint) {
        assert_child_lifecycle_failure_restores_with_hooks(&FailAt(point));
    }

    fn assert_child_lifecycle_failure_restores_with_hooks(hooks: &dyn LifecycleHooks) {
        let dir = tempfile::tempdir().unwrap();
        let (mut session, original) = staged_session(dir.path(), "lifecycle");
        let mut command = Command::new("/bin/sleep");
        command.arg("30");

        let error = run_child_lifecycle(&mut session, &mut command, hooks).unwrap_err();
        let child_pgid = session.report.child_pgid;
        let failure: Result<()> = session.fail(error);
        let failure = failure.unwrap_err();

        let failure_detail = format!("{failure:#}");
        assert!(
            failure_detail.contains("injected") || failure_detail.contains("received signal"),
            "unexpected lifecycle failure: {failure_detail}"
        );
        assert_eq!(fs::read(&original).unwrap(), ORIGINAL);
        assert!(!session.staged[0].backup_path.exists());
        assert!(!session.staged[0].temp_path.exists());
        if let Some(child_pgid) = child_pgid {
            assert!(!process_group_is_running(child_pgid).unwrap());
        }
        assert_eq!(
            read_report(&session.report_path).state,
            if session.target_started {
                SessionState::Restored
            } else {
                SessionState::RolledBack
            },
            "cleanup failures: {:?}; lifecycle failure: {failure_detail}",
            session.cleanup_failures
        );
    }

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
        let mut report = test_report(Path::new("/tmp/demo"), "demo", SessionState::Active);
        report.pid = 42;
        report.child_pgid = Some(42);
        report.child_start_time_micros = Some(123);
        report.files = vec![rewrite_entry("AGENTS.md", "original")];

        let encoded = serde_json::to_vec(&report).unwrap();
        let decoded: SessionReport = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.backend_kind, "macos_swap");
        assert_eq!(decoded.owner_pid, std::process::id());
        assert_eq!(decoded.child_pgid, Some(42));
        assert_eq!(decoded.child_start_time_micros, Some(123));
        assert_eq!(decoded.files.len(), 1);
        assert_eq!(decoded.state, SessionState::Active);
    }

    #[test]
    fn legacy_session_report_defaults_new_process_and_digest_fields() {
        let decoded: SessionReport = serde_json::from_value(serde_json::json!({
            "session_id": "legacy",
            "workspace_root": "/tmp/demo",
            "command": ["claude"],
            "agent_family": "claude",
            "backend_kind": "macos_swap",
            "pid": 42,
            "started_at": 1,
            "state": "active",
            "files": [{
                "path": "AGENTS.md",
                "decision": "rewrite",
                "reason": null,
                "content_sha256": "abc",
                "task_label": null,
                "original_bytes": 8,
                "rewritten_bytes": 4,
                "backup_path": "/tmp/backup",
                "temp_path": "/tmp/temp"
            }]
        }))
        .unwrap();

        assert_eq!(decoded.owner_pid, 0);
        assert_eq!(decoded.owner_start_time_micros, None);
        assert_eq!(decoded.child_pgid, None);
        assert_eq!(decoded.child_start_time_micros, None);
        assert_eq!(decoded.files[0].original_sha256, None);
    }

    #[test]
    fn durable_journal_replacement_leaves_only_parseable_report() {
        let dir = tempfile::tempdir().unwrap();
        let path = session_report_path(dir.path(), "journal");
        let mut report = test_report(dir.path(), "journal", SessionState::Staging);
        write_session_report(&path, &report).unwrap();
        report.state = SessionState::Active;
        report.pid = 77;
        report.child_pgid = Some(77);
        report.child_start_time_micros = Some(456);
        write_session_report(&path, &report).unwrap();

        let written = read_report(&path);
        assert_eq!(written.session_id, report.session_id);
        assert_eq!(written.state, SessionState::Active);
        assert_eq!(written.pid, 77);
        assert_eq!(written.child_pgid, Some(77));
        assert_eq!(written.child_start_time_micros, Some(456));
        let names = fs::read_dir(session_dir(dir.path()))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["journal.json"]);
    }

    #[test]
    fn injected_stage_failure_rolls_back_installed_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("AGENTS.md");
        fs::write(&original, ORIGINAL).unwrap();
        let report = test_report(dir.path(), "stage", SessionState::Staging);
        let mut session = SwapSession::new(session_report_path(dir.path(), "stage"), report);
        session.persist().unwrap();

        let error = stage_rewritten_file(
            &mut session,
            &original,
            "stage",
            REWRITTEN,
            sha256(ORIGINAL),
            rewrite_entry("AGENTS.md", &sha256(ORIGINAL)),
            0,
            &FailAt(LifecyclePoint::Stage(0)),
        )
        .unwrap_err();
        assert_eq!(fs::read(&original).unwrap(), REWRITTEN);
        let failure: Result<()> = session.fail(error);
        assert!(failure.unwrap_err().to_string().contains("injected"));

        assert_eq!(fs::read(&original).unwrap(), ORIGINAL);
        assert!(!session.staged[0].backup_path.exists());
        assert!(!session.staged[0].temp_path.exists());
        assert_eq!(
            read_report(&session.report_path).state,
            SessionState::RolledBack
        );
    }

    #[test]
    fn signal_during_staging_rolls_back_installed_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let (mut session, original) = staged_session(dir.path(), "staging-signal");
        session.arm_signal_relay(&NOOP_LIFECYCLE_HOOKS).unwrap();
        session.relay.as_ref().unwrap().record_signal(SIGTERM);

        let error = session.interrupt_if_signalled().unwrap_err();
        let failure: Result<()> = session.fail(error);

        assert!(failure.unwrap_err().to_string().contains("received signal"));
        assert_eq!(fs::read(&original).unwrap(), ORIGINAL);
        assert_eq!(
            read_report(&session.report_path).state,
            SessionState::RolledBack
        );
    }

    #[test]
    fn install_revalidates_original_after_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("AGENTS.md");
        fs::write(&original, ORIGINAL).unwrap();
        let staged =
            prepare_rewritten_file(&original, "source-race", REWRITTEN, sha256(ORIGINAL)).unwrap();
        let user_edit = b"user edit during daemon resolution\n";
        fs::write(&original, user_edit).unwrap();

        let error = install_rewritten_file(&staged).unwrap_err();

        assert!(error
            .to_string()
            .contains("changed while Packet28 resolved"));
        assert_eq!(fs::read(&original).unwrap(), user_edit);
        assert!(!staged.backup_path.exists());
        assert!(staged.temp_path.exists());
    }

    #[test]
    fn stage_records_the_locally_verified_rewritten_digest() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("AGENTS.md");
        fs::write(&original, ORIGINAL).unwrap();
        let report = test_report(dir.path(), "digest-mismatch", SessionState::Staging);
        let mut session =
            SwapSession::new(session_report_path(dir.path(), "digest-mismatch"), report);
        let mut entry = rewrite_entry("AGENTS.md", &sha256(ORIGINAL));
        entry.content_sha256 = Some("incorrect".to_string());

        stage_rewritten_file(
            &mut session,
            &original,
            "digest-mismatch",
            REWRITTEN,
            sha256(ORIGINAL),
            entry,
            0,
            &NOOP_LIFECYCLE_HOOKS,
        )
        .unwrap();

        assert_eq!(fs::read(&original).unwrap(), REWRITTEN);
        let expected = sha256(REWRITTEN);
        assert_eq!(
            session.report.files[0].content_sha256.as_deref(),
            Some(expected.as_str())
        );
        session.rollback().unwrap();
    }

    #[test]
    fn injected_spawn_failure_restores_staged_files() {
        assert_child_lifecycle_failure_restores(LifecyclePoint::Spawn);
    }

    #[test]
    fn operating_system_spawn_failure_restores_staged_files() {
        let dir = tempfile::tempdir().unwrap();
        let (mut session, original) = staged_session(dir.path(), "spawn-error");
        let mut command = Command::new(dir.path().join("missing-command"));

        let error =
            run_child_lifecycle(&mut session, &mut command, &NOOP_LIFECYCLE_HOOKS).unwrap_err();
        let failure: Result<()> = session.fail(error);

        assert!(failure
            .unwrap_err()
            .to_string()
            .contains("failed to launch macOS swap command"));
        assert_eq!(fs::read(&original).unwrap(), ORIGINAL);
        assert_eq!(
            read_report(&session.report_path).state,
            SessionState::RolledBack
        );
    }

    #[test]
    fn injected_after_spawn_failure_terminates_child_and_restores_files() {
        assert_child_lifecycle_failure_restores(LifecyclePoint::AfterSpawn);
    }

    #[test]
    fn injected_active_report_failure_terminates_child_and_restores_files() {
        assert_child_lifecycle_failure_restores(LifecyclePoint::ActiveReport);
    }

    #[test]
    fn actual_active_journal_write_failure_keeps_target_gated_and_restores_files() {
        let dir = tempfile::tempdir().unwrap();
        let (mut session, original) = staged_session(dir.path(), "journal-write-error");
        let target_started = dir.path().join("target-started");
        session.journal_store = Box::new(FailOnceJournalStore::new(0));
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("printf started > \"$P28_TARGET_STARTED\"; exec /bin/sleep 30")
            .env("P28_TARGET_STARTED", &target_started);

        let error =
            run_child_lifecycle(&mut session, &mut command, &NOOP_LIFECYCLE_HOOKS).unwrap_err();
        let child_pgid = session.report.child_pgid.unwrap();
        let failure: Result<()> = session.fail(error);

        assert!(failure
            .unwrap_err()
            .to_string()
            .contains("injected journal write failure"));
        assert_eq!(fs::read(&original).unwrap(), ORIGINAL);
        assert!(!target_started.exists());
        assert!(!process_group_is_running(child_pgid).unwrap());
        assert_eq!(
            read_report(&session.report_path).state,
            SessionState::RolledBack
        );
    }

    #[test]
    fn injected_signal_setup_failure_terminates_child_and_restores_files() {
        assert_child_lifecycle_failure_restores(LifecyclePoint::Signal);
    }

    #[test]
    fn signal_after_spawn_terminates_child_and_restores_files() {
        assert_child_lifecycle_failure_restores_with_hooks(&InjectSignalAt(
            LifecyclePoint::AfterSpawn,
        ));
    }

    #[test]
    fn signal_after_active_report_terminates_child_and_restores_files() {
        assert_child_lifecycle_failure_restores_with_hooks(&InjectSignalAt(
            LifecyclePoint::AfterActiveReport,
        ));
    }

    #[test]
    fn wait_escalates_and_reaps_a_term_ignoring_child_group() {
        let dir = tempfile::tempdir().unwrap();
        let report = test_report(dir.path(), "term-ignoring", SessionState::Staging);
        let mut session =
            SwapSession::new(session_report_path(dir.path(), "term-ignoring"), report);
        session.arm_signal_relay(&NOOP_LIFECYCLE_HOOKS).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("trap '' TERM; /bin/sleep 30 & wait")
            .process_group(0);
        let child = command.spawn().unwrap();
        session.adopt_child(child).unwrap();
        let child_pgid = session.report.child_pgid.unwrap();
        session.relay.as_ref().unwrap().record_signal(SIGTERM);

        let started = Instant::now();
        let status = session.wait_child().unwrap();

        assert!(!status.success());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(session.process_group_cleaned);
        assert!(!process_group_is_running(child_pgid).unwrap());
        session.finish().unwrap();
    }

    #[test]
    fn injected_wait_failure_terminates_child_and_restores_files() {
        assert_child_lifecycle_failure_restores(LifecyclePoint::Wait);
    }

    #[test]
    fn successful_child_lifecycle_restores_files_and_reports_exit_status() {
        let dir = tempfile::tempdir().unwrap();
        let (mut session, original) = staged_session(dir.path(), "success");
        let mut command = Command::new("/usr/bin/true");

        let (status, signal) =
            run_child_lifecycle(&mut session, &mut command, &NOOP_LIFECYCLE_HOOKS).unwrap();
        session.finish().unwrap();

        assert!(status.success());
        assert_eq!(signal, 0);
        assert_eq!(fs::read(&original).unwrap(), ORIGINAL);
        assert_eq!(
            read_report(&session.report_path).state,
            SessionState::Restored
        );
    }

    #[test]
    fn unwind_drop_terminates_child_and_restores_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let original = root.join("AGENTS.md");
        let pid_file = root.join("child.pid");
        let report_path = session_report_path(root, "unwind");
        let unwind = panic::catch_unwind(AssertUnwindSafe(|| {
            let (mut session, _) = staged_session(root, "unwind");
            let mut command = Command::new("/bin/sh");
            command
                .arg("-c")
                .arg("echo $$ > \"$P28_TEST_PID_FILE\"; exec /bin/sleep 30")
                .env("P28_TEST_PID_FILE", &pid_file);
            let hooks = PanicAtWait {
                pid_file: pid_file.clone(),
            };
            let _ = run_child_lifecycle(&mut session, &mut command, &hooks);
        }));

        assert!(unwind.is_err());
        let child_pid = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        assert!(!process_is_running(child_pid));
        assert_eq!(fs::read(&original).unwrap(), ORIGINAL);
        assert_eq!(read_report(&report_path).state, SessionState::Restored);
    }

    #[test]
    fn hard_exit_after_spawn_is_recovered_before_instruction_restore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let command_path = root.join("heartbeat-child");
        fs::write(
            &command_path,
            "#!/bin/sh\n\
             trap '/bin/cp AGENTS.md \"$P28_TEST_TERMINATION_OBSERVATION\"; exit 0' 15\n\
             printf ready > \"$P28_TEST_HEARTBEAT\"\n\
             while :; do :; done\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&command_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command_path, permissions).unwrap();

        let executable = std::env::current_exe().unwrap();
        let mut owner = Command::new(executable)
            .arg("--exact")
            .arg("cmd_macos_swap::tests::hard_exit_after_spawn_process")
            .arg("--nocapture")
            .env(TEST_HARD_EXIT_ROOT_ENV, root)
            .env(TEST_HARD_EXIT_COMMAND_ENV, &command_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = owner.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = owner.kill();
                let _ = owner.wait();
                let _ = recover_stale_sessions(root);
                panic!("hard-exit owner did not reach the AfterSpawn checkpoint");
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(HARD_EXIT_STATUS));

        let report_path = session_report_path(root, "after-spawn-hard-exit");
        let active = read_report(&report_path);
        assert_eq!(active.state, SessionState::Active);
        let child_pgid = active.child_pgid.unwrap();
        assert!(process_group_is_running(child_pgid).unwrap());
        assert!(fs::metadata(root.join("child-heartbeat")).unwrap().len() > 0);
        let mut process_group_guard = TestProcessGroupGuard::new(child_pgid);

        recover_stale_sessions(root).unwrap();

        assert_eq!(
            fs::read(root.join("termination-observation")).unwrap(),
            REWRITTEN
        );
        assert_eq!(fs::read(root.join("AGENTS.md")).unwrap(), ORIGINAL);
        assert!(!process_group_is_running(child_pgid).unwrap());
        assert_eq!(read_report(&report_path).state, SessionState::Restored);
        process_group_guard.disarm();
    }

    #[test]
    fn partial_rollback_is_retryable_without_removing_visible_files() {
        let dir = tempfile::tempdir().unwrap();
        let first = installed_rewrite(dir.path(), "AGENTS.md", "partial");
        let second = installed_rewrite(dir.path(), "CLAUDE.md", "partial");
        let staged = vec![first.clone(), second.clone()];

        let error = restore_staged_files_with_hooks(&staged, &FailAt(LifecyclePoint::Restore(1)))
            .unwrap_err();
        assert!(error.to_string().contains("injected"));
        assert_eq!(fs::read(&second.original_path).unwrap(), ORIGINAL);
        assert_eq!(fs::read(&first.original_path).unwrap(), REWRITTEN);
        assert!(first.original_path.exists());
        assert!(first.backup_path.exists());

        restore_staged_files(&staged).unwrap();
        assert_eq!(fs::read(&first.original_path).unwrap(), ORIGINAL);
        assert_eq!(fs::read(&second.original_path).unwrap(), ORIGINAL);
        assert!(!first.backup_path.exists());
        assert!(!second.backup_path.exists());
    }

    #[test]
    fn restore_continues_after_an_earlier_entry_fails() {
        let dir = tempfile::tempdir().unwrap();
        let first = installed_rewrite(dir.path(), "AGENTS.md", "best-effort");
        let second = installed_rewrite(dir.path(), "CLAUDE.md", "best-effort");
        let staged = vec![first.clone(), second.clone()];

        let error = restore_staged_files_with_hooks(&staged, &FailAt(LifecyclePoint::Restore(0)))
            .unwrap_err();

        assert!(error.to_string().contains("injected"));
        assert_eq!(fs::read(&first.original_path).unwrap(), ORIGINAL);
        assert_eq!(fs::read(&second.original_path).unwrap(), REWRITTEN);
        restore_staged_files(&staged).unwrap();
        assert_eq!(fs::read(&second.original_path).unwrap(), ORIGINAL);
    }

    #[test]
    fn missing_backup_preserves_current_file_and_recovery_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("AGENTS.md");
        let backup = dir.path().join("AGENTS.md.p28-backup.missing");
        let temp = dir.path().join("AGENTS.md.p28-rewrite.missing.tmp");
        fs::write(&original, REWRITTEN).unwrap();
        fs::write(&temp, b"prepared rewrite").unwrap();
        let staged = StagedRewrite {
            original_path: original.clone(),
            backup_path: backup,
            temp_path: temp.clone(),
            original_sha256: sha256(ORIGINAL),
            rewritten_sha256: sha256(REWRITTEN),
        };

        let error = restore_staged_file(&staged).unwrap_err();

        assert!(error.to_string().contains("Manual repair"));
        assert_eq!(fs::read(&original).unwrap(), REWRITTEN);
        assert_eq!(fs::read(&temp).unwrap(), b"prepared rewrite");
        assert!(original.exists());
    }

    #[test]
    fn missing_backup_is_idempotent_after_original_was_already_restored() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("AGENTS.md");
        let staged = StagedRewrite {
            original_path: original.clone(),
            backup_path: dir.path().join("AGENTS.md.p28-backup.retry"),
            temp_path: dir.path().join("AGENTS.md.p28-rewrite.retry.tmp"),
            original_sha256: sha256(ORIGINAL),
            rewritten_sha256: sha256(REWRITTEN),
        };
        fs::write(&original, ORIGINAL).unwrap();
        fs::write(&staged.temp_path, REWRITTEN).unwrap();

        restore_staged_file(&staged).unwrap();

        assert_eq!(fs::read(&original).unwrap(), ORIGINAL);
        assert!(!staged.temp_path.exists());
    }

    #[test]
    fn restore_preserves_a_temp_path_replaced_before_atomic_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let staged = installed_rewrite(dir.path(), "AGENTS.md", "temp-race");
        fs::write(&staged.temp_path, REWRITTEN).unwrap();
        let concurrent_content = b"concurrent user content\n";
        let hooks = ReplaceTempAt {
            point: LifecyclePoint::BeforeTempQuarantine,
            temp_path: staged.temp_path.clone(),
            replacement: concurrent_content,
        };

        let error = restore_staged_file_with_hooks(&staged, &hooks).unwrap_err();

        assert!(error.to_string().contains("temp artifact changed"));
        assert_eq!(fs::read(&staged.original_path).unwrap(), ORIGINAL);
        assert_eq!(fs::read(&staged.temp_path).unwrap(), concurrent_content);
        assert!(!staged.backup_path.exists());
    }

    #[test]
    fn restore_preserves_a_temp_path_recreated_after_atomic_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let staged = installed_rewrite(dir.path(), "AGENTS.md", "temp-post-move-race");
        fs::write(&staged.temp_path, REWRITTEN).unwrap();
        let concurrent_content = b"concurrent post-move content\n";
        let hooks = ReplaceTempAt {
            point: LifecyclePoint::AfterTempQuarantine,
            temp_path: staged.temp_path.clone(),
            replacement: concurrent_content,
        };

        let error = restore_staged_file_with_hooks(&staged, &hooks).unwrap_err();

        assert!(error.to_string().contains("temp path was recreated"));
        assert_eq!(fs::read(&staged.original_path).unwrap(), ORIGINAL);
        assert_eq!(fs::read(&staged.temp_path).unwrap(), concurrent_content);
        assert!(!staged.backup_path.exists());
    }

    #[test]
    fn missing_original_is_restored_from_authenticated_backup() {
        let dir = tempfile::tempdir().unwrap();
        let staged = installed_rewrite(dir.path(), "AGENTS.md", "missing-original");
        fs::remove_file(&staged.original_path).unwrap();

        restore_staged_file(&staged).unwrap();

        assert_eq!(fs::read(&staged.original_path).unwrap(), ORIGINAL);
        assert!(!staged.backup_path.exists());
        assert!(!staged.temp_path.exists());
    }

    #[test]
    fn exclusive_rename_preserves_a_concurrently_created_destination() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("exclusive-source");
        let destination = dir.path().join("exclusive-destination");
        fs::write(&source, ORIGINAL).unwrap();
        fs::write(&destination, b"concurrent user edit\n").unwrap();

        let error = rename_path_exclusive(&source, &destination).unwrap_err();

        assert!(error.to_string().contains("exclusively"));
        assert_eq!(fs::read(&source).unwrap(), ORIGINAL);
        assert_eq!(fs::read(&destination).unwrap(), b"concurrent user edit\n");
    }

    #[test]
    fn corrupt_backup_preserves_visible_file_and_recovery_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("AGENTS.md");
        let backup = dir.path().join("AGENTS.md.p28-backup.corrupt");
        let temp = dir.path().join("AGENTS.md.p28-rewrite.corrupt.tmp");
        fs::write(&original, REWRITTEN).unwrap();
        fs::write(&backup, b"corrupt backup").unwrap();
        fs::write(&temp, b"prepared rewrite").unwrap();
        let staged = StagedRewrite {
            original_path: original.clone(),
            backup_path: backup.clone(),
            temp_path: temp.clone(),
            original_sha256: sha256(ORIGINAL),
            rewritten_sha256: sha256(REWRITTEN),
        };

        let error = restore_staged_file(&staged).unwrap_err();

        assert!(error.to_string().contains("backup content"));
        assert_eq!(fs::read(&original).unwrap(), REWRITTEN);
        assert_eq!(fs::read(&backup).unwrap(), b"corrupt backup");
        assert_eq!(fs::read(&temp).unwrap(), b"prepared rewrite");
    }

    #[test]
    fn child_modified_file_is_not_overwritten_during_restore() {
        let dir = tempfile::tempdir().unwrap();
        let staged = installed_rewrite(dir.path(), "AGENTS.md", "child-edit");
        let child_edit = b"child-authored instruction update\n";
        fs::write(&staged.original_path, child_edit).unwrap();

        let error = restore_staged_file(&staged).unwrap_err();

        assert!(error
            .to_string()
            .contains("changed while the swap was active"));
        assert_eq!(fs::read(&staged.original_path).unwrap(), child_edit);
        assert_eq!(fs::read(&staged.backup_path).unwrap(), ORIGINAL);
    }

    #[test]
    fn crash_recovery_restores_from_durable_journal() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let staged = installed_rewrite(root, "AGENTS.md", "crash");
        let mut report = test_report(root, "crash", SessionState::Active);
        report.owner_pid = u32::MAX;
        report.files = vec![report_entry_for_staged(&staged)];
        let report_path = session_report_path(root, "crash");
        write_session_report(&report_path, &report).unwrap();

        recover_stale_sessions(root).unwrap();

        assert_eq!(fs::read(&staged.original_path).unwrap(), ORIGINAL);
        assert!(!staged.backup_path.exists());
        assert!(!staged.temp_path.exists());
        assert_eq!(read_report(&report_path).state, SessionState::Restored);
    }

    #[test]
    fn stale_recovery_preserves_swapped_files_when_orphan_cleanup_fails() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let staged = installed_rewrite(root, "AGENTS.md", "orphan-error");
        let mut report = test_report(root, "orphan-error", SessionState::Active);
        report.owner_pid = u32::MAX;
        report.child_pgid = Some(-1);
        report.child_start_time_micros = Some(1);
        report.files = vec![report_entry_for_staged(&staged)];
        let report_path = session_report_path(root, "orphan-error");
        write_session_report(&report_path, &report).unwrap();

        let error = recover_stale_sessions(root).unwrap_err();

        assert!(error
            .to_string()
            .contains("failed to terminate orphaned child"));
        assert_eq!(fs::read(&staged.original_path).unwrap(), REWRITTEN);
        assert_eq!(fs::read(&staged.backup_path).unwrap(), ORIGINAL);
        assert_eq!(
            read_report(&report_path).state,
            SessionState::RecoveryFailed
        );
    }

    #[test]
    fn stale_recovery_marks_missing_backup_and_preserves_current_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let original = root.join("AGENTS.md");
        let backup = root.join("AGENTS.md.p28-backup.missing-crash");
        let temp = root.join("AGENTS.md.p28-rewrite.missing-crash.tmp");
        fs::write(&original, REWRITTEN).unwrap();
        fs::write(&temp, b"recovery evidence").unwrap();
        let staged = StagedRewrite {
            original_path: original.clone(),
            backup_path: backup,
            temp_path: temp.clone(),
            original_sha256: sha256(ORIGINAL),
            rewritten_sha256: sha256(REWRITTEN),
        };
        let mut report = test_report(root, "missing-crash", SessionState::Staging);
        report.owner_pid = u32::MAX;
        report.files = vec![report_entry_for_staged(&staged)];
        let report_path = session_report_path(root, "missing-crash");
        write_session_report(&report_path, &report).unwrap();

        let error = recover_stale_sessions(root).unwrap_err();

        assert!(error.to_string().contains("failed to recover stale"));
        assert_eq!(fs::read(&original).unwrap(), REWRITTEN);
        assert_eq!(fs::read(&temp).unwrap(), b"recovery evidence");
        assert_eq!(
            read_report(&report_path).state,
            SessionState::RecoveryFailed
        );

        fs::write(&staged.backup_path, ORIGINAL).unwrap();
        let error = recover_stale_sessions(root).unwrap_err();
        assert!(error.to_string().contains("temp artifact contains unknown"));
        assert_eq!(fs::read(&original).unwrap(), ORIGINAL);
        assert_eq!(fs::read(&temp).unwrap(), b"recovery evidence");
        assert_eq!(
            read_report(&report_path).state,
            SessionState::RecoveryFailed
        );

        fs::remove_file(&temp).unwrap();
        recover_stale_sessions(root).unwrap();
        assert_eq!(read_report(&report_path).state, SessionState::Restored);
    }

    #[test]
    fn stale_recovery_rejects_journal_for_another_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let mut report = test_report(other.path(), "wrong-root", SessionState::Staging);
        report.owner_pid = u32::MAX;
        let report_path = session_report_path(dir.path(), "wrong-root");
        write_session_report(&report_path, &report).unwrap();

        let error = recover_stale_sessions(dir.path()).unwrap_err();

        assert!(error.to_string().contains("refusing macOS swap journal"));
        assert_eq!(
            read_report(&report_path).state,
            SessionState::RecoveryFailed
        );
    }

    #[test]
    fn recovery_accepts_equivalent_canonical_workspace_paths() {
        let dir = tempfile::tempdir().unwrap();
        let canonical_root = dir.path().canonicalize().unwrap();
        let report = test_report(dir.path(), "canonical-root", SessionState::Staging);

        recover_report_files(&canonical_root, &report).unwrap();
    }

    #[test]
    fn running_session_blocks_an_overlapping_workspace_swap() {
        let dir = tempfile::tempdir().unwrap();
        let report = test_report(dir.path(), "active-owner", SessionState::Staging);
        let report_path = session_report_path(dir.path(), "active-owner");
        write_session_report(&report_path, &report).unwrap();

        let error = recover_stale_sessions(dir.path()).unwrap_err();

        assert!(error.to_string().contains("refusing to overlap"));
        assert_eq!(read_report(&report_path).state, SessionState::Staging);
    }

    #[test]
    fn workspace_lock_serializes_independent_open_descriptions() {
        let dir = tempfile::tempdir().unwrap();
        let first = WorkspaceSwapLock::acquire(dir.path()).unwrap();

        let error = WorkspaceSwapLock::acquire(dir.path()).unwrap_err();

        assert!(error
            .to_string()
            .contains("another macOS swap session currently owns workspace"));
        drop(first);
        WorkspaceSwapLock::acquire(dir.path()).unwrap();
    }

    #[test]
    fn owner_start_identity_allows_recovery_after_pid_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let mut report = test_report(dir.path(), "owner-reuse", SessionState::Staging);
        report.owner_start_time_micros = report
            .owner_start_time_micros
            .map(|start| start.saturating_add(1));
        let report_path = session_report_path(dir.path(), "owner-reuse");
        write_session_report(&report_path, &report).unwrap();

        recover_stale_sessions(dir.path()).unwrap();

        assert_eq!(read_report(&report_path).state, SessionState::Restored);
    }

    #[test]
    fn stale_recovery_terminates_orphaned_child_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let descendant_pid_file = root.join("descendant.pid");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("/bin/sleep 30 & echo $! > \"$P28_DESCENDANT_PID\"; wait")
            .env("P28_DESCENDANT_PID", &descendant_pid_file)
            .process_group(0);
        let child = command.spawn().unwrap();
        let child = TestChildGuard::new(child);
        assert!(wait_until(Duration::from_secs(2), || {
            descendant_pid_file
                .metadata()
                .map(|metadata| metadata.len() > 0)
                .unwrap_or(false)
        }));
        let descendant_pid = fs::read_to_string(&descendant_pid_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        assert!(process_is_running(descendant_pid));

        let child_pid = child.id();
        let child_pgid = i32::try_from(child_pid).unwrap();
        let mut report = test_report(root, "orphan", SessionState::Active);
        report.owner_pid = u32::MAX;
        report.pid = child_pid;
        report.child_pgid = Some(child_pgid);
        report.child_start_time_micros = process_start_time_micros(child_pid).unwrap();
        let report_path = session_report_path(root, "orphan");
        write_session_report(&report_path, &report).unwrap();

        recover_stale_sessions(root).unwrap();
        let _ = child.wait().unwrap();

        assert!(wait_until(Duration::from_secs(2), || {
            !process_is_running(descendant_pid)
        }));
        assert!(!process_group_is_running(child_pgid).unwrap());
        assert_eq!(read_report(&report_path).state, SessionState::Restored);
    }

    #[test]
    fn stale_recovery_fails_closed_for_a_reused_process_group_identity() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut command = Command::new("/bin/sleep");
        command.arg("30").process_group(0);
        let child = command.spawn().unwrap();
        let child = TestChildGuard::new(child);
        let child_pid = child.id();
        let child_pgid = i32::try_from(child_pid).unwrap();
        let actual_start = process_start_time_micros(child_pid).unwrap().unwrap();

        let mut report = test_report(root, "pid-reuse", SessionState::Active);
        report.owner_pid = u32::MAX;
        report.pid = child_pid;
        report.child_pgid = Some(child_pgid);
        report.child_start_time_micros = Some(actual_start.saturating_add(1));
        let report_path = session_report_path(root, "pid-reuse");
        write_session_report(&report_path, &report).unwrap();

        let error = recover_stale_sessions(root).unwrap_err();

        assert!(error.to_string().contains("leader identity was reused"));
        assert!(process_is_running(child_pid));
        assert!(process_group_is_running(child_pgid).unwrap());
        assert_eq!(
            read_report(&report_path).state,
            SessionState::RecoveryFailed
        );
        drop(child);
    }

    #[test]
    fn stale_recovery_kills_descendants_after_the_group_leader_exits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let descendant_pid_file = root.join("leader-exited-descendant.pid");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("trap '' TERM; /bin/sleep 30 & echo $! > \"$P28_DESCENDANT_PID\"; exit 0")
            .env("P28_DESCENDANT_PID", &descendant_pid_file)
            .process_group(0);
        let mut child = command.spawn().unwrap();
        let child_pid = child.id();
        let child_pgid = i32::try_from(child_pid).unwrap();
        let child_start_time_micros = process_start_time_micros(child_pid).unwrap().unwrap();
        let mut process_group_guard = TestProcessGroupGuard::new(child_pgid);
        assert!(wait_until(Duration::from_secs(2), || {
            descendant_pid_file
                .metadata()
                .map(|metadata| metadata.len() > 0)
                .unwrap_or(false)
        }));
        let descendant_pid = fs::read_to_string(&descendant_pid_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(process_start_time_micros(child_pid).unwrap(), None);
        assert!(process_is_running(descendant_pid));
        assert!(process_group_is_running(child_pgid).unwrap());

        let mut report = test_report(root, "leader-exited", SessionState::Active);
        report.owner_pid = u32::MAX;
        report.pid = child_pid;
        report.child_pgid = Some(child_pgid);
        report.child_start_time_micros = Some(child_start_time_micros);
        let report_path = session_report_path(root, "leader-exited");
        write_session_report(&report_path, &report).unwrap();

        recover_stale_sessions(root).unwrap();

        assert!(wait_until(Duration::from_secs(2), || {
            !process_is_running(descendant_pid)
        }));
        assert!(!process_group_is_running(child_pgid).unwrap());
        assert_eq!(read_report(&report_path).state, SessionState::Restored);
        process_group_guard.disarm();
    }

    #[test]
    fn cleanup_kills_term_ignoring_descendant_after_leader_exits() {
        let dir = tempfile::tempdir().unwrap();
        let descendant_pid_file = dir.path().join("descendant.pid");
        let report = test_report(dir.path(), "descendant-cleanup", SessionState::Staging);
        let mut session = SwapSession::new(
            session_report_path(dir.path(), "descendant-cleanup"),
            report,
        );
        session.arm_signal_relay(&NOOP_LIFECYCLE_HOOKS).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("trap '' TERM; /bin/sleep 30 & echo $! > \"$P28_DESCENDANT_PID\"; exit 0")
            .env("P28_DESCENDANT_PID", &descendant_pid_file);

        let (status, signal) =
            run_child_lifecycle(&mut session, &mut command, &NOOP_LIFECYCLE_HOOKS).unwrap();
        assert!(status.success());
        assert_eq!(signal, 0);
        let descendant_pid = fs::read_to_string(&descendant_pid_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        assert!(process_is_running(descendant_pid));

        session.finish().unwrap();

        assert!(wait_until(Duration::from_secs(2), || {
            !process_is_running(descendant_pid)
        }));
        assert!(!process_group_is_running(session.report.child_pgid.unwrap()).unwrap());
    }

    #[test]
    fn legacy_recovery_without_authenticated_original_digest_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let staged = installed_rewrite(root, "AGENTS.md", "legacy-manual");
        let mut report = test_report(root, "legacy-manual", SessionState::Active);
        report.owner_pid = u32::MAX;
        let mut entry = report_entry_for_staged(&staged);
        entry.original_sha256 = None;
        report.files = vec![entry];
        let report_path = session_report_path(root, "legacy-manual");
        write_session_report(&report_path, &report).unwrap();

        let error = recover_stale_sessions(root).unwrap_err();

        assert!(error
            .to_string()
            .contains("legacy swap journal does not record"));
        assert_eq!(fs::read(&staged.original_path).unwrap(), REWRITTEN);
        assert_eq!(fs::read(&staged.backup_path).unwrap(), ORIGINAL);
        assert_eq!(
            read_report(&report_path).state,
            SessionState::RecoveryFailed
        );
    }
}
