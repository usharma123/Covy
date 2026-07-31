#[path = "support/daemon_task_await.rs"]
mod daemon_task_await;
#[path = "support/daemon_task_core.rs"]
mod daemon_task_core;
#[path = "support/daemon_task_mcp.rs"]
mod daemon_task_mcp;
#[path = "support/daemon_task_seed.rs"]
mod daemon_task_seed;
#[expect(
    dead_code,
    reason = "shared integration harness APIs are exercised by sibling test binaries"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use predicates::prelude::*;
use serde_json::json;

use daemon_task_await::{
    await_handoff, await_newer_handoff, launch_agent_for_bootstrap_mode,
    repo_with_checkpointed_handoff, stop_daemon, task_status,
};
use daemon_task_core::{ensure_packet28d_built, suite_cmd};
use daemon_task_mcp::{
    initialize_mcp_session, run_claude_hook, start_mcp_server, stop_mcp_server,
    write_intention_via_mcp,
};

#[test]
#[cfg(unix)]
fn test_daemon_task_cli_await_handoff_reports_ready_status() {
    ensure_packet28d_built();
    let dir =
        repo_with_checkpointed_handoff("task-daemon-await", "Prepare daemon-owned handoff wait");

    let value = await_handoff(dir.path(), "task-daemon-await", "1000", "50");
    assert_eq!(value["task_status"]["handoff_ready"], true);
    assert!(value["waited_ms"].as_u64().unwrap() <= 1_000);
    assert!(value["polls"].as_u64().unwrap() >= 1);

    stop_daemon(dir.path());
}

#[test]
#[cfg(unix)]
fn test_daemon_task_cli_await_handoff_can_require_newer_context_version() {
    ensure_packet28d_built();
    let dir =
        repo_with_checkpointed_handoff("task-daemon-newer-handoff", "Prepare initial handoff");
    let mut server = start_mcp_server(dir.path());
    initialize_mcp_session(&mut server);

    let launch_value = launch_agent_for_bootstrap_mode(dir.path(), "task-daemon-newer-handoff");
    let launched_status = task_status(dir.path(), "task-daemon-newer-handoff");
    let previous_context_version = launched_status["latest_agent_context_version"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(launch_value["bootstrap_mode"], "handoff");

    suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--after-context-version",
            &previous_context_version,
            "--timeout-ms",
            "100",
            "--poll-ms",
            "20",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "newer handoff than context version",
        ));

    let _ = write_intention_via_mcp(
        &mut server,
        4,
        "task-daemon-newer-handoff",
        "Resume from a newer handoff",
        "editing",
        &["src/beta.rs"],
    );
    stop_mcp_server(server);
    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreCompact",
            "task_id":"task-daemon-newer-handoff",
            "session_id":"session-daemon-newer-handoff",
        }),
    );
    assert_eq!(status, 0);

    let value = await_newer_handoff(
        dir.path(),
        "task-daemon-newer-handoff",
        &previous_context_version,
    );
    assert_eq!(value["task_status"]["handoff_ready"], true);
    assert_ne!(
        value["task_status"]["latest_context_version"]
            .as_str()
            .unwrap(),
        previous_context_version
    );

    stop_daemon(dir.path());
}
