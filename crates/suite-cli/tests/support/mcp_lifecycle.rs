use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

pub fn corrupt_task_event_log(root: &Path, task_id: &str) {
    use packet28_daemon_protocol::paths::{task_event_log_path, TaskStorageId};

    let storage_id = TaskStorageId::try_from(task_id).unwrap();
    let event_path = task_event_log_path(root, &storage_id);
    fs::create_dir_all(event_path.parent().unwrap()).unwrap();
    if event_path.exists() {
        fs::remove_file(&event_path).unwrap();
    }
    fs::create_dir(&event_path).unwrap();
    assert!(
        packet28_daemon_core::storage::load_task_events_from_offset(root, task_id, 0).is_err(),
        "unreadable event-log fixture remained readable"
    );
}

pub fn wait_for_file(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for fixture marker '{}'",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn small_buffered_stdout_pair() -> (UnixStream, UnixStream) {
    let (child_stdout, parent_stdout) = UnixStream::pair().unwrap();
    set_socket_send_buffer(&child_stdout);
    set_socket_send_buffer(&parent_stdout);
    (child_stdout, parent_stdout)
}

fn set_socket_send_buffer(stream: &UnixStream) {
    let requested: libc::c_int = 4 * 1024;
    // SAFETY: `stream` owns a valid socket descriptor and the option
    // pointer references an initialized integer for the duration of the call.
    let result = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            (&raw const requested).cast(),
            std::mem::size_of_val(&requested)
                .try_into()
                .expect("socket option length fits socklen_t"),
        )
    };
    assert_eq!(
        result,
        0,
        "failed to bound fixture stdout socket: {}",
        std::io::Error::last_os_error()
    );
}

fn socket_send_buffer(stream: &UnixStream) -> usize {
    let mut size: libc::c_int = 0;
    let mut length: libc::socklen_t = std::mem::size_of_val(&size)
        .try_into()
        .expect("socket option length fits socklen_t");
    // SAFETY: `stream` owns a valid socket descriptor and both output
    // pointers remain initialized and writable for the duration of the call.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            (&raw mut size).cast(),
            &raw mut length,
        )
    };
    assert_eq!(
        result,
        0,
        "failed to read fixture stdout socket size: {}",
        std::io::Error::last_os_error()
    );
    usize::try_from(size).expect("socket send buffer is non-negative")
}

fn socket_pending_bytes(stream: &UnixStream) -> usize {
    let mut pending: libc::c_int = 0;
    // SAFETY: `stream` owns a valid socket descriptor and `pending` is a
    // writable integer used by `FIONREAD`.
    let result = unsafe { libc::ioctl(stream.as_raw_fd(), libc::FIONREAD, &raw mut pending) };
    assert_eq!(
        result,
        0,
        "failed to inspect fixture stdout socket: {}",
        std::io::Error::last_os_error()
    );
    usize::try_from(pending).expect("pending byte count is non-negative")
}

pub fn write_content_length_message(writer: &mut impl Write, value: &Value) -> usize {
    let body = serde_json::to_vec(value).unwrap();
    write!(writer, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    writer.write_all(&body).unwrap();
    writer.flush().unwrap();
    body.len()
}

pub fn read_content_length_message(reader: &mut impl BufRead) -> Value {
    let mut content_length = None;
    let mut line = String::new();
    loop {
        line.clear();
        assert_ne!(reader.read_line(&mut line).unwrap(), 0);
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:").map(str::trim) {
            content_length = Some(value.parse::<usize>().unwrap());
        }
    }
    let mut body = vec![0; content_length.expect("missing Content-Length")];
    std::io::Read::read_exact(reader, &mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

pub fn write_newline_message(writer: &mut impl Write, value: &Value) -> usize {
    let body = serde_json::to_vec(value).unwrap();
    writer.write_all(&body).unwrap();
    writer.write_all(b"\n").unwrap();
    writer.flush().unwrap();
    body.len()
}

pub fn read_newline_message(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
    serde_json::from_str(&line).unwrap()
}

pub fn wait_for_stdout_backpressure(
    stdout: &UnixStream,
    response_lower_bound: usize,
    timeout: Duration,
) {
    let send_buffer = socket_send_buffer(stdout);
    assert!(
        response_lower_bound > send_buffer.saturating_mul(4),
        "fixture response is not large enough to prove stdout backpressure"
    );
    let minimum_pending = (send_buffer / 4).max(1);
    let deadline = Instant::now() + timeout;
    while socket_pending_bytes(stdout) < minimum_pending {
        assert!(
            Instant::now() < deadline,
            "stdout never reached its bounded socket capacity"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(50));
}

pub fn wait_for_child(
    child: &mut std::process::Child,
    timeout: Duration,
) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn large_response_batch() -> Value {
    let id_suffix = "x".repeat(32 * 1024);
    Value::Array(
        (0..128)
            .map(|index| {
                json!({
                    "jsonrpc":"2.0",
                    "id":format!("{index}-{id_suffix}"),
                    "method":"prompts/list"
                })
            })
            .collect(),
    )
}
