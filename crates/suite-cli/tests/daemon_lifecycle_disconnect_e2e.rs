#[path = "support/daemon_lifecycle.rs"]
mod daemon_lifecycle;

use daemon_lifecycle::{ensure_packet28d_built, init_repo, suite_cmd, write_repo_fixture};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

#[test]
#[cfg(unix)]
fn test_daemon_lifecycle_cli_suppresses_disconnect_log_noise() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    init_repo(dir.path());

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let status_output = suite_cmd()
        .args([
            "daemon",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: Value = serde_json::from_slice(&status_output).unwrap();
    let socket = PathBuf::from(status.get("socket_path").and_then(Value::as_str).unwrap());
    let start = std::time::Instant::now();
    let mut stream = loop {
        match std::os::unix::net::UnixStream::connect(&socket) {
            Ok(stream) => break stream,
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound
                    && start.elapsed() < Duration::from_secs(15) =>
            {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(err) => panic!(
                "failed to connect to daemon socket {}: {err}",
                socket.display()
            ),
        }
    };
    packet28_daemon_protocol::frame::write_frame(
        &mut stream,
        &packet28_daemon_protocol::message::DaemonRequest::Status,
    )
    .unwrap();
    drop(stream);

    std::thread::sleep(Duration::from_millis(300));

    let log_path = dir.path().join(".packet28/daemon/packet28d.log");
    let start = std::time::Instant::now();
    while !log_path.exists() && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(25));
    }
    let log = fs::read_to_string(&log_path).unwrap();
    assert!(!log.contains("request handling failed: Broken pipe"));
    assert!(!log.contains("request handling failed: Connection reset"));
    assert!(!log.contains("request handling failed: unexpected end of file"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
