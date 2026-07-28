use std::io::Cursor;

use packet28_daemon_core::{
    daemon_dir, read_socket_message, socket_path, write_socket_message, BrokerGetContextRequest,
    ContextRecallRequest, DaemonIndexStatusRequest, DaemonRequest, DaemonResponse,
    HookIngestRequest, TaskAwaitHandoffRequest, TaskRegistry, WatchSpec,
};

#[test]
fn legacy_root_paths_remain_source_and_wire_compatible() {
    let _watch = WatchSpec::default();
    let _recall = ContextRecallRequest::default();
    let _broker = BrokerGetContextRequest::default();
    let _hook = HookIngestRequest::default();
    let _index = DaemonIndexStatusRequest::default();
    let _await_handoff = TaskAwaitHandoffRequest::default();
    let _registry = TaskRegistry::default();

    let root = std::path::Path::new("/workspace");
    assert!(daemon_dir(root).ends_with(".packet28/daemon"));
    assert_eq!(
        socket_path(root)
            .extension()
            .and_then(|value| value.to_str()),
        Some("sock")
    );

    let mut bytes = Vec::new();
    write_socket_message(&mut bytes, &DaemonRequest::Status).unwrap();
    let request: DaemonRequest = read_socket_message(&mut Cursor::new(bytes)).unwrap();
    assert!(matches!(request, DaemonRequest::Status));

    let response = DaemonResponse::Ack {
        message: "ready".to_string(),
    };
    assert_eq!(
        serde_json::to_value(response).unwrap()["type"],
        serde_json::json!("ack")
    );
}
