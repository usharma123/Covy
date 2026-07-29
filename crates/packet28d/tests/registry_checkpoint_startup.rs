use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use packet28_daemon_core::storage::{ensure_daemon_dir, save_task_watch_registry_checkpoint};
use packet28_daemon_protocol::paths::{ready_path, task_registry_path, watch_registry_path};
use packet28_daemon_protocol::task::{TaskRegistry, WatchRegistry};

fn checkpoint_generation(path: &std::path::Path) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).expect("read registry"))
        .expect("decode registry")
        .get("task_watch_checkpoint_generation")
        .and_then(serde_json::Value::as_u64)
}

#[test]
fn daemon_rejects_mixed_registry_generations_before_readiness() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    save_task_watch_registry_checkpoint(
        workspace.path(),
        &TaskRegistry::default(),
        &WatchRegistry::default(),
    )
    .expect("persist initial paired checkpoint");
    let watch_path = watch_registry_path(workspace.path());
    let mut watch: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&watch_path).expect("read watch registry"))
            .expect("decode watch registry");
    let generation = watch
        .get("task_watch_checkpoint_generation")
        .and_then(serde_json::Value::as_u64)
        .expect("watch checkpoint generation");
    watch
        .as_object_mut()
        .expect("watch registry object")
        .insert(
            "task_watch_checkpoint_generation".to_string(),
            serde_json::Value::from(generation + 1),
        );
    std::fs::write(
        &watch_path,
        serde_json::to_vec_pretty(&watch).expect("encode mismatched watch registry"),
    )
    .expect("write mismatched watch registry");
    std::fs::write(ready_path(workspace.path()), b"stale-ready\n")
        .expect("seed stale readiness marker");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_packet28d"))
        .args(["serve", "--root"])
        .arg(workspace.path())
        .env("PACKET28D_FORCE_TCP", "1")
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
            "daemon did not reject mixed registry generations before timeout"
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
        "daemon unexpectedly accepted mixed state"
    );
    assert!(
        stderr.contains("task/watch registry checkpoint generations disagree"),
        "daemon did not report the registry generation mismatch: {stderr:?}"
    );
    assert!(
        !ready_path(workspace.path()).exists(),
        "daemon advertised readiness for mixed registry generations"
    );
}

#[test]
fn daemon_promotes_a_legacy_pair_before_readiness() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    ensure_daemon_dir(workspace.path()).expect("create daemon state");
    let task_path = task_registry_path(workspace.path());
    let watch_path = watch_registry_path(workspace.path());
    std::fs::write(
        &task_path,
        serde_json::to_vec_pretty(&TaskRegistry::default()).expect("encode legacy tasks"),
    )
    .expect("write legacy tasks");
    std::fs::write(
        &watch_path,
        serde_json::to_vec_pretty(&WatchRegistry::default()).expect("encode legacy watches"),
    )
    .expect("write legacy watches");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_packet28d"))
        .args(["serve", "--root"])
        .arg(workspace.path())
        .env("PACKET28D_FORCE_TCP", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn packet28d");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            daemon.try_wait().expect("probe daemon").is_none(),
            "daemon exited before promoting legacy registries"
        );
        if ready_path(workspace.path()).exists() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not become ready before timeout"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let task_generation = checkpoint_generation(&task_path).expect("task generation");
    let watch_generation = checkpoint_generation(&watch_path).expect("watch generation");
    assert!(task_generation > 0);
    assert_eq!(watch_generation, task_generation);

    daemon.kill().expect("stop daemon");
    daemon.wait().expect("join daemon");
}
