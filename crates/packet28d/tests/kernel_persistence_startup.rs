use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use packet28_daemon_protocol::paths::{
    pid_path, ready_path, runtime_path, socket_path, workspace_socket_path,
};

#[test]
fn daemon_rejects_primary_kernel_persistence_failure_before_readiness() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let cache_dir = workspace.path().join(".packet28");
    std::fs::create_dir(&cache_dir).expect("create cache directory");
    std::fs::create_dir(cache_dir.join("packet-cache-v3.lock"))
        .expect("create invalid persistence lock directory");
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_packet28d"))
        .args(["serve", "--root"])
        .arg(workspace.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn packet28d");

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = daemon.try_wait().expect("probe daemon") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not reject the persistence failure before timeout"
        );
        thread::sleep(Duration::from_millis(10));
    };
    let mut stderr = String::new();
    daemon
        .stderr
        .take()
        .expect("captured daemon stderr")
        .read_to_string(&mut stderr)
        .expect("read daemon stderr");

    assert!(
        !status.success(),
        "daemon unexpectedly started successfully"
    );
    assert!(
        stderr.contains("failed to open primary persistent kernel")
            && stderr.contains("cache persistence failed"),
        "daemon did not report the primary persistence failure: {stderr:?}"
    );
    assert!(
        !ready_path(workspace.path()).exists(),
        "daemon advertised readiness without persistent kernel ownership"
    );
    assert!(
        !pid_path(workspace.path()).exists(),
        "daemon leaked its pid file after startup failure"
    );
    assert!(
        !runtime_path(workspace.path()).exists(),
        "daemon leaked runtime discovery after startup failure"
    );
    assert!(
        !socket_path(workspace.path()).exists()
            && !workspace_socket_path(workspace.path()).exists(),
        "daemon leaked socket discovery after startup failure"
    );
}
