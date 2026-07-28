use std::io::Cursor;

use packet28_daemon_protocol::broker::BrokerGetContextRequest;
use packet28_daemon_protocol::commands::WatchSpec;
use packet28_daemon_protocol::context_store::ContextRecallRequest;
use packet28_daemon_protocol::frame::{read_frame, write_frame};
use packet28_daemon_protocol::hooks::HookIngestRequest;
use packet28_daemon_protocol::index::DaemonIndexStatusRequest;
use packet28_daemon_protocol::message::{DaemonRequest, DaemonResponse};
use packet28_daemon_protocol::paths::{daemon_dir, socket_path};
use packet28_daemon_protocol::task::TaskAwaitHandoffRequest;

#[test]
fn documented_modules_form_a_complete_client_surface() {
    let _watch = WatchSpec::default();
    let _recall = ContextRecallRequest::default();
    let _broker = BrokerGetContextRequest::default();
    let _hook = HookIngestRequest::default();
    let _index = DaemonIndexStatusRequest::default();
    let _await_handoff = TaskAwaitHandoffRequest::default();

    let root = std::path::Path::new("/workspace");
    assert!(daemon_dir(root).ends_with(".packet28/daemon"));
    assert_eq!(
        socket_path(root)
            .extension()
            .and_then(|value| value.to_str()),
        Some("sock")
    );

    let mut bytes = Vec::new();
    write_frame(&mut bytes, &DaemonRequest::Status).unwrap();
    let request: DaemonRequest = read_frame(&mut Cursor::new(bytes)).unwrap();
    assert!(matches!(request, DaemonRequest::Status));

    let response = DaemonResponse::Ack {
        message: "ready".to_string(),
    };
    assert_eq!(
        serde_json::to_value(response).unwrap()["type"],
        serde_json::json!("ack")
    );
}
