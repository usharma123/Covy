use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use packet28_daemon_core::task_store_lease::try_acquire_task_store_retention_lease;
use packet28_daemon_protocol::paths::{ready_path, runtime_path};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn daemon_startup_waits_for_exclusive_task_store_maintenance() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let retention_lease = try_acquire_task_store_retention_lease(workspace.path())
        .expect("retention lease attempt")
        .expect("exclusive retention lease");
    let mut daemon = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_packet28d"))
            .args(["serve", "--root"])
            .arg(workspace.path())
            .env("PACKET28D_MAX_CONNECTIONS", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn packet28d"),
    );

    thread::sleep(Duration::from_millis(250));
    assert!(
        daemon.0.try_wait().expect("probe daemon").is_none(),
        "daemon exited instead of waiting for the task-store lease"
    );
    assert!(
        !runtime_path(workspace.path()).exists(),
        "daemon published runtime metadata before acquiring its shared lease"
    );
    assert!(
        !ready_path(workspace.path()).exists(),
        "daemon published readiness before acquiring its shared lease"
    );

    drop(retention_lease);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = daemon.0.try_wait().expect("probe released daemon") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not cross the task-store lease boundary after release"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    daemon
        .0
        .stderr
        .take()
        .expect("captured daemon stderr")
        .read_to_string(&mut stderr)
        .expect("read daemon stderr");
    assert!(
        !status.success()
            && stderr.contains("PACKET28D_MAX_CONNECTIONS must be greater than zero"),
        "daemon did not reach the post-lease configuration sentinel: status={status}, stderr={stderr:?}"
    );
    assert!(
        !runtime_path(workspace.path()).exists() && !ready_path(workspace.path()).exists(),
        "failed post-lease configuration must not publish runtime or readiness"
    );
}
