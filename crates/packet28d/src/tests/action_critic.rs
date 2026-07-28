use super::*;

#[test]
fn choose_tool_action_critic_flags_missing_intent_and_risky_commands() {
    let missing = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(missing
        .iter()
        .any(|line| line.contains("missing_tool_intent")));

    let risky = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            query: Some("run rm -rf target/tmp after checking".to_string()),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(risky
        .iter()
        .any(|line| line.contains("destructive_command")));

    let scoped_search = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            query: Some("rg AlphaService".to_string()),
            focus_paths: vec!["src/alpha.rs".to_string()],
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(!scoped_search
        .iter()
        .any(|line| line.contains("broad_search")));

    let broad_search = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            query: Some("rg AlphaService".to_string()),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(broad_search
        .iter()
        .any(|line| line.contains("broad_search")));
}

#[test]
fn choose_tool_action_critic_flags_finalization_without_recent_verification() {
    let missing_verification = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            query: Some("commit and push this change".to_string()),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(missing_verification
        .iter()
        .any(|line| line.contains("verification_gap")));

    let verified = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            query: Some("commit and push this change".to_string()),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload {
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-1".to_string(),
                sequence: 1,
                tool_name: "cargo test".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        },
        &[],
    );
    assert!(!verified
        .iter()
        .any(|line| line.contains("verification_gap")));
}

#[test]
fn edit_action_critic_flags_missing_scope_and_unread_paths() {
    let missing_scope = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::Edit),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(missing_scope
        .iter()
        .any(|line| line.contains("missing_edit_scope")));

    let unread = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::Edit),
            focus_paths: vec![
                "src/read.rs".to_string(),
                "src/unread.rs".to_string(),
                "./src/tool-read.rs".to_string(),
            ],
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload {
            files_read: vec!["src/read.rs".to_string()],
            read_paths_by_tool: vec![suite_packet_core::ToolPathSummary {
                tool_name: "rg".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Read,
                paths: vec!["src/tool-read.rs".to_string()],
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
        &[],
    );
    assert!(unread
        .iter()
        .any(|line| line.contains("read_before_edit") && line.contains("src/unread.rs")));
    assert!(!unread
        .iter()
        .any(|line| line.contains("src/read.rs") || line.contains("src/tool-read.rs")));
}
