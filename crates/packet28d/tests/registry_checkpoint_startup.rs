use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::net::UnixStream;

use packet28_daemon_core::storage::{ensure_daemon_dir, save_task_watch_registry_checkpoint};
use packet28_daemon_protocol::frame::{read_frame, write_frame};
use packet28_daemon_protocol::message::{DaemonRequest, DaemonResponse, DaemonRuntimeInfo};
use packet28_daemon_protocol::paths::{
    ready_path, runtime_path, task_registry_path, watch_registry_path,
};
use packet28_daemon_protocol::task::{TaskRecord, TaskRegistry, WatchRegistry};

fn checkpoint_generation(path: &std::path::Path) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).expect("read registry"))
        .expect("decode registry")
        .get("task_watch_checkpoint_generation")
        .and_then(serde_json::Value::as_u64)
}

fn request_on_stream(
    stream: &mut (impl Read + Write),
    runtime: &DaemonRuntimeInfo,
    request: &DaemonRequest,
) -> DaemonResponse {
    if let Some(auth) = runtime.transport_auth.as_ref() {
        write_frame(&mut *stream, auth).expect("write daemon authentication");
        assert!(matches!(
            read_frame::<_, DaemonResponse>(&mut *stream)
                .expect("read authentication response"),
            DaemonResponse::Ack { ref message } if message == "authenticated"
        ));
    }
    write_frame(&mut *stream, request).expect("write daemon request");
    read_frame(&mut *stream).expect("read daemon response")
}

fn request(runtime: &DaemonRuntimeInfo, request: &DaemonRequest) -> DaemonResponse {
    if let Some(endpoint) = runtime.socket_path.strip_prefix("tcp://") {
        let mut stream = TcpStream::connect(endpoint).expect("connect to packet28d TCP endpoint");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("set daemon response timeout");
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .expect("set daemon request timeout");
        return request_on_stream(&mut stream, runtime, request);
    }
    #[cfg(unix)]
    {
        let mut stream =
            UnixStream::connect(&runtime.socket_path).expect("connect to packet28d Unix endpoint");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("set daemon response timeout");
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .expect("set daemon request timeout");
        request_on_stream(&mut stream, runtime, request)
    }
    #[cfg(not(unix))]
    {
        panic!(
            "unsupported non-TCP daemon endpoint '{}'",
            runtime.socket_path
        );
    }
}

fn wait_for_ready(daemon: &mut std::process::Child, root: &std::path::Path) -> DaemonRuntimeInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if ready_path(root).exists() {
            return serde_json::from_slice(
                &std::fs::read(runtime_path(root)).expect("read runtime metadata"),
            )
            .expect("decode runtime metadata");
        }
        assert!(
            daemon.try_wait().expect("probe daemon").is_none(),
            "daemon exited before readiness"
        );
        assert!(
            Instant::now() < deadline,
            "daemon did not become ready before timeout"
        );
        thread::sleep(Duration::from_millis(10));
    }
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

#[test]
fn daemon_restart_recovers_the_prior_pair_after_first_half_crash() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let task_id = "committed-n";
    save_task_watch_registry_checkpoint(
        workspace.path(),
        &TaskRegistry {
            tasks: BTreeMap::from([(
                task_id.to_string(),
                TaskRecord {
                    task_id: task_id.to_string(),
                    last_error: Some("committed-state-n".to_string()),
                    ..TaskRecord::default()
                },
            )]),
        },
        &WatchRegistry::default(),
    )
    .expect("persist committed state N");

    let mut fault_child = Command::new(env!("CARGO_BIN_EXE_packet28d"))
        .args(["serve", "--root"])
        .arg(workspace.path())
        .env("PACKET28_REGISTRY_CHECKPOINT_EXIT_AFTER", "watch")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn checkpoint fault child");
    let crash_deadline = Instant::now() + Duration::from_secs(10);
    let crash_status = loop {
        if let Some(status) = fault_child.try_wait().expect("probe fault child") {
            break status;
        }
        assert!(
            Instant::now() < crash_deadline,
            "fault child did not crash after the first checkpoint half"
        );
        thread::sleep(Duration::from_millis(10));
    };
    let mut fault_stderr = String::new();
    fault_child
        .stderr
        .take()
        .expect("fault child stderr")
        .read_to_string(&mut fault_stderr)
        .expect("read fault child stderr");
    assert_eq!(
        crash_status.code(),
        Some(86),
        "fault child did not exit at the injected checkpoint boundary: {fault_stderr}"
    );

    let mut restarted = Command::new(env!("CARGO_BIN_EXE_packet28d"))
        .args(["serve", "--root"])
        .arg(workspace.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("restart packet28d without a synthetic repair write");
    let runtime = wait_for_ready(&mut restarted, workspace.path());
    let task = match request(
        &runtime,
        &DaemonRequest::TaskStatus {
            task_id: task_id.to_string(),
        },
    ) {
        DaemonResponse::TaskStatus { task: Some(task) } => task,
        other => panic!("unexpected recovered task response: {other:?}"),
    };
    assert_eq!(task.last_error.as_deref(), Some("committed-state-n"));
    assert!(matches!(
        request(&runtime, &DaemonRequest::Stop),
        DaemonResponse::Ack { ref message } if message == "stopping"
    ));
    assert!(
        restarted.wait().expect("join restarted daemon").success(),
        "restarted daemon did not shut down cleanly"
    );
}
