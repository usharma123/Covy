use super::*;

#[tokio::test]
async fn local_server_dispatches_mixed_json_rpc_batches() {
    let root = tempfile::tempdir().unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = dispatch_local_payload(
        root.path(),
        &session,
        json!([
            {"jsonrpc":"2.0","id":1,"method":"tools/list"},
            {"jsonrpc":"2.0","method":"notifications/initialized"},
            {"jsonrpc":"2.0","id":"missing","method":"unsupported/test"}
        ]),
    )
    .await
    .unwrap()
    .unwrap();

    let responses = response.as_array().unwrap();
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["id"], 1);
    assert!(responses[0]["result"]["tools"].is_array());
    assert_eq!(responses[1]["id"], "missing");
    assert_eq!(responses[1]["error"]["code"], -32601);
    assert!(session.lock().unwrap().initialized);
}

#[tokio::test]
async fn local_server_rejects_empty_and_invalid_batches() {
    let root = tempfile::tempdir().unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));

    for payload in [json!([]), json!([17])] {
        let response = dispatch_local_payload(root.path(), &session, payload)
            .await
            .unwrap()
            .unwrap();
        let error = response
            .as_array()
            .and_then(|responses| responses.first())
            .unwrap_or(&response);
        assert_eq!(error["id"], Value::Null);
        assert_eq!(error["error"]["code"], -32600);
    }

    assert!(dispatch_local_payload(
        root.path(),
        &session,
        json!([
            {"jsonrpc":"2.0","method":"notifications/initialized"}
        ]),
    )
    .await
    .unwrap()
    .is_none());
}

#[tokio::test]
async fn local_batch_limit_plus_one_is_bounded_and_next_request_remains_responsive() {
    let root = tempfile::tempdir().unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let oversized = Value::Array(vec![
        json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        MAX_MCP_BATCH_MESSAGES + 1
    ]);

    let rejection = dispatch_local_payload(root.path(), &session, oversized)
        .await
        .unwrap()
        .unwrap();
    let responses = rejection.as_array().unwrap();
    assert_eq!(
        (
            responses.len(),
            responses[0]["id"].clone(),
            responses[0]["error"]["code"].clone(),
        ),
        (1, Value::Null, json!(-32000))
    );

    let next = dispatch_local_payload(
        root.path(),
        &session,
        json!({"jsonrpc":"2.0","id":"next","method":"tools/list"}),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(next["id"], "next");
}

fn task_version_json_path(root: &Path, task_id: &str, context_version: &str) -> PathBuf {
    validated_task_version_json_path(root, task_id, context_version).unwrap()
}

#[test]
fn artifact_fetch_entry_points_reject_nonopaque_external_handles() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();

    for handle in [
        "../outside.json",
        outside.path().to_str().unwrap(),
        "Result.json",
        "con.json",
    ] {
        assert!(
            support::load_tool_result_artifact(root.path(), "task", Some(handle), None,).is_err()
        );
        assert!(support::load_raw_output_artifact(root.path(), "task", handle).is_err());
    }
    assert!(!root.path().join(".packet28").exists());
}

#[test]
fn artifact_store_entry_point_validates_before_task_store_mutation() {
    let root = tempfile::tempdir().unwrap();
    let overlong = "a".repeat(251);

    for invocation_id in ["../escape", "Invocation", "con", overlong.as_str()] {
        assert!(support::store_tool_artifact(
            root.path(),
            "task",
            invocation_id,
            "result",
            &json!({"ok": true}),
        )
        .is_err());
    }
    assert!(!root.path().join(".packet28").exists());
}

#[test]
fn artifact_store_and_fetch_entry_points_roundtrip_opaque_handles() {
    let root = tempfile::tempdir().unwrap();
    let artifact_id = support::store_tool_artifact(
        root.path(),
        "task",
        "invocation-1",
        "result",
        &json!({"task_id": "task", "value": 1}),
    )
    .unwrap();

    let (loaded_id, payload) =
        support::load_tool_result_artifact(root.path(), "task", Some(&artifact_id), None).unwrap();

    assert_eq!(loaded_id, artifact_id);
    assert_eq!(payload["value"], 1);
}

#[test]
fn context_artifact_identity_requires_exact_version_and_artifact_fields() {
    assert!(validate_context_artifact_identity(
        &json!({"context_version": "ctx-1", "artifact_id": "ctx-1"}),
        "ctx-1",
    )
    .is_ok());
    assert!(
        validate_context_artifact_identity(&json!({"context_version": "ctx-1"}), "ctx-1",).is_err()
    );
    assert!(validate_context_artifact_identity(
        &json!({"context_version": "ctx-1", "artifact_id": "ctx-2"}),
        "ctx-1",
    )
    .is_err());
}

#[test]
fn native_tool_lifecycle_matches_reviewed_structural_snapshot() {
    let mut actual = native_tools::structural_snapshot();
    let all_tools = tools_list_payload(McpToolset::All);
    actual["all_tools_list_names"] = Value::Array(
        all_tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool.get("name").cloned())
            .collect(),
    );
    let expected: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/mcp/native_tool_lifecycle.json"
    ))
    .unwrap();

    assert_eq!(actual, expected);
}

#[test]
fn tools_list_exposes_search_fast_without_task_id() {
    let root = tempfile::tempdir().unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let payload = handle_method(root.path(), &session, "tools/list", Value::Null).unwrap();
    let tools = payload["tools"].as_array().unwrap();
    let search_fast = tools
        .iter()
        .find(|tool| tool["name"] == "packet28_search_fast")
        .unwrap();
    let props = search_fast["inputSchema"]["properties"]
        .as_object()
        .unwrap();

    assert!(props.contains_key("query"));
    assert!(!props.contains_key("task_id"));
}

#[test]
fn tools_list_exposes_fff_search_strategy() {
    let root = tempfile::tempdir().unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let payload = handle_method(root.path(), &session, "tools/list", Value::Null).unwrap();
    let tools = payload["tools"].as_array().unwrap();

    for name in ["packet28_search", "packet28_search_fast"] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        let has_fff = tool["inputSchema"]["properties"]["search_strategy"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .any(|strategy| strategy == "fff");
        assert!(has_fff);
    }
}

#[test]
fn tools_list_defaults_to_core_catalog_to_reduce_first_load() {
    let root = tempfile::tempdir().unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let core_payload = handle_method(root.path(), &session, "tools/list", Value::Null).unwrap();
    let core_tools = core_payload["tools"].as_array().unwrap();
    let core_names = core_tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();

    assert!(core_names.contains(&"packet28_search"));
    assert!(core_names.contains(&"packet28_read_regions"));
    assert!(core_names.contains(&"packet28_fetch_tool_result"));
    assert!(core_names.contains(&"packet28_write_intention"));
    assert!(core_names.contains(&"packet28_prepare_handoff"));
    assert!(!core_names.contains(&"packet28_handoff"));
    assert!(!core_names.contains(&"packet28_memory_store"));
    assert!(!core_names.contains(&"packet28_action_critic"));
    assert!(!core_names.contains(&"packet28_recommend_next_tool"));
    assert!(!core_names.contains(&"packet28_validate_tool_outcome"));
    assert!(!core_names.contains(&"packet28_patch_risk"));
    assert!(core_tools.len() <= 12);

    let all_session = Arc::new(Mutex::new(McpSessionState {
        toolset: McpToolset::All,
        ..McpSessionState::default()
    }));
    let all_payload = handle_method(root.path(), &all_session, "tools/list", Value::Null).unwrap();
    let core_bytes = serde_json::to_vec(&core_payload).unwrap().len();
    let all_bytes = serde_json::to_vec(&all_payload).unwrap().len();
    assert!(core_bytes <= 8 * 1024, "core={core_bytes}");
    assert!(
        core_bytes * 4 < all_bytes,
        "core={core_bytes} all={all_bytes}"
    );
}

#[test]
fn tools_list_omits_handoff_alias_but_accepts_compatibility_names() {
    assert_eq!(
        canonical_tool_name("packet28_handoff"),
        "packet28.prepare_handoff"
    );
    assert_eq!(
        canonical_tool_name("packet28.handoff"),
        "packet28.prepare_handoff"
    );

    let root = tempfile::tempdir().unwrap();
    let session = Arc::new(Mutex::new(McpSessionState {
        toolset: McpToolset::All,
        ..McpSessionState::default()
    }));
    let payload = handle_method(root.path(), &session, "tools/list", Value::Null).unwrap();
    let tool_names = payload["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();

    assert!(tool_names.contains(&"packet28_prepare_handoff"));
    assert!(!tool_names.contains(&"packet28_handoff"));

    for required in [
        "packet28_reduce",
        "packet28_rewrite",
        "packet28_verify_handoff",
        "packet28_prompt_pressure",
        "packet28_handoff_diff",
        "packet28_handoff_compress",
        "packet28_handoff_lint_dependencies",
        "packet28_handoff_lint_paths",
        "packet28_handoff_lint_tests",
        "packet28_handoff_lint_stale_commands",
        "packet28_handoff_lint_environment",
        "packet28_handoff_lint_all",
        "packet28_handoff_fix_plan",
        "packet28_handoff_repair_verify",
        "packet28_handoff_lint_trends",
        "packet28_handoff_lint_regressions",
        "packet28_validate_plan",
        "packet28_action_critic",
        "packet28_recommend_next_tool",
        "packet28_validate_tool_outcome",
        "packet28_agent_status",
        "packet28_patch_risk",
        "packet28_verify_experiments",
        "packet28_reducer_drift",
        "packet28_hypothesis_add",
        "packet28_hypothesis_list",
        "packet28_hypothesis_resolve",
        "packet28_doctor",
        "packet28_memory_list",
        "packet28_memory_lint",
        "packet28_context_anomalies",
        "packet28_verify_context_anomalies",
        "packet28_memory_embed",
        "packet28_memory_extract_patterns",
        "packet28_feedback_search",
        "packet28_feedback_list",
        "packet28_feedback_apply",
        "packet28_feedback_delete",
        "packet28_feedback_stats",
        "packet28_wakeup",
        "packet28_learn_project",
        "packet28_transcript_append",
        "packet28_transcript_search",
        "packet28_transcript_stats",
        "packet28_graph_create",
        "packet28_graph_list",
        "packet28_graph_show",
        "packet28_graph_search",
        "packet28_graph_export",
        "packet28_graph_stats",
        "packet28_graph_inspect_concept",
        "packet28_graph_distill",
    ] {
        assert!(
            tool_names.contains(&required),
            "{required} missing from tools/list"
        );
    }
}

#[test]
fn agent_status_reports_safe_cache_policy() {
    let root = tempfile::tempdir().unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let payload = handle_method(
        root.path(),
        &session,
        "tools/call",
        json!({
            "name": "packet28.agent_status",
            "arguments": {}
        }),
    )
    .unwrap();
    let content = payload["structuredContent"].clone();

    assert_eq!(content["status"], "ok");
    assert_eq!(
        content["reducer_cache_safety"]["workspace_fingerprint_enabled"],
        true
    );
    assert_eq!(content["mcp"]["manual_json_rpc_required"], false);
}

#[test]
fn context_anomalies_tool_reports_dashboard_quality_signals() {
    let root = tempfile::tempdir().unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    crate::cmd_dashboard::record_memory_lint_history(
        root.path(),
        &json!({
            "ok": false,
            "memory_count": 1,
            "issue_count": 1,
            "lint": {
                "issues": [{
                    "kind": "runtime_specific_memory",
                    "detail": "mentions windsurf"
                }]
            }
        }),
    )
    .unwrap();

    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.context_anomalies",
            "arguments": {}
        }),
    )
    .unwrap();

    let content = &response["structuredContent"];
    assert_eq!(content["anomaly_count"], 1);
    assert_eq!(content["anomalies"][0]["category"], "memory_lint");
    assert!(content["anomalies"][0]["next_check"]
        .as_str()
        .unwrap()
        .contains("memory-lint"));
    assert!(content["anomalies"][0]["repair_hint"]
        .as_str()
        .unwrap()
        .contains("stale runtime"));
}

#[test]
fn verify_context_anomalies_tool_enforces_high_threshold() {
    let root = tempfile::tempdir().unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    crate::cmd_dashboard::record_memory_lint_history(
        root.path(),
        &json!({
            "ok": false,
            "memory_count": 1,
            "issue_count": 1,
            "lint": {
                "issues": [{
                    "kind": "runtime_specific_memory",
                    "detail": "mentions windsurf"
                }]
            }
        }),
    )
    .unwrap();

    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.verify_context_anomalies",
            "arguments": {
                "max_high": 0
            }
        }),
    )
    .unwrap();

    let content = &response["structuredContent"];
    assert_eq!(content["ok"], false);
    assert_eq!(content["high_count"], 1);
    assert!(content["anomalies"][0]["next_check"]
        .as_str()
        .unwrap()
        .contains("memory-lint"));
    assert!(content["anomalies"][0]["repair_hint"]
        .as_str()
        .unwrap()
        .contains("stale runtime"));
    assert!(serde_json::to_string(content).unwrap().len() < 1024);
}

#[test]
fn reduce_and_rewrite_tools_return_structured_results() {
    let root = tempfile::tempdir().unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let reduce = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.reduce",
            "arguments": {
                "command": "git status --short",
                "stdout": " M src/lib.rs\n",
                "exit_code": 0
            }
        }),
    )
    .unwrap();
    assert_eq!(
        reduce["structuredContent"]["reducer_family"],
        Value::String("git".to_string())
    );

    let rewrite = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.rewrite",
            "arguments": {
                "command": "git status --short"
            }
        }),
    )
    .unwrap();
    assert_eq!(
        rewrite["structuredContent"]["route"],
        Value::String("reducer_rewrite".to_string())
    );
}

#[test]
fn verify_experiments_tool_returns_manifest_status() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs/experiments")).unwrap();
    std::fs::write(
        root.path().join("docs/experiments/evidence.md"),
        "saved_tokens: 12\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("docs/experiments/manifest.json"),
        r#"{
          "experiments": [{
            "id": "mcp-verify",
            "workflow": "MCP experiment audit",
            "commands": ["Packet28 verify experiments --json"],
            "artifacts": ["docs/experiments/evidence.md"],
            "metrics": [{"name":"saved_tokens","value":12,"min":10,"evidence":["saved_tokens: 12"]}],
            "runtime_versions": [{"name":"packet28","version":"0.2.59"}]
          }]
        }"#,
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.verify_experiments",
            "arguments": {
                "require_workflows": ["MCP experiment audit"]
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["ok"], true);
    assert_eq!(response["structuredContent"]["experiment_count"], 1);
    assert_eq!(response["structuredContent"]["issue_count"], 0);
}

#[test]
fn reducer_drift_tool_flags_missing_marker() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs/reducer-drift")).unwrap();
    std::fs::write(
        root.path().join("docs/reducer-drift/fixtures.json"),
        r#"{
          "cases": [{
            "id": "mcp-missing-marker",
            "command_argv": ["cargo", "test", "removed_failure"],
            "stdout": "running 1 test\ntest removed_failure ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n",
            "stderr": "",
            "exit_code": 101,
            "required_markers": ["FAIL removed_failure"]
          }]
        }"#,
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.reducer_drift",
            "arguments": {}
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["ok"], false);
    assert_eq!(response["structuredContent"]["issue_count"], 1);
    assert_eq!(
        response["structuredContent"]["issues"][0]["kind"],
        Value::String("missing_marker".to_string())
    );
    assert!(response["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("issues=1"));
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 1024
    );
}

#[test]
fn verify_handoff_fails_when_next_action_is_missing() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-replay";
    let context_version = "ctx-missing-next";
    let path = task_version_json_path(root.path(), task_id, context_version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "context_version": context_version,
            "artifact_id": context_version,
            "brief": "## Task Objective\nFinish the replay verifier.",
            "sections": [{"id": "context_debt", "title": "Context Debt", "body": "- debt_summary: stale_paths=0 open_questions=0 unverified_edits=0 contradictions=0"}],
            "evidence_artifact_ids": ["artifact-1"],
            "next_action_summary": null,
            "latest_intention": null
        }))
        .unwrap(),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.verify_handoff",
            "arguments": {
                "task_id": task_id,
                "context_version": context_version
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["ready"], false);
    assert_eq!(response["structuredContent"]["score"], 75);
    assert!(response["structuredContent"]["missing"]
        .as_array()
        .unwrap()
        .iter()
        .any(|missing| missing == "next_action"));
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 512
    );
}

#[test]
fn prompt_pressure_identifies_largest_removable_section() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-prompt-pressure";
    let context_version = "ctx-pressure";
    let path = task_version_json_path(root.path(), task_id, context_version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "context_version": context_version,
            "artifact_id": context_version,
            "brief": "## Task Objective\nKeep only the decisive replay context.",
            "sections": [
                {"id": "objective", "title": "Objective", "body": "finish the prompt pressure verifier"},
                {"id": "search_evidence", "title": "Search Evidence", "body": "line with redundant evidence ".repeat(90)},
                {"id": "next_action", "title": "Next Action", "body": "run focused verifier"}
            ],
            "next_action_summary": "run focused verifier"
        }))
        .unwrap(),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.prompt_pressure",
            "arguments": {
                "task_id": task_id,
                "context_version": context_version,
                "next_prompt": "Continue the handoff and implement the focused verifier.",
                "budget_tokens": 220
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["pressure"], "over_budget");
    assert_eq!(response["structuredContent"]["over_budget"], true);
    assert_eq!(
        response["structuredContent"]["largest_removable_sections"][0]["id"],
        "search_evidence"
    );
    assert!(
        response["structuredContent"]["pointer_savings_tokens"]
            .as_u64()
            .unwrap()
            > 100
    );
    assert!(
        response["structuredContent"]["pointer_total_tokens"]
            .as_u64()
            .unwrap()
            < response["structuredContent"]["total_tokens"]
                .as_u64()
                .unwrap()
    );
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 768
    );
}

#[test]
fn handoff_diff_reports_changed_next_action_as_top_delta() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-diff";
    for (context_version, next_action) in [
        ("ctx-before", "run cargo check before editing"),
        ("ctx-after", "edit cmd_mcp_native.rs before cargo check"),
    ] {
        let path = task_version_json_path(root.path(), task_id, context_version);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "context_version": context_version,
                "artifact_id": context_version,
                "brief": "## Task Objective\nFinish the handoff diff verifier.",
                "sections": [{"id": "context_debt", "title": "Context Debt", "body": "none"}],
                "evidence_artifact_ids": ["artifact-1"],
                "next_action_summary": next_action
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_diff",
            "arguments": {
                "task_id": task_id,
                "left_context_version": "ctx-before",
                "right_context_version": "ctx-after"
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["top_delta"], "next_action");
    assert_eq!(
        response["structuredContent"]["deltas"][0]["field"],
        "next_action"
    );
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 1024
    );
}

#[test]
fn handoff_compress_preserves_objective_and_next_action_sections() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-compress";
    let context_version = "ctx-compress";
    let path = task_version_json_path(root.path(), task_id, context_version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "context_version": context_version,
            "artifact_id": context_version,
            "brief": "## Task Objective\nCompress the handoff without losing replay anchors.",
            "sections": [
                {"id": "objective", "title": "Objective", "body": "preserve this objective anchor ".repeat(40)},
                {"id": "next_action", "title": "Next Action", "body": "preserve this next action anchor ".repeat(40)},
                {"id": "search_evidence", "title": "Search Evidence", "body": "drop redundant search result ".repeat(120)}
            ],
            "evidence_artifact_ids": ["artifact-1"],
            "next_action_summary": "continue with focused verification"
        }))
        .unwrap(),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_compress",
            "arguments": {
                "task_id": task_id,
                "context_version": context_version,
                "next_prompt": "Continue with focused verification.",
                "budget_tokens": 350
            }
        }),
    )
    .unwrap();

    let recommendations = response["structuredContent"]["recommendations"]
        .as_array()
        .unwrap();
    assert!(recommendations
        .iter()
        .any(|recommendation| recommendation["id"] == "search_evidence"));
    assert!(!recommendations
        .iter()
        .any(|recommendation| recommendation["id"] == "objective"));
    assert!(!recommendations
        .iter()
        .any(|recommendation| recommendation["id"] == "next_action"));
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 1024
    );
}

#[test]
fn handoff_dependency_lint_flags_missing_artifact_handle() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-dependency-lint";
    let context_version = "ctx-dependency-lint";
    let path = task_version_json_path(root.path(), task_id, context_version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "context_version": context_version,
            "artifact_id": context_version,
            "brief": "## Task Objective\nReplay artifact-present and artifact-missing.",
            "sections": [{
                "id": "evidence",
                "title": "Evidence",
                "body": "Use artifact-present for proof. artifact-missing is referenced but not attached."
            }],
            "evidence_artifact_ids": ["artifact-present"],
            "next_action_summary": "fetch attached proof"
        }))
        .unwrap(),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_lint_dependencies",
            "arguments": {
                "task_id": task_id,
                "context_version": context_version
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["ok"], false);
    assert_eq!(response["structuredContent"]["issue_count"], 1);
    assert_eq!(
        response["structuredContent"]["issues"][0]["reference"],
        "artifact-missing"
    );
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 1024
    );
}

#[test]
fn handoff_path_lint_flags_missing_path_reference() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/lib.rs"), "pub fn present() {}\n").unwrap();
    let task_id = "task-handoff-path-lint";
    let context_version = "ctx-path-lint";
    let path = task_version_json_path(root.path(), task_id, context_version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "context_version": context_version,
            "artifact_id": context_version,
            "brief": "## Task Objective\nCheck src/lib.rs and src/missing.rs before editing.",
            "sections": [{
                "id": "next_action",
                "title": "Next Action",
                "body": "Read src/lib.rs first, then verify src/missing.rs exists."
            }],
            "changed_paths_since_checkpoint": ["src/lib.rs"],
            "next_action_summary": "read referenced files"
        }))
        .unwrap(),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_lint_paths",
            "arguments": {
                "task_id": task_id,
                "context_version": context_version
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["ok"], false);
    assert_eq!(response["structuredContent"]["issue_count"], 1);
    assert_eq!(
        response["structuredContent"]["issues"][0]["reference"],
        "src/missing.rs"
    );
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 1024
    );
}

#[test]
fn handoff_test_lint_flags_named_test_without_command() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-test-lint";
    let context_version = "ctx-test-lint";
    let path = task_version_json_path(root.path(), task_id, context_version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "context_version": context_version,
            "artifact_id": context_version,
            "brief": "## Task Objective\nVerify missing_command_test and command_backed_test.",
            "sections": [{
                "id": "verification",
                "title": "Verification",
                "body": "Run missing_command_test later.\nUse cargo test -p suite-cli command_backed_test now."
            }],
            "next_action_summary": "verify named tests"
        }))
        .unwrap(),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_lint_tests",
            "arguments": {
                "task_id": task_id,
                "context_version": context_version
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["ok"], false);
    assert_eq!(response["structuredContent"]["issue_count"], 1);
    assert_eq!(
        response["structuredContent"]["issues"][0]["reference"],
        "missing_command_test"
    );
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 1024
    );
}

#[test]
fn handoff_stale_command_lint_flags_pre_edit_command() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-stale-command-lint";
    let context_version = "ctx-stale-command-lint";
    let path = task_version_json_path(root.path(), task_id, context_version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "context_version": context_version,
            "artifact_id": context_version,
            "brief": "## Task Objective\nVerify the command freshness.",
            "sections": [{
                "id": "verification",
                "title": "Verification",
                "body": "cargo test -p suite-cli stale_command_test\ncargo test -p suite-cli fresh_command_test"
            }],
            "changed_paths_since_checkpoint": ["src/lib.rs"],
            "next_action_summary": "trust only post-edit verification"
        }))
        .unwrap(),
    )
    .unwrap();
    let events_dir = root.path().join(".packet28/daemon/tasks");
    std::fs::create_dir_all(&events_dir).unwrap();
    let events_path = events_dir.join(format!("{task_id}.events.jsonl"));
    std::fs::write(
        &events_path,
        format!(
            "{}\n",
            [
                json!({
                    "seq": 1,
                    "task_id": task_id,
                    "event": {
                        "kind": "command_finished",
                        "occurred_at_unix": 10,
                        "data": {"command": "cargo test -p suite-cli stale_command_test"}
                    }
                })
                .to_string(),
                json!({
                    "seq": 2,
                    "task_id": task_id,
                    "event": {
                        "kind": "file_edited",
                        "occurred_at_unix": 20,
                        "data": {"paths": ["src/lib.rs"]}
                    }
                })
                .to_string(),
                json!({
                    "seq": 3,
                    "task_id": task_id,
                    "event": {
                        "kind": "command_finished",
                        "occurred_at_unix": 30,
                        "data": {"command": "cargo test -p suite-cli fresh_command_test"}
                    }
                })
                .to_string(),
            ]
            .join("\n")
        ),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_lint_stale_commands",
            "arguments": {
                "task_id": task_id,
                "context_version": context_version
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["ok"], false);
    assert_eq!(response["structuredContent"]["issue_count"], 1);
    assert_eq!(
        response["structuredContent"]["issues"][0]["reference"],
        "cargo test -p suite-cli stale_command_test"
    );
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 1024
    );
}

#[test]
fn handoff_environment_lint_flags_missing_env_var() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-environment-lint";
    let context_version = "ctx-environment-lint";
    let path = task_version_json_path(root.path(), task_id, context_version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "context_version": context_version,
            "artifact_id": context_version,
            "brief": "## Task Objective\nCheck command environment.",
            "sections": [{
                "id": "verification",
                "title": "Verification",
                "body": "cargo test -p suite-cli needs_env_test $PACKET28_ENV_LINT_SHOULD_BE_MISSING_12345\ncargo test -p suite-cli present_tool_test"
            }],
            "next_action_summary": "verify command environment before handoff"
        }))
        .unwrap(),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_lint_environment",
            "arguments": {
                "task_id": task_id,
                "context_version": context_version
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["ok"], false);
    assert_eq!(response["structuredContent"]["issue_count"], 1);
    assert_eq!(
        response["structuredContent"]["issues"][0]["reference"],
        "PACKET28_ENV_LINT_SHOULD_BE_MISSING_12345"
    );
    assert_eq!(
        response["structuredContent"]["issues"][0]["kind"],
        "missing_env"
    );
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 1024
    );
}

#[test]
fn handoff_lint_all_reports_exact_failing_categories() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-lint-all";
    let context_version = "ctx-lint-all";
    let path = task_version_json_path(root.path(), task_id, context_version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "context_version": context_version,
            "artifact_id": context_version,
            "evidence_artifact_ids": ["artifact-present"],
            "brief": "## Task Objective\nReplay a handoff with artifact-ghost and docs/missing.md.",
            "sections": [{
                "id": "verification",
                "title": "Verification",
                "body": "Run missing_command_test after checking src/lib.rs.\ncargo test -p suite-cli stale_command_test\ncargo test -p suite-cli fresh_command_test $PACKET28_LINT_ALL_MISSING_ENV_12345"
            }],
            "changed_paths_since_checkpoint": ["src/lib.rs"],
            "next_action_summary": "fix the aggregate handoff lint failures"
        }))
        .unwrap(),
    )
    .unwrap();
    let events_dir = root.path().join(".packet28/daemon/tasks");
    std::fs::create_dir_all(&events_dir).unwrap();
    let events_path = events_dir.join(format!("{task_id}.events.jsonl"));
    std::fs::write(
        &events_path,
        format!(
            "{}\n",
            [
                json!({
                    "seq": 1,
                    "task_id": task_id,
                    "event": {
                        "kind": "command_finished",
                        "occurred_at_unix": 10,
                        "data": {"command": "cargo test -p suite-cli stale_command_test"}
                    }
                })
                .to_string(),
                json!({
                    "seq": 2,
                    "task_id": task_id,
                    "event": {
                        "kind": "file_edited",
                        "occurred_at_unix": 20,
                        "data": {"paths": ["src/lib.rs"]}
                    }
                })
                .to_string(),
            ]
            .join("\n")
        ),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_lint_all",
            "arguments": {
                "task_id": task_id,
                "context_version": context_version
            }
        }),
    )
    .unwrap();
    let categories = response["structuredContent"]["failing_categories"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();

    assert_eq!(response["structuredContent"]["ok"], false);
    assert_eq!(
        categories,
        vec![
            "dependencies",
            "paths",
            "tests",
            "stale_commands",
            "environment"
        ]
    );
    assert!(
        response["structuredContent"]["issue_count"]
            .as_u64()
            .unwrap()
            >= 5
    );
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 1024
    );
}

#[test]
fn handoff_fix_plan_recommends_path_test_and_env_repairs() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-fix-plan";
    let context_version = "ctx-fix-plan";
    let path = task_version_json_path(root.path(), task_id, context_version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "context_version": context_version,
            "artifact_id": context_version,
            "brief": "## Task Objective\nRepair a handoff that mentions docs/missing.md.",
            "sections": [{
                "id": "verification",
                "title": "Verification",
                "body": "Run missing_command_test after setup.\ncargo test -p suite-cli env_backed_test $PACKET28_FIX_PLAN_MISSING_ENV_12345"
            }],
            "changed_paths_since_checkpoint": ["src/lib.rs"],
            "next_action_summary": "repair path, test, and environment blockers"
        }))
        .unwrap(),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_fix_plan",
            "arguments": {
                "task_id": task_id,
                "context_version": context_version
            }
        }),
    )
    .unwrap();
    let kinds = response["structuredContent"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|action| action.get("kind").and_then(Value::as_str))
        .collect::<Vec<_>>();

    assert_eq!(response["structuredContent"]["status"], "needs_fix");
    assert_eq!(
        kinds,
        vec![
            "read_or_correct_path",
            "add_test_command",
            "setup_environment"
        ]
    );
    assert_eq!(
        response["structuredContent"]["actions"][1]["command"],
        "cargo test missing_command_test"
    );
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 768
    );
}

#[test]
fn handoff_repair_verify_reports_cleared_categories() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-repair-verify";
    let before_context_version = "ctx-repair-before";
    let after_context_version = "ctx-repair-after";
    let existing_path = root.path().join("docs/existing.md");
    std::fs::create_dir_all(existing_path.parent().unwrap()).unwrap();
    std::fs::write(&existing_path, "fixed path").unwrap();
    for (context_version, body, path_ref) in [
        (
            before_context_version,
            "Run missing_command_test after setup.\ncargo test -p suite-cli env_backed_test $PACKET28_REPAIR_VERIFY_MISSING_ENV_12345",
            "docs/missing.md",
        ),
        (
            after_context_version,
            "cargo test -p suite-cli missing_command_test",
            "docs/existing.md",
        ),
    ] {
        let path = task_version_json_path(root.path(), task_id, context_version);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "context_version": context_version,
                "artifact_id": context_version,
                "brief": format!("## Task Objective\nRepair handoff reference {path_ref}."),
                "sections": [{
                    "id": "verification",
                    "title": "Verification",
                    "body": body
                }],
                "changed_paths_since_checkpoint": ["src/lib.rs"],
                "next_action_summary": "verify repaired handoff"
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_repair_verify",
            "arguments": {
                "task_id": task_id,
                "before_context_version": before_context_version,
                "after_context_version": after_context_version
            }
        }),
    )
    .unwrap();
    let cleared = response["structuredContent"]["cleared_categories"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();

    assert_eq!(response["structuredContent"]["verified"], true);
    assert_eq!(cleared, vec!["paths", "tests", "environment"]);
    assert!(response["structuredContent"]["remaining_categories"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(response["structuredContent"]["regressed_categories"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 768
    );
}

#[test]
fn handoff_lint_trends_reports_recurring_and_cleared_categories() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-lint-trends";
    for (context_version, body) in [
        (
            "ctx-trend-1",
            "cargo test -p suite-cli env_backed_test $PACKET28_TREND_MISSING_ENV_12345",
        ),
        (
            "ctx-trend-2",
            "cargo test -p suite-cli env_backed_test $PACKET28_TREND_MISSING_ENV_12345",
        ),
        ("ctx-trend-3", "cargo test -p suite-cli env_backed_test"),
    ] {
        let path = task_version_json_path(root.path(), task_id, context_version);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "context_version": context_version,
                "artifact_id": context_version,
                "brief": "## Task Objective\nTrack repeated handoff lint blockers.",
                "sections": [{
                    "id": "verification",
                    "title": "Verification",
                    "body": body
                }],
                "changed_paths_since_checkpoint": ["src/lib.rs"],
                "next_action_summary": "verify handoff lint trends"
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_lint_trends",
            "arguments": {
                "task_id": task_id,
                "artifact_ids": ["ctx-trend-1", "ctx-trend-2", "ctx-trend-3"]
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["artifact_count"], 3);
    assert_eq!(
        response["structuredContent"]["recurring_categories"][0]["category"],
        "environment"
    );
    assert_eq!(
        response["structuredContent"]["recurring_categories"][0]["count"],
        2
    );
    assert_eq!(
        response["structuredContent"]["cleared_categories"][0],
        "environment"
    );
    assert!(response["structuredContent"]["latest_blocking_categories"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 768
    );
}

#[test]
fn handoff_lint_regressions_flags_reintroduced_category() {
    let root = tempfile::tempdir().unwrap();
    let task_id = "task-handoff-lint-regressions";
    for (context_version, body) in [
        (
            "ctx-regression-1",
            "cargo test -p suite-cli env_backed_test $PACKET28_REGRESSION_MISSING_ENV_12345",
        ),
        (
            "ctx-regression-2",
            "cargo test -p suite-cli env_backed_test",
        ),
        (
            "ctx-regression-3",
            "cargo test -p suite-cli env_backed_test $PACKET28_REGRESSION_MISSING_ENV_12345",
        ),
    ] {
        let path = task_version_json_path(root.path(), task_id, context_version);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "context_version": context_version,
                "artifact_id": context_version,
                "brief": "## Task Objective\nDetect handoff lint regressions.",
                "sections": [{
                    "id": "verification",
                    "title": "Verification",
                    "body": body
                }],
                "changed_paths_since_checkpoint": ["src/lib.rs"],
                "next_action_summary": "gate reintroduced handoff blockers"
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let session = Arc::new(Mutex::new(McpSessionState::default()));
    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.handoff_lint_regressions",
            "arguments": {
                "task_id": task_id,
                "artifact_ids": ["ctx-regression-1", "ctx-regression-2", "ctx-regression-3"]
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["ok"], false);
    assert_eq!(response["structuredContent"]["regression_count"], 1);
    assert_eq!(
        response["structuredContent"]["regressions"][0]["category"],
        "environment"
    );
    assert_eq!(
        response["structuredContent"]["regressions"][0]["latest_artifact_id"],
        "ctx-regression-3"
    );
    assert!(
        serde_json::to_string(&response["structuredContent"])
            .unwrap()
            .len()
            < 512
    );
}

#[test]
fn recommend_next_tool_changes_with_focus_freshness_and_roi() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".packet28")).unwrap();
    std::fs::write(
        root.path().join(".packet28/run-savings.jsonl"),
        [
            json!({
                "command": "Packet28 run -- cargo test",
                "cwd": root.path().display().to_string(),
                "family": "rust",
                "canonical_kind": "cargo_test",
                "exit_code": 0,
                "raw_est_tokens": 1200,
                "reduced_est_tokens": 100,
                "savings_percent": 91.7,
                "fallback_reason": null,
                "failure_fingerprint": null,
                "changed_paths": ["src/lib.rs"],
                "timestamp_unix_ms": 20
            })
            .to_string(),
            json!({
                "command": "Packet28 run -- npm test",
                "cwd": root.path().display().to_string(),
                "family": "node",
                "canonical_kind": "npm_test",
                "exit_code": 0,
                "raw_est_tokens": 300,
                "reduced_est_tokens": 100,
                "savings_percent": 66.7,
                "fallback_reason": null,
                "failure_fingerprint": null,
                "changed_paths": [],
                "timestamp_unix_ms": 10
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));

    let roi = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.recommend_next_tool",
            "arguments": {
                "task_id": "task-route",
                "query": "what should I run next",
                "max_recommendations": 1
            }
        }),
    )
    .unwrap();
    assert_eq!(
        roi["structuredContent"]["recommendations"][0]["command"],
        "Packet28 run -- cargo test"
    );
    assert!(roi["structuredContent"]["token_estimate"].as_u64().unwrap() < 256);

    let focused = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.recommend_next_tool",
            "arguments": {
                "task_id": "task-route",
                "focus_paths": ["src/lib.rs"],
                "max_recommendations": 1
            }
        }),
    )
    .unwrap();
    assert_eq!(
        focused["structuredContent"]["recommendations"][0]["risk"],
        "stale_focus_evidence"
    );
    assert!(
        focused["structuredContent"]["recommendations"][0]["command"]
            .as_str()
            .unwrap()
            .contains("packet28.read_regions")
    );
}

#[test]
fn validate_tool_outcome_does_not_treat_fallback_as_success() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".packet28")).unwrap();
    std::fs::write(
        root.path().join(".packet28/run-savings.jsonl"),
        json!({
            "command": "Packet28 run -- rg auth",
            "cwd": root.path().display().to_string(),
            "family": "search",
            "canonical_kind": "rg",
            "exit_code": 0,
            "raw_est_tokens": 900,
            "reduced_est_tokens": 200,
            "savings_percent": 77.8,
            "fallback_reason": "fff auto preferred backend failed: launch error",
            "failure_fingerprint": null,
            "changed_paths": [],
            "timestamp_unix_ms": 20
        })
        .to_string(),
    )
    .unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));

    let response = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.validate_tool_outcome",
            "arguments": {
                "task_id": "task-outcome",
                "command": "rg auth"
            }
        }),
    )
    .unwrap();

    assert_eq!(response["structuredContent"]["status"], "fallback");
    assert_eq!(response["structuredContent"]["valid_success"], false);
    assert!(response["structuredContent"]["evidence"]
        .as_str()
        .unwrap()
        .contains("fallback_reason="));
}

#[test]
fn patch_risk_requires_broader_checks_for_shared_unmapped_paths() {
    let root = tempfile::tempdir().unwrap();
    let state_dir = root.path().join(".covy/state");
    std::fs::create_dir_all(&state_dir).unwrap();
    let mut index = suite_packet_core::TestMapIndex::default();
    index.file_to_tests.insert(
        "src/leaf.rs".to_string(),
        ["tests/leaf_test.rs".to_string()].into_iter().collect(),
    );
    testy_core::pipeline_testmap::write_testmap(&state_dir.join("testmap.bin"), &index).unwrap();
    let session = Arc::new(Mutex::new(McpSessionState::default()));

    let leaf = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.patch_risk",
            "arguments": {
                "task_id": "task-risk",
                "paths": ["src/leaf.rs"]
            }
        }),
    )
    .unwrap();
    let shared = handle_tool_call(
        root.path(),
        &session,
        json!({
            "name": "packet28.patch_risk",
            "arguments": {
                "task_id": "task-risk",
                "paths": ["src/lib.rs"]
            }
        }),
    )
    .unwrap();

    assert!(
        shared["structuredContent"]["score"].as_u64().unwrap()
            > leaf["structuredContent"]["score"].as_u64().unwrap()
    );
    assert_eq!(shared["structuredContent"]["risk"], "medium");
    assert!(shared["structuredContent"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "missing_testmap_mappings=1"));
    assert!(leaf["structuredContent"]["required_checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "run tests/leaf_test.rs"));
}
