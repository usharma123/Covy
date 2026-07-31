use std::fs;
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use context_kernel_core::{KernelPacket, KernelRequest};
use packet28_daemon_protocol::context_store::{
    ContextRecallRequest, ContextStoreListRequest, ContextStorePruneDaemonRequest,
    ContextStoreStatsRequest,
};
use packet28_daemon_protocol::frame::{read_frame, write_frame};
use packet28_daemon_protocol::message::{DaemonRequest, DaemonResponse, DaemonRuntimeInfo};
use packet28_daemon_protocol::paths::{ready_path, runtime_path};
use serde_json::json;

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
            .stderr(Stdio::null())
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
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set daemon response read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set daemon request write timeout");
    let auth = runtime
        .transport_auth
        .as_ref()
        .expect("forced TCP runtime authentication capability");
    write_frame(&mut stream, auth).expect("write daemon authentication prelude");
    let auth_response: DaemonResponse =
        read_frame(&mut stream).expect("read daemon authentication response");
    assert!(matches!(
        auth_response,
        DaemonResponse::Ack { ref message } if message == "authenticated"
    ));
    write_frame(&mut stream, request).expect("write daemon request");
    read_frame(&mut stream).expect("read daemon response")
}

fn stop_and_wait(daemon: &mut ChildGuard, runtime: &DaemonRuntimeInfo) {
    let response = request(runtime, &DaemonRequest::Stop);
    assert!(matches!(
        response,
        DaemonResponse::Ack { ref message } if message == "stopping"
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = daemon.0.try_wait().expect("probe daemon exit") {
            assert!(status.success(), "daemon Stop completed with {status}");
            return;
        }
        assert!(Instant::now() < deadline, "daemon did not exit after Stop");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn daemon_context_apis_share_immediate_state_and_persist_prune_across_restart() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let root = workspace.path().to_string_lossy().to_string();
    let mut daemon = spawn_daemon(workspace.path());
    let runtime = wait_for_ready(&mut daemon, workspace.path());

    let execute = request(
        &runtime,
        &DaemonRequest::Execute {
            request: KernelRequest {
                target: "contextq.assemble".to_string(),
                input_packets: vec![KernelPacket::from_value(
                    json!({
                        "packet_id": "process-live-context",
                        "tool": "packet28d",
                        "reducer": "context",
                        "sections": [{
                            "title": "Process context",
                            "body": "cross process immediate visibility marker",
                            "refs": [],
                            "relevance": 1.0
                        }]
                    }),
                    None,
                )],
                ..KernelRequest::default()
            },
        },
    );
    assert!(matches!(execute, DaemonResponse::Execute { .. }));

    let listed = request(
        &runtime,
        &DaemonRequest::ContextStoreList {
            request: ContextStoreListRequest {
                root: root.clone(),
                limit: 20,
                ..ContextStoreListRequest::default()
            },
        },
    );
    assert!(matches!(
        listed,
        DaemonResponse::ContextStoreList { ref response } if response.entries.len() == 1
    ));

    let recalled = request(
        &runtime,
        &DaemonRequest::ContextRecall {
            request: ContextRecallRequest {
                query: "immediate visibility".to_string(),
                root: root.clone(),
                limit: 10,
                since: Some(0),
                ..ContextRecallRequest::default()
            },
        },
    );
    assert!(matches!(
        recalled,
        DaemonResponse::ContextRecall { ref response } if response.hits.len() == 1
    ));

    let pruned = request(
        &runtime,
        &DaemonRequest::ContextStorePrune {
            request: ContextStorePruneDaemonRequest {
                root: root.clone(),
                all: true,
                ttl_secs: None,
            },
        },
    );
    assert!(matches!(
        pruned,
        DaemonResponse::ContextStorePrune { ref response }
            if response.report.removed == 1 && response.report.remaining == 0
    ));
    stop_and_wait(&mut daemon, &runtime);

    let mut restarted = spawn_daemon(workspace.path());
    let restarted_runtime = wait_for_ready(&mut restarted, workspace.path());
    let stats = request(
        &restarted_runtime,
        &DaemonRequest::ContextStoreStats {
            request: ContextStoreStatsRequest { root },
        },
    );
    assert!(matches!(
        stats,
        DaemonResponse::ContextStoreStats { ref response } if response.stats.entries == 0
    ));
    stop_and_wait(&mut restarted, &restarted_runtime);
}
