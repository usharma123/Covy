#![cfg(unix)]

#[expect(
    dead_code,
    reason = "this focused test binary intentionally exercises selected harness operations"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use std::fs;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use process_harness::{HarnessLimits, McpHarness, ProcessHarness};
use serde_json::{json, Value};
use tempfile::TempDir;

const FAST_TIMEOUT: Duration = Duration::from_millis(150);
const TEST_TIMEOUT: Duration = Duration::from_secs(3);

fn test_limits() -> HarnessLimits {
    HarnessLimits::default()
        .with_capture_bytes(16 * 1024)
        .with_mcp_message_bytes(16 * 1024)
        .with_mcp_header_bytes(4 * 1024)
        .with_mcp_queue_capacity(16)
        .with_cleanup_grace(Duration::from_millis(100))
        .with_termination_grace(Duration::from_millis(100))
        .with_poll_interval(Duration::from_millis(5))
}

fn python(script: &str) -> Command {
    let mut command = Command::new("python3");
    command.args(["-u", "-c", script]);
    command
}

fn content_length_frame(value: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(value).unwrap();
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(&body);
    frame
}

fn wait_for_file(path: &Path, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(value) = fs::read_to_string(path) {
            if !value.trim().is_empty() {
                return value;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn process_is_absent(pid: u32) -> bool {
    // SAFETY: signal zero does not mutate the target; it only checks whether the
    // kernel still has a process with this PID.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

fn wait_for_process_absence(pid: u32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !process_is_absent(pid) {
        assert!(
            Instant::now() < deadline,
            "process {pid} remained alive after harness cleanup"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn process_wait_timeout_reaps_and_reports_bounded_captured_diagnostics() {
    const CAPTURE_LIMIT: usize = 64;
    const STDOUT_TAIL: &[u8] = b"stdout-tail-marker";
    const STDERR_TAIL: &[u8] = b"stderr-tail-marker";
    let mut command = python(
        r#"
import sys, time
sys.stdout.buffer.write((b"o" * 256) + b"stdout-tail-marker")
sys.stderr.buffer.write((b"e" * 256) + b"stderr-tail-marker")
sys.stdout.buffer.flush()
sys.stderr.buffer.flush()
time.sleep(30)
"#,
    );
    let limits = test_limits().with_capture_bytes(CAPTURE_LIMIT);

    let mut harness = ProcessHarness::spawn(&mut command, limits).unwrap();
    let pid = harness.pid();
    let capture_deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        let diagnostics = harness.diagnostics();
        if diagnostics.stdout.total_bytes > u64::try_from(STDOUT_TAIL.len()).unwrap()
            && diagnostics.stderr.total_bytes > u64::try_from(STDERR_TAIL.len()).unwrap()
        {
            break;
        }
        assert!(
            Instant::now() < capture_deadline,
            "child did not emit diagnostics before timeout"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();
    let error = match harness.wait(FAST_TIMEOUT) {
        Ok(_) => panic!("long-running command unexpectedly completed"),
        Err(error) => error,
    };
    wait_for_process_absence(pid, TEST_TIMEOUT);
    let rendered = error.to_string();
    let diagnostics = error
        .diagnostics()
        .expect("timeout errors should include process diagnostics");
    let mut wait_status = 0;
    // SAFETY: `pid` was this test's direct child. ECHILD proves timeout
    // cleanup waited for it instead of leaving a waitable zombie.
    let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut wait_status, libc::WNOHANG) };
    let wait_error = io::Error::last_os_error().raw_os_error();

    assert!(
        started.elapsed() < TEST_TIMEOUT
            && rendered.contains("timed out")
            && diagnostics.stdout.bytes.len() <= CAPTURE_LIMIT
            && diagnostics.stderr.bytes.len() <= CAPTURE_LIMIT
            && diagnostics.stdout.truncated
            && diagnostics.stderr.truncated
            && diagnostics.stdout.total_bytes
                > u64::try_from(diagnostics.stdout.bytes.len()).unwrap()
            && diagnostics.stderr.total_bytes
                > u64::try_from(diagnostics.stderr.bytes.len()).unwrap()
            && diagnostics.stdout.bytes.ends_with(STDOUT_TAIL)
            && diagnostics.stderr.bytes.ends_with(STDERR_TAIL)
            && waited == -1
            && wait_error == Some(libc::ECHILD),
        "unexpected timeout diagnostics after {:?}: {rendered}",
        started.elapsed()
    );
}

#[test]
fn process_stdin_write_timeout_kills_group_and_reaps_non_reader() {
    let mut command = python(
        r#"
import time
time.sleep(30)
"#,
    );
    let mut harness = ProcessHarness::spawn(&mut command, test_limits()).unwrap();
    let pid = harness.pid();
    let payload = vec![b'x'; 2 * 1024 * 1024];

    let started = Instant::now();
    let error = match harness.write_all(&payload, FAST_TIMEOUT) {
        Ok(()) => panic!("large write to a non-reading child unexpectedly completed"),
        Err(error) => error,
    };
    wait_for_process_absence(pid, TEST_TIMEOUT);
    let diagnostics = error
        .diagnostics()
        .expect("write timeout should include process diagnostics");
    let mut wait_status = 0;
    // SAFETY: `pid` was this test's direct child. ECHILD proves the timed
    // write killed and reaped it before returning the timeout.
    let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut wait_status, libc::WNOHANG) };
    let wait_error = io::Error::last_os_error().raw_os_error();

    assert!(
        started.elapsed() < TEST_TIMEOUT
            && error.to_string().contains("write child stdin timed out")
            && diagnostics.pid == pid
            && diagnostics.status.is_some()
            && waited == -1
            && wait_error == Some(libc::ECHILD),
        "unexpected blocked-write result after {:?}: {error}",
        started.elapsed()
    );
}

#[test]
fn mcp_raw_send_timeout_kills_group_and_reaps_non_reader() {
    let mut command = python(
        r#"
import time
time.sleep(30)
"#,
    );
    let mut harness = McpHarness::spawn(&mut command, test_limits()).unwrap();
    let pid = harness.pid();
    let payload = vec![b'x'; 2 * 1024 * 1024];

    let started = Instant::now();
    let error = match harness.raw_send(&payload, FAST_TIMEOUT) {
        Ok(()) => panic!("large raw MCP write to a non-reading child unexpectedly completed"),
        Err(error) => error,
    };
    wait_for_process_absence(pid, TEST_TIMEOUT);
    let diagnostics = error
        .diagnostics()
        .expect("raw MCP write timeout should include process diagnostics");
    let mut wait_status = 0;
    // SAFETY: `pid` was this test's direct child. ECHILD proves the timed
    // write killed and reaped it before returning the timeout.
    let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut wait_status, libc::WNOHANG) };
    let wait_error = io::Error::last_os_error().raw_os_error();

    assert!(
        started.elapsed() < TEST_TIMEOUT
            && error.to_string().contains("write MCP stdin timed out")
            && diagnostics.pid == pid
            && diagnostics.status.is_some()
            && waited == -1
            && wait_error == Some(libc::ECHILD),
        "unexpected blocked raw-send result after {:?}: {error}",
        started.elapsed()
    );
}

#[test]
fn mcp_request_deadline_includes_blocked_write_and_reaps_non_reader() {
    let mut command = python(
        r#"
import time
time.sleep(30)
"#,
    );
    let limits = test_limits().with_mcp_message_bytes(4 * 1024 * 1024);
    let mut harness = McpHarness::spawn(&mut command, limits).unwrap();
    let pid = harness.pid();

    let started = Instant::now();
    let error = match harness.request(
        "blocked",
        json!({"payload": "x".repeat(2 * 1024 * 1024)}),
        FAST_TIMEOUT,
    ) {
        Ok(value) => panic!("request to a non-reading child unexpectedly returned {value}"),
        Err(error) => error,
    };
    wait_for_process_absence(pid, TEST_TIMEOUT);
    let diagnostics = error
        .diagnostics()
        .expect("request write timeout should include process diagnostics");
    let mut wait_status = 0;
    // SAFETY: `pid` was this test's direct child. ECHILD proves the request
    // deadline killed and reaped it while its stdin write was blocked.
    let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut wait_status, libc::WNOHANG) };
    let wait_error = io::Error::last_os_error().raw_os_error();

    assert!(
        started.elapsed() < TEST_TIMEOUT
            && error.to_string().contains("write MCP stdin timed out")
            && diagnostics.pid == pid
            && diagnostics.status.is_some()
            && waited == -1
            && wait_error == Some(libc::ECHILD),
        "unexpected blocked request result after {:?}: {error}",
        started.elapsed()
    );
}

#[test]
fn mcp_request_response_timeout_kills_group_and_reaps_silent_reader() {
    let mut command = python(
        r#"
import sys, time

headers = {}
while True:
    line = sys.stdin.buffer.readline()
    if line in (b"\r\n", b"\n"):
        break
    name, value = line.decode("utf-8").split(":", 1)
    headers[name.lower().strip()] = value.strip()
sys.stdin.buffer.read(int(headers["content-length"]))
time.sleep(30)
"#,
    );
    let mut harness = McpHarness::spawn(&mut command, test_limits()).unwrap();
    let pid = harness.pid();

    let started = Instant::now();
    let error = match harness.request("silent", json!({}), FAST_TIMEOUT) {
        Ok(value) => panic!("silent child unexpectedly returned {value}"),
        Err(error) => error,
    };
    wait_for_process_absence(pid, TEST_TIMEOUT);
    let diagnostics = error
        .diagnostics()
        .expect("response timeout should include process diagnostics");
    let mut wait_status = 0;
    // SAFETY: `pid` was this test's direct child. ECHILD proves the response
    // deadline killed and reaped the silent reader before returning.
    let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut wait_status, libc::WNOHANG) };
    let wait_error = io::Error::last_os_error().raw_os_error();

    assert!(
        started.elapsed() < TEST_TIMEOUT
            && error.to_string().contains("receive MCP message timed out")
            && diagnostics.pid == pid
            && diagnostics.status.is_some()
            && waited == -1
            && wait_error == Some(libc::ECHILD),
        "unexpected silent-reader result after {:?}: {error}",
        started.elapsed()
    );
}

#[test]
fn mcp_receive_rejects_malformed_content_length() {
    let mut command = python(
        r#"
import sys
sys.stdout.buffer.write(b"Content-Length: invalid\r\n\r\n{}")
sys.stdout.buffer.flush()
"#,
    );
    let mut harness = McpHarness::spawn(&mut command, test_limits()).unwrap();

    let error = match harness.receive(TEST_TIMEOUT) {
        Ok(value) => panic!("malformed frame unexpectedly decoded as {value}"),
        Err(error) => error,
    };
    let pid = harness.pid();
    let diagnostics = harness.diagnostics();

    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("content-length")
            && diagnostics.pid == pid,
        "unexpected malformed-frame error: {error}"
    );
}

#[test]
fn mcp_receive_rejects_truncated_body() {
    let mut command = python(
        r#"
import sys
sys.stdout.buffer.write(b"Content-Length: 20\r\n\r\n{}")
sys.stdout.buffer.flush()
"#,
    );
    let mut harness = McpHarness::spawn(&mut command, test_limits()).unwrap();

    let error = match harness.receive(TEST_TIMEOUT) {
        Ok(value) => panic!("truncated frame unexpectedly decoded as {value}"),
        Err(error) => error,
    };
    let rendered = error.to_string().to_ascii_lowercase();

    assert!(
        rendered.contains("truncated")
            || rendered.contains("unexpected eof")
            || rendered.contains("failed to fill whole buffer"),
        "unexpected truncated-frame error: {error}"
    );
}

#[test]
fn mcp_receive_rejects_oversized_body_before_reading_it() {
    let limits = test_limits().with_mcp_message_bytes(32);
    let mut command = python(
        r#"
import sys, time
sys.stdout.buffer.write(b"Content-Length: 33\r\n\r\n")
sys.stdout.buffer.flush()
time.sleep(30)
"#,
    );
    let mut harness = McpHarness::spawn(&mut command, limits).unwrap();

    let started = Instant::now();
    let error = match harness.receive(TEST_TIMEOUT) {
        Ok(value) => panic!("oversized frame unexpectedly decoded as {value}"),
        Err(error) => error,
    };

    assert!(
        started.elapsed() < TEST_TIMEOUT
            && error.to_string().to_ascii_lowercase().contains("limit"),
        "unexpected oversized-frame error after {:?}: {error}",
        started.elapsed()
    );
}

#[test]
fn mcp_receive_rejects_oversized_header_before_delimiter() {
    let limits = test_limits().with_mcp_header_bytes(32);
    let mut command = python(
        r#"
import sys, time
sys.stdout.buffer.write(b"X" * 33)
sys.stdout.buffer.flush()
time.sleep(30)
"#,
    );
    let mut harness = McpHarness::spawn(&mut command, limits).unwrap();

    let started = Instant::now();
    let error = match harness.receive(TEST_TIMEOUT) {
        Ok(value) => panic!("oversized header unexpectedly decoded as {value}"),
        Err(error) => error,
    };

    assert!(
        started.elapsed() < TEST_TIMEOUT
            && error
                .to_string()
                .to_ascii_lowercase()
                .contains("header exceeds"),
        "unexpected oversized-header error after {:?}: {error}",
        started.elapsed()
    );
}

#[test]
fn mcp_recv_for_id_preserves_arbitrary_unmatched_ids() {
    let mut command = python(
        r#"
import json, sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    return json.loads(sys.stdin.buffer.read(int(headers["content-length"])))

def write_message(value):
    body = json.dumps(value, separators=(",", ":")).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

first = read_message()
second = read_message()
write_message({"jsonrpc": "2.0", "id": first["id"], "result": {"value": "string"}})
write_message({"jsonrpc": "2.0", "id": second["id"], "result": {"value": "numeric"}})
sys.stdin.buffer.read()
"#,
    );
    let mut harness = McpHarness::spawn(&mut command, test_limits()).unwrap();
    let string_id = json!("request-string");
    let numeric_id = json!(17);

    harness
        .send_value(
            &json!({
                "jsonrpc": "2.0",
                "id": string_id,
                "method": "echo"
            }),
            TEST_TIMEOUT,
        )
        .unwrap();
    harness
        .send_value(
            &json!({
                "jsonrpc": "2.0",
                "id": numeric_id,
                "method": "echo"
            }),
            TEST_TIMEOUT,
        )
        .unwrap();

    let numeric = harness.recv_for_id(&numeric_id, TEST_TIMEOUT).unwrap();
    let string = harness.recv_for_id(&string_id, TEST_TIMEOUT).unwrap();
    harness.close_stdin().unwrap();
    let output = harness.finish(TEST_TIMEOUT).unwrap();

    assert_eq!(
        (
            numeric["id"].clone(),
            numeric["result"]["value"].clone(),
            string["id"].clone(),
            string["result"]["value"].clone(),
            output.status.success(),
        ),
        (
            numeric_id,
            json!("numeric"),
            string_id,
            json!("string"),
            true,
        )
    );
}

#[test]
fn mcp_half_close_drains_valid_response_before_exit() {
    let mut command = python(
        r#"
import json, sys

headers = {}
while True:
    line = sys.stdin.buffer.readline()
    if line in (b"\r\n", b"\n"):
        break
    name, value = line.decode("utf-8").split(":", 1)
    headers[name.lower().strip()] = value.strip()
request = json.loads(sys.stdin.buffer.read(int(headers["content-length"])))
sys.stdin.buffer.read()
body = json.dumps({
    "jsonrpc": "2.0",
    "id": request["id"],
    "result": {"drained": True}
}, separators=(",", ":")).encode("utf-8")
sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii"))
sys.stdout.buffer.write(body)
sys.stdout.buffer.flush()
"#,
    );
    let mut harness = McpHarness::spawn(&mut command, test_limits()).unwrap();
    let request = json!({
        "jsonrpc": "2.0",
        "id": "half-close",
        "method": "drain"
    });

    harness
        .raw_send(&content_length_frame(&request), TEST_TIMEOUT)
        .unwrap();
    harness.close_stdin().unwrap();
    let response = harness.receive(TEST_TIMEOUT).unwrap();
    let output = harness.finish(TEST_TIMEOUT).unwrap();

    assert_eq!(
        (
            response["id"].clone(),
            response["result"]["drained"].clone(),
            output.status.success(),
        ),
        (json!("half-close"), json!(true), true)
    );
}

#[test]
fn process_harness_unwind_kills_group_and_reaps_leader_and_grandchild() {
    let temp = TempDir::new().unwrap();
    let pid_file = temp.path().join("process-group-pids");
    let mut command = Command::new("sh");
    command.env("PID_FILE", &pid_file).args([
        "-c",
        "sleep 30 & grandchild=$!; printf '%s %s\n' \"$$\" \"$grandchild\" > \"$PID_FILE\"; wait",
    ]);
    let harness = ProcessHarness::spawn(&mut command, test_limits()).unwrap();
    let leader = harness.pid();
    let pids = wait_for_file(&pid_file, TEST_TIMEOUT);
    let mut pids = pids.split_whitespace().map(|value| {
        value
            .parse::<u32>()
            .unwrap_or_else(|error| panic!("invalid PID {value:?}: {error}"))
    });
    let reported_leader = pids.next().unwrap();
    let grandchild = pids.next().unwrap();

    let unwind = catch_unwind(AssertUnwindSafe(move || {
        let _harness = harness;
        panic!("intentional unwind exercises ProcessHarness::drop");
    }));
    wait_for_process_absence(leader, TEST_TIMEOUT);
    wait_for_process_absence(grandchild, TEST_TIMEOUT);

    let mut status = 0;
    // SAFETY: `leader` was this test's child PID. ECHILD proves the harness
    // already waited for it instead of leaving a waitable zombie.
    let waited = unsafe { libc::waitpid(leader as libc::pid_t, &mut status, libc::WNOHANG) };
    let wait_error = io::Error::last_os_error().raw_os_error();

    assert_eq!(
        (unwind.is_err(), reported_leader, waited, wait_error),
        (true, leader, -1, Some(libc::ECHILD))
    );
}
