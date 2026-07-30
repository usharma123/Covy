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
use packet28_daemon_core::storage::{
    load_task_events, load_task_registry, load_task_watch_registry_with_deltas, save_task_registry,
};
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
            let runtime: DaemonRuntimeInfo =
                serde_json::from_slice(&fs::read(runtime_path(root)).expect("read daemon runtime"))
                    .expect("decode daemon runtime");
            if runtime.pid == daemon.0.id() {
                return runtime;
            }
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
    let auth = runtime
        .transport_auth
        .as_ref()
        .ok_or_else(|| "forced TCP runtime has no authentication capability".to_string())?;
    write_frame(&mut stream, auth)
        .map_err(|error| format!("write daemon authentication prelude: {error}"))?;
    let auth_response: DaemonResponse = read_frame(&mut stream)
        .map_err(|error| format!("read daemon authentication response: {error}"))?;
    if !matches!(
        auth_response,
        DaemonResponse::Ack { ref message } if message == "authenticated"
    ) {
        return Err(format!(
            "unexpected daemon authentication response: {auth_response:?}"
        ));
    }
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

fn wait_for_launch_gate(root: &Path, task_id: &str) {
    let storage_id = TaskStorageId::try_from(task_id).expect("valid task storage id");
    let agent_dir = task_artifact_dir(root, &storage_id).join("agent");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let gate_is_ready = fs::read_dir(&agent_dir).is_ok_and(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with("launch-") && name.ends_with(".log"))
                })
                .any(|entry| {
                    fs::read_to_string(entry.path())
                        .is_ok_and(|log| log.contains("packet28 delegated launch gate ready"))
                })
        });
        if gate_is_ready {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "delegated launch gate did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_agent_launch_events(root: &Path, task_id: &str) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let kinds = load_task_events(root, task_id)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|frame| {
                frame
                    .event
                    .kind
                    .starts_with("task.agent_launch_")
                    .then_some(frame.event.kind)
            })
            .collect::<Vec<_>>();
        if kinds
            .iter()
            .any(|kind| kind == "task.agent_launch_completed")
        {
            return kinds;
        }
        assert!(
            Instant::now() < deadline,
            "delegated launch did not publish its completion event"
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
fn active_delegated_launch_rejects_overlap_for_the_same_task() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let task_id = "reject-overlapping-agent";
    seed_ready_handoff(workspace.path(), task_id);
    let mut daemon = spawn_daemon(workspace.path());
    let runtime = wait_for_ready(&mut daemon, workspace.path());
    let child_ready = workspace.path().join("first-agent-ready");

    let first = request(
        &runtime,
        &DaemonRequest::TaskLaunchAgent {
            request: TaskLaunchAgentRequest {
                task_id: task_id.to_string(),
                command: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "printf ready > \"$1\"; sleep 30".to_string(),
                    "packet28-first-agent".to_string(),
                    child_ready.to_string_lossy().to_string(),
                ],
                ..TaskLaunchAgentRequest::default()
            },
        },
    );
    assert!(
        matches!(first, DaemonResponse::TaskLaunchAgent { .. }),
        "first delegated launch failed: {first:?}"
    );
    wait_for_path(&child_ready);

    let overlap = request(
        &runtime,
        &DaemonRequest::TaskLaunchAgent {
            request: TaskLaunchAgentRequest {
                task_id: task_id.to_string(),
                command: vec!["sh".to_string(), "-c".to_string(), "exit 0".to_string()],
                ..TaskLaunchAgentRequest::default()
            },
        },
    );
    assert!(
        matches!(
            overlap,
            DaemonResponse::Error { ref message }
                if message.contains("already has an active delegated agent launch")
        ),
        "overlapping delegated launch was not rejected: {overlap:?}"
    );

    assert!(matches!(
        request(&runtime, &DaemonRequest::Stop),
        DaemonResponse::Ack { ref message } if message == "stopping"
    ));
    let status = daemon
        .0
        .wait()
        .expect("join daemon after overlap rejection");
    assert!(status.success(), "daemon Stop completed with {status}");
}

#[test]
fn immediate_delegated_exit_records_started_before_completed() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let task_id = "immediate-agent-exit";
    seed_ready_handoff(workspace.path(), task_id);
    let mut daemon = spawn_daemon(workspace.path());
    let runtime = wait_for_ready(&mut daemon, workspace.path());

    let response = request(
        &runtime,
        &DaemonRequest::TaskLaunchAgent {
            request: TaskLaunchAgentRequest {
                task_id: task_id.to_string(),
                command: vec!["sh".to_string(), "-c".to_string(), "exit 0".to_string()],
                ..TaskLaunchAgentRequest::default()
            },
        },
    );
    assert!(
        matches!(response, DaemonResponse::TaskLaunchAgent { .. }),
        "immediate delegated launch failed: {response:?}"
    );

    assert_eq!(
        wait_for_agent_launch_events(workspace.path(), task_id),
        vec![
            "task.agent_launch_started".to_string(),
            "task.agent_launch_completed".to_string(),
        ]
    );

    assert!(matches!(
        request(&runtime, &DaemonRequest::Stop),
        DaemonResponse::Ack { ref message } if message == "stopping"
    ));
    let status = daemon
        .0
        .wait()
        .expect("join daemon after immediate delegated exit");
    assert!(status.success(), "daemon Stop completed with {status}");
}

#[test]
fn crash_between_spawn_and_ownership_checkpoint_never_releases_delegated_work() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let task_id = "spawn-checkpoint-crash";
    seed_ready_handoff(workspace.path(), task_id);
    let mut daemon = spawn_daemon(workspace.path());
    let runtime = wait_for_ready(&mut daemon, workspace.path());
    let registry_lock_path = workspace
        .path()
        .join(".packet28/daemon/.task-registry-v1.json.lock");
    let registry_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&registry_lock_path)
        .expect("open task registry checkpoint lock");
    FileExt::lock_exclusive(&registry_lock).expect("fault-inject blocked ownership checkpoint");

    let delegated_work = workspace.path().join("delegated-work-ran");
    let launch_runtime = runtime;
    let delegated_work_arg = delegated_work.to_string_lossy().to_string();
    let (finished_tx, finished_rx) = mpsc::channel();
    let launch = thread::spawn(move || {
        let response = try_request(
            &launch_runtime,
            &DaemonRequest::TaskLaunchAgent {
                request: TaskLaunchAgentRequest {
                    task_id: task_id.to_string(),
                    command: vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        "printf ran > \"$1\"".to_string(),
                        "packet28-crash-window-agent".to_string(),
                        delegated_work_arg,
                    ],
                    ..TaskLaunchAgentRequest::default()
                },
            },
        );
        finished_tx.send(()).ok();
        response
    });

    wait_for_launch_gate(workspace.path(), task_id);
    thread::sleep(Duration::from_millis(100));
    assert!(
        matches!(finished_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "launch bypassed the blocked ownership checkpoint"
    );
    assert!(
        !delegated_work.exists(),
        "delegated command ran before ownership became durable"
    );

    daemon.0.kill().expect("crash daemon in launch window");
    let crash_status = daemon.0.wait().expect("reap crashed daemon");
    assert!(
        !crash_status.success(),
        "fault injection did not crash daemon"
    );
    FileExt::unlock(&registry_lock).expect("release task registry checkpoint lock");
    assert!(
        launch
            .join()
            .expect("join interrupted launch request")
            .is_err(),
        "launch request unexpectedly survived daemon crash"
    );
    thread::sleep(Duration::from_millis(250));
    assert!(
        !delegated_work.exists(),
        "delegated command escaped its closed crash-window gate"
    );
    assert_eq!(
        load_task_registry(workspace.path())
            .expect("load pre-crash task registry")
            .tasks[task_id]
            .latest_agent_pid,
        None
    );

    let mut restarted = spawn_daemon(workspace.path());
    let restarted_runtime = wait_for_ready(&mut restarted, workspace.path());
    assert!(matches!(
        request(&restarted_runtime, &DaemonRequest::Stop),
        DaemonResponse::Ack { ref message } if message == "stopping"
    ));
    let restart_status = restarted.0.wait().expect("join restarted daemon");
    assert!(
        restart_status.success(),
        "daemon restart failed after gated launch crash: {restart_status}"
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
        !load_task_watch_registry_with_deltas(workspace.path())
            .expect("load registry after failed first submission")
            .tasks
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
