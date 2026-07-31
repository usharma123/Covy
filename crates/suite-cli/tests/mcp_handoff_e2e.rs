#[path = "support/mcp_handoff.rs"]
mod mcp_handoff;
#[expect(
    dead_code,
    reason = "this integration binary exercises a focused subset of the shared harness"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use mcp_handoff::{
    ensure_packet28d_built, init_repo, initialize_mcp_session, read_mcp_message_for_id,
    run_claude_hook, start_mcp_server, stop_mcp_server, suite_cmd, write_intention_via_mcp,
    write_mcp_message, write_repo_fixture,
};
use serde_json::{json, Value};
use tempfile::TempDir;

#[test]
#[cfg(unix)]
fn test_mcp_handoff_prepare_requires_checkpoint_and_persists_artifact() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let mut server = start_mcp_server(dir.path());

    initialize_mcp_session(&mut server);
    let intention = write_intention_via_mcp(
        &mut server,
        2,
        "task-handoff",
        "Inspect Alpha before editing it",
        "investigating",
        &["src/alpha.rs"],
    );
    assert_eq!(intention["result"]["structuredContent"]["accepted"], true);

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.prepare_handoff",
                "arguments":{
                    "task_id":"task-handoff"
                }
            }
        }),
    );
    let not_ready = read_mcp_message_for_id(&mut server, 3);
    let not_ready_payload = &not_ready["result"]["structuredContent"];
    assert_eq!(not_ready_payload["handoff_ready"], false);
    assert!(not_ready_payload["context"].is_null());

    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"Stop",
            "task_id":"task-handoff",
            "session_id":"session-task-handoff",
        }),
    );
    assert_eq!(status, 0);

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.prepare_handoff",
                "arguments":{
                    "task_id":"task-handoff",
                    "response_mode":"slim"
                }
            }
        }),
    );
    let handoff = read_mcp_message_for_id(&mut server, 4);
    let handoff_payload = &handoff["result"]["structuredContent"];
    assert_eq!(handoff_payload["handoff_ready"], true);
    assert!(handoff_payload["latest_checkpoint_id"].is_null());
    assert_eq!(
        handoff_payload["latest_intention"]["text"],
        "Inspect Alpha before editing it"
    );
    let handoff_context = &handoff_payload["context"];
    assert_eq!(handoff_context["response_mode"], "slim");
    assert_eq!(handoff_context["handoff_ready"], true);
    assert!(handoff_context["brief"]
        .as_str()
        .unwrap()
        .contains("Latest Intention"));
    let handoff_artifact_id = handoff_context["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28.fetch_context",
                "arguments":{
                    "task_id":"task-handoff",
                    "artifact_id": handoff_artifact_id
                }
            }
        }),
    );
    let fetched = read_mcp_message_for_id(&mut server, 5);
    let fetched_payload = &fetched["result"]["structuredContent"];
    assert_eq!(fetched_payload["response_mode"], "full");
    assert_eq!(
        fetched_payload["latest_intention"]["step_id"],
        "investigating"
    );
    assert!(fetched_payload["sections"]
        .as_array()
        .unwrap()
        .iter()
        .any(|section| section["id"] == "agent_intention"));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"packet28.task_status",
                "arguments":{
                    "task_id":"task-handoff"
                }
            }
        }),
    );
    let status = read_mcp_message_for_id(&mut server, 6);
    let status_payload = &status["result"]["structuredContent"];
    assert_eq!(status_payload["handoff_ready"], true);
    assert!(status_payload["latest_handoff_checkpoint_id"].is_null());
    assert_eq!(
        status_payload["latest_handoff_artifact_id"],
        handoff_context["artifact_id"]
    );

    let (resume_status, resume_output) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"SessionStart",
            "task_id":"task-handoff",
            "session_id":"session-task-handoff-resume",
            "cwd": dir.path().display().to_string(),
        }),
    );
    assert_eq!(resume_status, 0);
    let resume_payload: Value = serde_json::from_str(&resume_output).unwrap();
    let additional_context = resume_payload["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(additional_context.contains("Packet28 Context v"));
    assert!(additional_context.contains("Latest Intention"));

    stop_mcp_server(server);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
