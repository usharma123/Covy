use serde_json::{json, Value};
use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const DEFAULT_CAPTURE_BYTES: usize = 64 * 1024;
const DEFAULT_MCP_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_MCP_HEADER_BYTES: usize = 64 * 1024;
const DEFAULT_MCP_QUEUE_CAPACITY: usize = 64;
const DEFAULT_CLEANUP_GRACE: Duration = Duration::from_millis(100);
const DEFAULT_TERMINATION_GRACE: Duration = Duration::from_millis(500);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Resource and deadline limits applied by the integration process harness.
#[derive(Clone, Copy, Debug)]
pub struct HarnessLimits {
    capture_bytes: usize,
    mcp_message_bytes: usize,
    mcp_header_bytes: usize,
    mcp_queue_capacity: usize,
    cleanup_grace: Duration,
    termination_grace: Duration,
    poll_interval: Duration,
}

impl Default for HarnessLimits {
    fn default() -> Self {
        Self {
            capture_bytes: DEFAULT_CAPTURE_BYTES,
            mcp_message_bytes: DEFAULT_MCP_MESSAGE_BYTES,
            mcp_header_bytes: DEFAULT_MCP_HEADER_BYTES,
            mcp_queue_capacity: DEFAULT_MCP_QUEUE_CAPACITY,
            cleanup_grace: DEFAULT_CLEANUP_GRACE,
            termination_grace: DEFAULT_TERMINATION_GRACE,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

impl HarnessLimits {
    pub fn with_capture_bytes(mut self, bytes: usize) -> Self {
        self.capture_bytes = bytes;
        self
    }

    pub fn with_mcp_message_bytes(mut self, bytes: usize) -> Self {
        self.mcp_message_bytes = bytes;
        self
    }

    pub fn with_mcp_header_bytes(mut self, bytes: usize) -> Self {
        self.mcp_header_bytes = bytes;
        self
    }

    pub fn with_mcp_queue_capacity(mut self, capacity: usize) -> Self {
        self.mcp_queue_capacity = capacity.max(1);
        self
    }

    pub fn with_cleanup_grace(mut self, grace: Duration) -> Self {
        self.cleanup_grace = grace;
        self
    }

    pub fn with_termination_grace(mut self, grace: Duration) -> Self {
        self.termination_grace = grace;
        self
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

/// A bounded tail snapshot of process output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureSnapshot {
    pub bytes: Vec<u8>,
    pub total_bytes: u64,
    pub truncated: bool,
}

/// Diagnostics captured at the point of a timeout or protocol failure.
#[derive(Clone, Debug)]
pub struct ProcessDiagnostics {
    pub command: String,
    pub pid: u32,
    pub status: Option<ExitStatus>,
    pub stdout: CaptureSnapshot,
    pub stderr: CaptureSnapshot,
}

/// Bounded output returned after a child has exited and been reaped.
#[derive(Debug)]
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

/// Failure from a bounded child-process or MCP operation.
#[derive(Debug)]
pub enum HarnessError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    Timeout {
        operation: &'static str,
        timeout: Duration,
        diagnostics: Box<ProcessDiagnostics>,
    },
    Mcp {
        message: String,
        diagnostics: Box<ProcessDiagnostics>,
    },
}

impl HarnessError {
    pub fn diagnostics(&self) -> Option<&ProcessDiagnostics> {
        match self {
            Self::Io { .. } => None,
            Self::Timeout { diagnostics, .. } | Self::Mcp { diagnostics, .. } => Some(diagnostics),
        }
    }
}

impl fmt::Display for HarnessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::Timeout {
                operation,
                timeout,
                diagnostics,
            } => {
                write!(
                    formatter,
                    "{operation} timed out after {timeout:?}; {}",
                    DisplayDiagnostics(diagnostics)
                )
            }
            Self::Mcp {
                message,
                diagnostics,
            } => write!(
                formatter,
                "MCP failure: {message}; {}",
                DisplayDiagnostics(diagnostics)
            ),
        }
    }
}

impl Error for HarnessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Timeout { .. } | Self::Mcp { .. } => None,
        }
    }
}

struct DisplayDiagnostics<'a>(&'a ProcessDiagnostics);

impl fmt::Display for DisplayDiagnostics<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let diagnostics = self.0;
        write!(
            formatter,
            "command={:?}, pid={}, status={:?}, stdout{}={:?}, stderr{}={:?}",
            diagnostics.command,
            diagnostics.pid,
            diagnostics.status,
            truncation_suffix(&diagnostics.stdout),
            String::from_utf8_lossy(&diagnostics.stdout.bytes),
            truncation_suffix(&diagnostics.stderr),
            String::from_utf8_lossy(&diagnostics.stderr.bytes)
        )
    }
}

fn truncation_suffix(snapshot: &CaptureSnapshot) -> &'static str {
    if snapshot.truncated {
        "(truncated)"
    } else {
        ""
    }
}

#[derive(Clone)]
struct BoundedCapture {
    inner: Arc<Mutex<CaptureState>>,
}

struct CaptureState {
    tail: VecDeque<u8>,
    limit: usize,
    total_bytes: u64,
}

impl BoundedCapture {
    fn new(limit: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CaptureState {
                tail: VecDeque::with_capacity(limit.min(8 * 1024)),
                limit,
                total_bytes: 0,
            })),
        }
    }

    fn append(&self, bytes: &[u8]) {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.total_bytes = state
            .total_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if state.limit == 0 {
            return;
        }
        if bytes.len() >= state.limit {
            let limit = state.limit;
            state.tail.clear();
            state
                .tail
                .extend(bytes[bytes.len().saturating_sub(limit)..].iter().copied());
            return;
        }
        let overflow = state
            .tail
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(state.limit);
        for _ in 0..overflow {
            let _ = state.tail.pop_front();
        }
        state.tail.extend(bytes.iter().copied());
    }

    fn snapshot(&self) -> CaptureSnapshot {
        let state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bytes: Vec<u8> = state.tail.iter().copied().collect();
        CaptureSnapshot {
            truncated: state.total_bytes > u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            total_bytes: state.total_bytes,
            bytes,
        }
    }
}

struct CapturingReader<R> {
    inner: R,
    capture: BoundedCapture,
}

impl<R: Read> Read for CapturingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.capture.append(&buffer[..read]);
        Ok(read)
    }
}

struct ReaderPump {
    handle: Option<JoinHandle<()>>,
    done: Receiver<()>,
}

impl ReaderPump {
    fn spawn(name: &str, task: impl FnOnce() + Send + 'static) -> io::Result<Self> {
        let (done_sender, done) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
                let _ = done_sender.try_send(());
            })?;
        Ok(Self {
            handle: Some(handle),
            done,
        })
    }

    fn finish_bounded(&mut self, timeout: Duration) {
        let finished = match self.done.recv_timeout(timeout) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => true,
            Err(RecvTimeoutError::Timeout) => false,
        };
        let Some(handle) = self.handle.take() else {
            return;
        };
        if finished {
            let _ = handle.join();
        }
    }
}

struct ManagedProcess {
    command: String,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stderr: BoundedCapture,
    stderr_pump: Option<ReaderPump>,
    pid: u32,
    status: Option<ExitStatus>,
    limits: HarnessLimits,
}

impl ManagedProcess {
    fn spawn(
        command: &mut Command,
        limits: HarnessLimits,
    ) -> Result<(Self, ChildStdout), HarnessError> {
        let command_display = format!("{command:?}");
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command.spawn().map_err(|source| HarnessError::Io {
            operation: "spawn child process",
            source,
        })?;
        let pid = child.id();
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                cleanup_spawn_failure(&mut child, pid);
                return Err(missing_pipe("child stdin"));
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                cleanup_spawn_failure(&mut child, pid);
                return Err(missing_pipe("child stdout"));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                cleanup_spawn_failure(&mut child, pid);
                return Err(missing_pipe("child stderr"));
            }
        };
        let stderr_capture = BoundedCapture::new(limits.capture_bytes);
        let stderr_pump =
            match spawn_capture_pump("packet28-test-stderr", stderr, stderr_capture.clone()) {
                Ok(pump) => pump,
                Err(source) => {
                    cleanup_spawn_failure(&mut child, pid);
                    return Err(HarnessError::Io {
                        operation: "spawn stderr reader",
                        source,
                    });
                }
            };
        Ok((
            Self {
                command: command_display,
                child: Some(child),
                stdin: Some(stdin),
                stderr: stderr_capture,
                stderr_pump: Some(stderr_pump),
                pid,
                status: None,
                limits,
            },
            stdout,
        ))
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), HarnessError> {
        let stdin = self.stdin.as_mut().ok_or_else(|| HarnessError::Io {
            operation: "write child stdin",
            source: io::Error::new(io::ErrorKind::BrokenPipe, "child stdin is closed"),
        })?;
        stdin.write_all(bytes).map_err(|source| HarnessError::Io {
            operation: "write child stdin",
            source,
        })?;
        stdin.flush().map_err(|source| HarnessError::Io {
            operation: "flush child stdin",
            source,
        })
    }

    fn close_stdin(&mut self) {
        let _ = self.stdin.take();
    }

    fn observe_status(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(self.status);
        };
        let status = child.try_wait()?;
        if let Some(status) = status {
            self.status = Some(status);
        }
        Ok(status)
    }

    fn wait_until(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        let deadline = checked_deadline(timeout);
        loop {
            if let Some(status) = self.observe_status()? {
                return Ok(Some(status));
            }
            let Some(remaining) = remaining_until(deadline) else {
                return Ok(None);
            };
            sleep_for_poll(remaining, self.limits.poll_interval);
        }
    }

    fn reap_observed_child(&mut self) -> io::Result<ExitStatus> {
        if let Some(mut child) = self.child.take() {
            let status = child.wait()?;
            self.status = Some(status);
            Ok(status)
        } else {
            self.status.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "child process was already reaped")
            })
        }
    }

    fn complete_successful_wait(&mut self) -> io::Result<ExitStatus> {
        #[cfg(unix)]
        terminate_remaining_group(
            self.pid,
            self.limits.termination_grace,
            self.limits.poll_interval,
        );
        self.reap_observed_child()
    }

    fn terminate_and_reap(&mut self, allow_eof_grace: bool) {
        self.close_stdin();
        if allow_eof_grace {
            let _ = self.wait_until(self.limits.cleanup_grace);
        } else {
            let _ = self.observe_status();
        }

        #[cfg(unix)]
        {
            terminate_remaining_group(
                self.pid,
                self.limits.termination_grace,
                self.limits.poll_interval,
            );
        }

        let leader_alive = self.status.is_none() && self.observe_status().ok().flatten().is_none();
        if leader_alive {
            if let Some(child) = self.child.as_mut() {
                let _ = child.kill();
            }
        }
        if let Some(mut child) = self.child.take() {
            if let Ok(status) = child.wait() {
                self.status = Some(status);
            }
        }
    }

    fn diagnostics(&mut self, stdout: &BoundedCapture) -> ProcessDiagnostics {
        let _ = self.observe_status();
        ProcessDiagnostics {
            command: self.command.clone(),
            pid: self.pid,
            status: self.status,
            stdout: stdout.snapshot(),
            stderr: self.stderr.snapshot(),
        }
    }

    fn finish_stderr_pump(&mut self) {
        if let Some(mut pump) = self.stderr_pump.take() {
            pump.finish_bounded(reader_join_timeout(self.limits));
        }
    }
}

fn missing_pipe(name: &'static str) -> HarnessError {
    HarnessError::Io {
        operation: "configure child process",
        source: io::Error::new(io::ErrorKind::BrokenPipe, format!("{name} was not piped")),
    }
}

fn spawn_capture_pump<R>(
    name: &str,
    mut reader: R,
    capture: BoundedCapture,
) -> io::Result<ReaderPump>
where
    R: Read + Send + 'static,
{
    ReaderPump::spawn(name, move || {
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => capture.append(&buffer[..read]),
            }
        }
    })
}

fn checked_deadline(timeout: Duration) -> Option<Instant> {
    Instant::now().checked_add(timeout)
}

fn remaining_until(deadline: Option<Instant>) -> Option<Duration> {
    let deadline = deadline?;
    let now = Instant::now();
    if now >= deadline {
        None
    } else {
        Some(deadline.duration_since(now))
    }
}

fn sleep_for_poll(remaining: Duration, poll: Duration) {
    let delay = remaining.min(poll);
    if delay.is_zero() {
        thread::yield_now();
    } else {
        thread::sleep(delay);
    }
}

fn reader_join_timeout(limits: HarnessLimits) -> Duration {
    limits
        .cleanup_grace
        .checked_add(limits.termination_grace)
        .unwrap_or(limits.termination_grace)
}

fn cleanup_spawn_failure(child: &mut Child, pid: u32) {
    #[cfg(unix)]
    {
        let _ = signal_process_group(pid, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn signal_process_group(pgid: u32, signal: i32) -> io::Result<()> {
    let pgid = i32::try_from(pgid).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("process group id {pgid} does not fit i32"),
        )
    })?;
    // SAFETY: `kill` takes integer values and dereferences no pointers. The
    // negative PID targets the process group created for this child at spawn.
    let result = unsafe { libc::kill(-pgid, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn process_group_alive(pgid: u32) -> bool {
    match signal_process_group(pgid, 0) {
        Ok(()) => true,
        Err(error) => error.raw_os_error() == Some(libc::EPERM),
    }
}

#[cfg(unix)]
fn terminate_remaining_group(pgid: u32, grace: Duration, poll: Duration) {
    if !process_group_alive(pgid) {
        return;
    }
    let _ = signal_process_group(pgid, libc::SIGTERM);
    let deadline = checked_deadline(grace);
    while process_group_alive(pgid) {
        let Some(remaining) = remaining_until(deadline) else {
            break;
        };
        sleep_for_poll(remaining, poll);
    }
    if process_group_alive(pgid) {
        let _ = signal_process_group(pgid, libc::SIGKILL);
    }
}

/// RAII owner for a child process with bounded capture and shutdown.
pub struct ProcessHarness {
    process: ManagedProcess,
    stdout: BoundedCapture,
    stdout_pump: Option<ReaderPump>,
}

impl ProcessHarness {
    pub fn spawn(command: &mut Command, limits: HarnessLimits) -> Result<Self, HarnessError> {
        let (mut process, stdout) = ManagedProcess::spawn(command, limits)?;
        let stdout_capture = BoundedCapture::new(limits.capture_bytes);
        let stdout_pump =
            match spawn_capture_pump("packet28-test-stdout", stdout, stdout_capture.clone()) {
                Ok(pump) => pump,
                Err(source) => {
                    process.terminate_and_reap(false);
                    process.finish_stderr_pump();
                    return Err(HarnessError::Io {
                        operation: "spawn stdout reader",
                        source,
                    });
                }
            };
        Ok(Self {
            process,
            stdout: stdout_capture,
            stdout_pump: Some(stdout_pump),
        })
    }

    pub fn run(
        command: &mut Command,
        input: &[u8],
        timeout: Duration,
        limits: HarnessLimits,
    ) -> Result<ProcessOutput, HarnessError> {
        let mut harness = Self::spawn(command, limits)?;
        harness.write_all(input)?;
        harness.finish(timeout)
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<(), HarnessError> {
        self.process.write_all(bytes)
    }

    pub fn close_stdin(&mut self) -> Result<(), HarnessError> {
        self.process.close_stdin();
        Ok(())
    }

    pub fn wait(&mut self, timeout: Duration) -> Result<ProcessOutput, HarnessError> {
        let status = match self.process.wait_until(timeout) {
            Ok(Some(_)) => match self.process.complete_successful_wait() {
                Ok(status) => status,
                Err(source) => {
                    self.process.terminate_and_reap(false);
                    self.finish_readers();
                    return Err(HarnessError::Io {
                        operation: "reap child process",
                        source,
                    });
                }
            },
            Ok(None) => {
                self.process.terminate_and_reap(false);
                self.finish_readers();
                return Err(HarnessError::Timeout {
                    operation: "wait for child process",
                    timeout,
                    diagnostics: Box::new(self.process.diagnostics(&self.stdout)),
                });
            }
            Err(source) => {
                self.process.terminate_and_reap(false);
                self.finish_readers();
                return Err(HarnessError::Io {
                    operation: "poll child process",
                    source,
                });
            }
        };
        self.finish_readers();
        let stdout = self.stdout.snapshot();
        let stderr = self.process.stderr.snapshot();
        Ok(ProcessOutput {
            status,
            stdout: stdout.bytes,
            stderr: stderr.bytes,
            stdout_truncated: stdout.truncated,
            stderr_truncated: stderr.truncated,
        })
    }

    pub fn finish(&mut self, timeout: Duration) -> Result<ProcessOutput, HarnessError> {
        self.close_stdin()?;
        self.wait(timeout)
    }

    pub fn diagnostics(&mut self) -> ProcessDiagnostics {
        self.process.diagnostics(&self.stdout)
    }

    pub fn pid(&self) -> u32 {
        self.process.pid
    }

    fn finish_readers(&mut self) {
        if let Some(mut pump) = self.stdout_pump.take() {
            pump.finish_bounded(reader_join_timeout(self.process.limits));
        }
        self.process.finish_stderr_pump();
    }
}

impl Drop for ProcessHarness {
    fn drop(&mut self) {
        self.process.terminate_and_reap(true);
        self.finish_readers();
    }
}

type McpEvent = Result<Value, String>;

/// RAII owner for a Content-Length-framed MCP child process.
pub struct McpHarness {
    process: ManagedProcess,
    stdout: BoundedCapture,
    reader: Option<ReaderPump>,
    events: Option<Receiver<McpEvent>>,
    reader_failure: Arc<Mutex<Option<String>>>,
    mailbox: VecDeque<Value>,
    limits: HarnessLimits,
    next_id: u64,
}

impl McpHarness {
    pub fn spawn(command: &mut Command, limits: HarnessLimits) -> Result<Self, HarnessError> {
        let (mut process, stdout) = ManagedProcess::spawn(command, limits)?;
        let stdout_capture = BoundedCapture::new(limits.capture_bytes);
        let (sender, events) = mpsc::sync_channel(limits.mcp_queue_capacity.max(1));
        let reader_failure = Arc::new(Mutex::new(None));
        let reader = match spawn_mcp_reader(
            stdout,
            stdout_capture.clone(),
            sender,
            Arc::clone(&reader_failure),
            limits,
        ) {
            Ok(reader) => reader,
            Err(source) => {
                process.terminate_and_reap(false);
                process.finish_stderr_pump();
                return Err(HarnessError::Io {
                    operation: "spawn MCP reader",
                    source,
                });
            }
        };
        Ok(Self {
            process,
            stdout: stdout_capture,
            reader: Some(reader),
            events: Some(events),
            reader_failure,
            mailbox: VecDeque::new(),
            limits,
            next_id: 1,
        })
    }

    pub fn send_value(&mut self, value: &Value) -> Result<(), HarnessError> {
        let body = serde_json::to_vec(value)
            .map_err(|error| self.mcp_error(format!("serialize MCP message: {error}")))?;
        if body.len() > self.limits.mcp_message_bytes {
            return Err(self.mcp_error(format!(
                "outgoing MCP message is {} bytes; limit is {} bytes",
                body.len(),
                self.limits.mcp_message_bytes
            )));
        }
        let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        framed.extend_from_slice(&body);
        self.raw_send(&framed)
    }

    pub fn raw_send(&mut self, bytes: &[u8]) -> Result<(), HarnessError> {
        self.process.write_all(bytes)
    }

    pub fn receive(&mut self, timeout: Duration) -> Result<Value, HarnessError> {
        if let Some(message) = self.mailbox.pop_front() {
            return Ok(message);
        }
        self.receive_direct(timeout)
    }

    pub fn recv_for_id(
        &mut self,
        expected_id: &Value,
        timeout: Duration,
    ) -> Result<Value, HarnessError> {
        if let Some(index) = self
            .mailbox
            .iter()
            .position(|message| message.get("id") == Some(expected_id))
        {
            return Ok(self
                .mailbox
                .remove(index)
                .expect("mailbox index came from position"));
        }

        let deadline = checked_deadline(timeout);
        loop {
            let remaining = remaining_until(deadline).unwrap_or(Duration::ZERO);
            let message = self.receive_direct(remaining)?;
            if message.get("id") == Some(expected_id) {
                return Ok(message);
            }
            if self.mailbox.len() >= self.limits.mcp_queue_capacity.max(1) {
                return Err(self.mcp_error(format!(
                    "unmatched MCP mailbox reached its {} message limit",
                    self.limits.mcp_queue_capacity.max(1)
                )));
            }
            self.mailbox.push_back(message);
        }
    }

    pub fn request_with_id(
        &mut self,
        id: Value,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, HarnessError> {
        if let Some(id_number) = id.as_u64() {
            self.next_id = self.next_id.max(id_number.saturating_add(1));
        }
        self.send_value(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        self.recv_for_id(&id, timeout)
    }

    pub fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, HarnessError> {
        let id = json!(self.next_id);
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        self.request_with_id(id, method, params, timeout)
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), HarnessError> {
        self.send_value(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    pub fn close_stdin(&mut self) -> Result<(), HarnessError> {
        self.process.close_stdin();
        Ok(())
    }

    pub fn wait(&mut self, timeout: Duration) -> Result<ProcessOutput, HarnessError> {
        let status = match self.process.wait_until(timeout) {
            Ok(Some(_)) => match self.process.complete_successful_wait() {
                Ok(status) => status,
                Err(source) => {
                    self.process.terminate_and_reap(false);
                    self.finish_readers();
                    return Err(HarnessError::Io {
                        operation: "reap MCP child process",
                        source,
                    });
                }
            },
            Ok(None) => {
                self.process.terminate_and_reap(false);
                self.finish_readers();
                return Err(HarnessError::Timeout {
                    operation: "wait for MCP child process",
                    timeout,
                    diagnostics: Box::new(self.process.diagnostics(&self.stdout)),
                });
            }
            Err(source) => {
                self.process.terminate_and_reap(false);
                self.finish_readers();
                return Err(HarnessError::Io {
                    operation: "poll MCP child process",
                    source,
                });
            }
        };
        self.finish_readers();
        let stdout = self.stdout.snapshot();
        let stderr = self.process.stderr.snapshot();
        Ok(ProcessOutput {
            status,
            stdout: stdout.bytes,
            stderr: stderr.bytes,
            stdout_truncated: stdout.truncated,
            stderr_truncated: stderr.truncated,
        })
    }

    pub fn finish(&mut self, timeout: Duration) -> Result<ProcessOutput, HarnessError> {
        self.close_stdin()?;
        self.wait(timeout)
    }

    pub fn diagnostics(&mut self) -> ProcessDiagnostics {
        self.process.diagnostics(&self.stdout)
    }

    pub fn pid(&self) -> u32 {
        self.process.pid
    }

    fn receive_direct(&mut self, timeout: Duration) -> Result<Value, HarnessError> {
        let event = {
            let Some(events) = self.events.as_ref() else {
                return Err(self.mcp_error("MCP reader is closed"));
            };
            events.recv_timeout(timeout)
        };
        match event {
            Ok(Ok(message)) => Ok(message),
            Ok(Err(message)) => Err(self.mcp_error(message)),
            Err(RecvTimeoutError::Timeout) => Err(HarnessError::Timeout {
                operation: "receive MCP message",
                timeout,
                diagnostics: Box::new(self.process.diagnostics(&self.stdout)),
            }),
            Err(RecvTimeoutError::Disconnected) => {
                let message = self
                    .reader_failure
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .unwrap_or_else(|| "MCP reader disconnected unexpectedly".to_owned());
                Err(self.mcp_error(message))
            }
        }
    }

    fn mcp_error(&mut self, message: impl Into<String>) -> HarnessError {
        HarnessError::Mcp {
            message: message.into(),
            diagnostics: Box::new(self.process.diagnostics(&self.stdout)),
        }
    }

    fn finish_readers(&mut self) {
        if let Some(mut reader) = self.reader.take() {
            reader.finish_bounded(reader_join_timeout(self.limits));
        }
        self.process.finish_stderr_pump();
    }
}

impl Drop for McpHarness {
    fn drop(&mut self) {
        let _ = self.events.take();
        self.process.terminate_and_reap(true);
        self.finish_readers();
    }
}

fn spawn_mcp_reader(
    stdout: ChildStdout,
    capture: BoundedCapture,
    sender: SyncSender<McpEvent>,
    reader_failure: Arc<Mutex<Option<String>>>,
    limits: HarnessLimits,
) -> io::Result<ReaderPump> {
    ReaderPump::spawn("packet28-test-mcp-reader", move || {
        let reader = CapturingReader {
            inner: stdout,
            capture,
        };
        let mut reader = BufReader::new(reader);
        loop {
            match read_mcp_message(
                &mut reader,
                limits.mcp_header_bytes,
                limits.mcp_message_bytes,
            ) {
                Ok(Some(message)) => match sender.try_send(Ok(message)) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        set_reader_failure(
                            &reader_failure,
                            format!(
                                "MCP response queue reached its {} message limit",
                                limits.mcp_queue_capacity.max(1)
                            ),
                        );
                        break;
                    }
                    Err(TrySendError::Disconnected(_)) => break,
                },
                Ok(None) => {
                    set_reader_failure(&reader_failure, "MCP stdout closed".to_owned());
                    break;
                }
                Err(message) => {
                    set_reader_failure(&reader_failure, message.clone());
                    let _ = sender.try_send(Err(message));
                    break;
                }
            }
        }
    })
}

fn set_reader_failure(reader_failure: &Arc<Mutex<Option<String>>>, message: String) {
    let mut failure = reader_failure
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if failure.is_none() {
        *failure = Some(message);
    }
}

fn read_mcp_message<R: Read>(
    reader: &mut R,
    header_limit: usize,
    message_limit: usize,
) -> Result<Option<Value>, String> {
    let mut header = Vec::with_capacity(header_limit.min(1024));
    loop {
        let mut byte = [0_u8; 1];
        match reader.read(&mut byte) {
            Ok(0) if header.is_empty() => return Ok(None),
            Ok(0) => return Err("MCP stdout ended in the middle of a header".to_owned()),
            Ok(_) => {
                if header.len() >= header_limit {
                    return Err(format!("MCP header exceeds the {header_limit} byte limit"));
                }
                header.push(byte[0]);
                if header.ends_with(b"\r\n\r\n") || header.ends_with(b"\n\n") {
                    break;
                }
            }
            Err(error) => return Err(format!("read MCP header: {error}")),
        }
    }

    let header = std::str::from_utf8(&header)
        .map_err(|error| format!("MCP header is not UTF-8: {error}"))?;
    let mut content_length = None;
    for line in header.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| format!("malformed MCP header line: {line:?}"))?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err("MCP frame has duplicate Content-Length headers".to_owned());
            }
            let parsed = value
                .trim()
                .parse::<usize>()
                .map_err(|error| format!("invalid MCP Content-Length: {error}"))?;
            content_length = Some(parsed);
        }
    }
    let content_length =
        content_length.ok_or_else(|| "MCP frame is missing Content-Length".to_owned())?;
    if content_length > message_limit {
        return Err(format!(
            "MCP message is {content_length} bytes and exceeds the {message_limit} byte limit"
        ));
    }

    let mut body = vec![0_u8; content_length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("read MCP body: {error}"))?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| format!("decode MCP JSON body: {error}"))
}
