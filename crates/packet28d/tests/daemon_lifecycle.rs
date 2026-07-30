use std::collections::BTreeMap;
use std::fs;
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use packet28_daemon_core::storage::save_task_registry;
use packet28_daemon_core::task_store_lease::{
    acquire_daemon_instance_lease, try_acquire_task_store_retention_lease,
};
use packet28_daemon_protocol::broker::{
    BrokerGetContextResponse, BrokerHandoffDescriptor, BrokerHandoffStatus, BrokerResponseMode,
};
use packet28_daemon_protocol::frame::{read_frame, write_frame};
use packet28_daemon_protocol::message::{DaemonRequest, DaemonResponse, DaemonRuntimeInfo};
use packet28_daemon_protocol::paths::{
    ready_path, runtime_path, task_version_json_path, task_versions_dir, ContextVersionStorageId,
    TaskStorageId,
};
use packet28_daemon_protocol::task::{TaskLaunchAgentRequest, TaskRecord, TaskRegistry};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_daemon(root: &Path) -> ChildGuard {
    ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_packet28d"))
            .args(["serve", "--root"])
            .arg(root)
            .env("PACKET28D_FORCE_TCP", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn packet28d"),
    )
}

fn wait_for_ready(daemon: &mut ChildGuard, root: &Path) -> DaemonRuntimeInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if ready_path(root).exists() {
            return serde_json::from_slice(
                &fs::read(runtime_path(root)).expect("read daemon runtime"),
            )
            .expect("decode daemon runtime");
        }
        assert!(
            daemon.0.try_wait().expect("probe daemon").is_none(),
            "daemon exited before readiness"
        );
        assert!(
            Instant::now() < deadline,
            "daemon did not publish readiness before timeout"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn request(runtime: &DaemonRuntimeInfo, request: &DaemonRequest) -> DaemonResponse {
    let endpoint = runtime
        .socket_path
        .strip_prefix("tcp://")
        .expect("forced TCP endpoint");
    let mut stream = TcpStream::connect(endpoint).expect("connect to packet28d");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set daemon response read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .expect("set daemon request write timeout");
    write_frame(&mut stream, request).expect("write daemon request");
    read_frame(&mut stream).expect("read daemon response")
}

fn seed_ready_handoff(root: &Path, task_id: &str) {
    let context_version = "ctx-1";
    let artifact_id = "artifact-1";
    let handoff_id = "handoff-1";
    let storage_id = TaskStorageId::try_from(task_id).expect("valid task storage id");
    let context_storage_id =
        ContextVersionStorageId::try_from(context_version).expect("valid context storage id");
    let context = BrokerGetContextResponse {
        context_version: context_version.to_string(),
        response_mode: BrokerResponseMode::Full,
        artifact_id: Some(artifact_id.to_string()),
        brief: "resume delegated lifecycle test".to_string(),
        ..BrokerGetContextResponse::default()
    };

    let handoff = BrokerHandoffDescriptor {
        handoff_id: handoff_id.to_string(),
        task_id: task_id.to_string(),
        artifact_id: artifact_id.to_string(),
        context_version: context_version.to_string(),
        status: BrokerHandoffStatus::Ready,
        generated_at_unix_ms: 1,
        ..BrokerHandoffDescriptor::default()
    };
    save_task_registry(
        root,
        &TaskRegistry {
            tasks: BTreeMap::from([(
                task_id.to_string(),
                TaskRecord {
                    task_id: task_id.to_string(),
                    latest_context_version: Some(context_version.to_string()),
                    latest_handoff_id: Some(handoff_id.to_string()),
                    latest_handoff_artifact_id: Some(artifact_id.to_string()),
                    latest_handoff_generated_at_unix: Some(1),
                    handoffs: vec![handoff],
                    ..TaskRecord::default()
                },
            )]),
        },
    )
    .expect("seed ready handoff task");
    fs::create_dir_all(task_versions_dir(root, &storage_id))
        .expect("create task versions directory");
    fs::write(
        task_version_json_path(root, &storage_id, &context_storage_id),
        serde_json::to_vec_pretty(&context).expect("encode handoff context"),
    )
    .expect("write handoff context");
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "delegated child did not publish readiness"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn stop_terminates_term_resistant_process_group_before_releasing_leases() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let task_id = "stop-process-group";
    seed_ready_handoff(workspace.path(), task_id);
    let mut daemon = spawn_daemon(workspace.path());
    let runtime = wait_for_ready(&mut daemon, workspace.path());
    let child_ready = workspace.path().join("delegated-ready");

    let response = request(
        &runtime,
        &DaemonRequest::TaskLaunchAgent {
            request: TaskLaunchAgentRequest {
                task_id: task_id.to_string(),
                command: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "trap '' TERM; printf ready > \"$1\"; while :; do sleep 1; done".to_string(),
                    "packet28-stop-child".to_string(),
                    child_ready.to_string_lossy().to_string(),
                ],
                ..TaskLaunchAgentRequest::default()
            },
        },
    );
    let pid = match response {
        DaemonResponse::TaskLaunchAgent { response } => response.pid,
        other => panic!("unexpected launch response: {other:?}"),
    };
    wait_for_path(&child_ready);

    let stop_started = Instant::now();
    assert!(matches!(
        request(&runtime, &DaemonRequest::Stop),
        DaemonResponse::Ack { ref message } if message == "stopping"
    ));
    let status = daemon.0.wait().expect("join daemon after Stop");
    assert!(status.success(), "daemon Stop completed with {status}");
    assert!(
        stop_started.elapsed() < Duration::from_secs(10),
        "daemon Stop exceeded the bounded process reap window"
    );

    let process_group = i32::try_from(pid).expect("child pid fits process-group id");
    // SAFETY: signal 0 only probes the process group created by this test.
    let probe_result = unsafe { libc::kill(-process_group, 0) };
    assert_eq!(probe_result, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert!(
        acquire_daemon_instance_lease(workspace.path()).is_ok(),
        "daemon instance lease was released before child reaping completed"
    );
    assert!(
        try_acquire_task_store_retention_lease(workspace.path())
            .expect("try task-store retention lease")
            .is_some(),
        "task-store retention lease was released before child reaping completed"
    );
}
