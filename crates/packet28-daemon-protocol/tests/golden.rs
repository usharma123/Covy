use packet28_daemon_protocol::broker::{BrokerAction, BrokerGetContextRequest};
use packet28_daemon_protocol::hooks::{HookBoundaryKind, HookEventKind, HookIngestRequest};
use packet28_daemon_protocol::message::{
    ContextBackendKind, ContextResolveRequest, ContextSourceKind, DaemonRequest,
    Packet28SearchRequest,
};
use suite_packet_core::search::SearchRequest;

fn golden(contents: &str) -> serde_json::Value {
    serde_json::from_str(contents).unwrap()
}

#[test]
fn instruction_adapter_request_matches_golden_contract() {
    let request = DaemonRequest::ContextResolve {
        request: ContextResolveRequest {
            workspace_root: "/repo".to_string(),
            source_kind: ContextSourceKind::InstructionFile,
            source_path: Some("AGENTS.md".to_string()),
            source_sha256: "abc123".to_string(),
            source_content: "# Instructions".to_string(),
            render_mode: Some(packet28_daemon_protocol::message::InstructionRenderMode::Stable),
            stable_config: Some(
                packet28_daemon_protocol::message::InstructionStableConfig::default(),
            ),
            task_id: Some("task-1".to_string()),
            task_label: Some("remediation".to_string()),
            budget_tokens: Some(4096),
            schema_version: 1,
            agent_family: Some("codex".to_string()),
            backend_kind: ContextBackendKind::LinuxPreload,
        },
    };

    assert_eq!(
        serde_json::to_value(request).unwrap(),
        golden(include_str!("golden/context_resolve_request.json"))
    );
}

#[test]
fn search_adapter_request_matches_golden_contract() {
    let request = DaemonRequest::Packet28Search {
        request: Packet28SearchRequest {
            request: SearchRequest {
                query: "DaemonRequest".to_string(),
                requested_paths: vec!["crates".to_string()],
                fixed_string: true,
                case_sensitive: Some(false),
                whole_word: true,
                context_lines: Some(2),
                max_matches_per_file: Some(5),
                max_total_matches: Some(25),
            },
            force_indexed: true,
        },
    };

    assert_eq!(
        serde_json::to_value(request).unwrap(),
        golden(include_str!("golden/packet28_search_request.json"))
    );
}

#[test]
fn broker_request_matches_golden_contract() {
    let request = DaemonRequest::BrokerGetContext {
        request: BrokerGetContextRequest {
            task_id: "task-1".to_string(),
            action: Some(BrokerAction::Inspect),
            budget_tokens: Some(2048),
            focus_paths: vec!["crates/packet28d".to_string()],
            include_self_context: true,
            ..BrokerGetContextRequest::default()
        },
    };

    assert_eq!(
        serde_json::to_value(request).unwrap(),
        golden(include_str!("golden/broker_get_context_request.json"))
    );
}

#[test]
fn hook_request_matches_golden_contract() {
    let request = DaemonRequest::HookIngest {
        request: HookIngestRequest {
            task_id: "task-1".to_string(),
            session_id: Some("session-1".to_string()),
            event_kind: HookEventKind::PreCompact,
            boundary_kind: HookBoundaryKind::PreCompact,
            host_context_budget_tokens: Some(200_000),
            ..HookIngestRequest::default()
        },
    };

    assert_eq!(
        serde_json::to_value(request).unwrap(),
        golden(include_str!("golden/hook_ingest_request.json"))
    );
}
