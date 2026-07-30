#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use context_kernel_core::{KernelRequest, KernelSequenceRequest, KernelStepRequest};
use fs2::FileExt;
use packet28_daemon_core::storage::{load_task_registry, save_task_registry};
use packet28_daemon_core::task_store_lease::{
    acquire_daemon_instance_lease, try_acquire_task_store_retention_lease,
};
use packet28_daemon_protocol::broker::{
    BrokerGetContextResponse, BrokerHandoffDescriptor, BrokerHandoffStatus, BrokerResponseMode,
};
use packet28_daemon_protocol::commands::TaskSubmitSpec;
use packet28_daemon_protocol::frame::{read_frame, write_frame};
use packet28_daemon_protocol::message::{DaemonRequest, DaemonResponse, DaemonRuntimeInfo};
use packet28_daemon_protocol::paths::{
    ready_path, runtime_path, task_artifact_dir, task_event_log_path, task_version_json_path,
    task_versions_dir, ContextVersionStorageId, TaskStorageId,
};
use packet28_daemon_protocol::task::{TaskLaunchAgentRequest, TaskRecord, TaskRegistry};
use serde_json::json;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_daemon(root: &Path) -> ChildGuard {
    spawn_daemon_with_shutdown_grace(root, None)
}

fn spawn_daemon_with_shutdown_grace(root: &Path, grace_ms: Option<u64>) -> ChildGuard {
    let mut command = Command::new(env!("CARGO_BIN_EXE_packet28d"));
    command
        .args(["serve", "--root"])
        .arg(root)
        .env("PACKET28D_FORCE_TCP", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(grace_ms) = grace_ms {
        command.env("PACKET28D_SHUTDOWN_GRACE_MS", grace_ms.to_string());
    }
    ChildGuard(command.spawn().expect("spawn packet28d"))
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
    try_request(runtime, request).expect("complete daemon request")
}

fn try_request(
    runtime: &DaemonRuntimeInfo,
    request: &DaemonRequest,
) -> Result<DaemonResponse, String> {
    let endpoint = runtime
        .socket_path
        .strip_prefix("tcp://")
        .ok_or_else(|| "expected forced TCP endpoint".to_string())?;
    let mut stream =
        TcpStream::connect(endpoint).map_err(|error| format!("connect to packet28d: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| format!("set daemon response read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| format!("set daemon request write timeout: {error}"))?;
    write_frame(&mut stream, request).map_err(|error| format!("write daemon request: {error}"))?;
    read_frame(&mut stream).map_err(|error| format!("read daemon response: {error}"))
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

fn wait_for_exit(daemon: &mut ChildGuard, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = daemon.0.try_wait().expect("probe daemon exit") {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not exit within its bounded shutdown window"
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
    let probe = Command::new("kill")
        .args(["-0", "--", &format!("-{process_group}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("probe delegated process group with the system kill utility");
    assert!(
        !probe.success(),
        "delegated process group remained alive after daemon Stop"
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

#[test]
fn held_secondary_owner_lock_cannot_block_bounded_daemon_shutdown() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let secondary = tempfile::tempdir().expect("secondary persistent root");
    fs::create_dir_all(secondary.path().join(".packet28"))
        .expect("create secondary persistence directory");
    let owner_lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(secondary.path().join(".packet28/packet-cache-v3.lock"))
        .expect("open secondary persistence owner lock");
    FileExt::lock_exclusive(&owner_lock).expect("hold secondary persistence owner lock");

    let mut daemon = spawn_daemon_with_shutdown_grace(workspace.path(), Some(250));
    let runtime = wait_for_ready(&mut daemon, workspace.path());
    let blocked_runtime = runtime.clone();
    let secondary_root = secondary.path().to_string_lossy().to_string();
    let (finished_tx, finished_rx) = mpsc::channel();
    let blocked_request = thread::spawn(move || {
        let response = try_request(
            &blocked_runtime,
            &DaemonRequest::Execute {
                request: KernelRequest {
                    target: "agenty.state.snapshot".to_string(),
                    reducer_input: json!({"task_id": "held-secondary-owner"}),
                    policy_context: json!({"persist_root": secondary_root}),
                    ..KernelRequest::default()
                },
            },
        );
        finished_tx.send(response).ok();
    });

    thread::sleep(Duration::from_millis(100));
    assert!(
        matches!(finished_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "cross-root request unexpectedly bypassed the held persistence owner lock"
    );
    assert!(matches!(
        request(&runtime, &DaemonRequest::Status),
        DaemonResponse::Status { .. }
    ));

    let stop_started = Instant::now();
    assert!(matches!(
        request(&runtime, &DaemonRequest::Stop),
        DaemonResponse::Ack { ref message } if message == "stopping"
    ));
    let status = wait_for_exit(&mut daemon, Duration::from_secs(3));
    assert!(
        stop_started.elapsed() < Duration::from_secs(2),
        "held persistence owner lock defeated the configured shutdown deadline"
    );
    assert!(
        !status.success(),
        "daemon silently reported a clean shutdown with admitted blocking work still active"
    );

    FileExt::unlock(&owner_lock).expect("release secondary persistence owner lock");
    blocked_request
        .join()
        .expect("join blocked cross-root request");
    assert!(
        finished_rx
            .recv()
            .expect("receive blocked cross-root request result")
            .is_err(),
        "blocked request unexpectedly completed after the daemon exited"
    );
}

#[test]
fn failed_initial_submission_can_retry_the_same_task_id() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let task_id = "retry-failed-initial";
    let storage_id = TaskStorageId::try_from(task_id).expect("valid task storage id");
    let mut daemon = spawn_daemon(workspace.path());
    let runtime = wait_for_ready(&mut daemon, workspace.path());

    let failed = request(
        &runtime,
        &DaemonRequest::ExecuteSequence {
            spec: TaskSubmitSpec {
                task_id: task_id.to_string(),
                sequence: KernelSequenceRequest {
                    steps: vec![KernelStepRequest {
                        id: "invalid".to_string(),
                        target: "missing.reducer".to_string(),
                        ..KernelStepRequest::default()
                    }],
                    ..KernelSequenceRequest::default()
                },
                ..TaskSubmitSpec::default()
            },
        },
    );
    assert!(
        matches!(&failed, DaemonResponse::Error { .. }),
        "invalid first submission unexpectedly succeeded: {failed:?}"
    );
    assert!(
        !load_task_registry(workspace.path())
            .expect("load registry after failed first submission")
            .tasks
            .contains_key(task_id),
        "failed first submission remained durably admitted"
    );
    assert!(
        !task_artifact_dir(workspace.path(), &storage_id).exists(),
        "failed first submission left an artifact namespace"
    );
    assert!(
        !task_event_log_path(workspace.path(), &storage_id).exists(),
        "failed first submission left an event namespace"
    );

    let retried = request(
        &runtime,
        &DaemonRequest::ExecuteSequence {
            spec: TaskSubmitSpec {
                task_id: task_id.to_string(),
                sequence: KernelSequenceRequest {
                    steps: vec![KernelStepRequest {
                        id: "valid".to_string(),
                        target: "contextq.correlate".to_string(),
                        ..KernelStepRequest::default()
                    }],
                    ..KernelSequenceRequest::default()
                },
                ..TaskSubmitSpec::default()
            },
        },
    );
    assert!(
        matches!(&retried, DaemonResponse::ExecuteSequence { .. }),
        "valid same-ID retry did not succeed: {retried:?}"
    );

    assert!(matches!(
        request(&runtime, &DaemonRequest::Stop),
        DaemonResponse::Ack { ref message } if message == "stopping"
    ));
    let status = daemon.0.wait().expect("join daemon after retry");
    assert!(status.success(), "daemon Stop completed with {status}");
}
