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
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready_path(workspace.path()).exists() && Instant::now() < deadline {
        assert!(
            daemon.0.try_wait().expect("probe daemon").is_none(),
            "daemon exited after the retention lease was released"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        ready_path(workspace.path()).exists(),
        "daemon did not publish readiness after the retention lease was released"
    );

    daemon.0.kill().expect("terminate daemon");
    let status = daemon.0.wait().expect("reap daemon");
    assert!(
        !status.success(),
        "forced daemon termination unexpectedly succeeded"
    );
}
