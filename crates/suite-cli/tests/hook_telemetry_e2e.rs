#[path = "support/hook_telemetry.rs"]
mod hook_telemetry;
#[expect(
    dead_code,
    reason = "this integration binary exercises a focused subset of the shared harness"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use hook_telemetry::{run_hook_raw_with_env, suite_cmd};
use predicates::prelude::*;
use serde_json::{json, Value};
use std::path::Path;
use tempfile::TempDir;

fn run_claude_hook_with_home(root: &Path, home: &Path, payload: &Value) -> (i32, String, String) {
    run_hook_raw_with_env(
        "claude",
        root,
        &serde_json::to_string(payload).unwrap(),
        &[("HOME", home.as_os_str())],
    )
}

#[test]
fn test_hook_telemetry_records_local_event_log_stats_and_dashboard_count() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let payload = json!({
        "hook_event_name":"PostToolUse",
        "task_id":"hook-telemetry-task",
        "session_id":"hook-telemetry-session",
        "project":"coverage-hook",
        "matcher":"Bash",
        "tool_name":"Bash",
        "tool_input":{"command":"git status --short"},
        "tool_response":{"stdout":"- Hook auto extraction stores post tool facts\n","stderr":"","exit_code":0}
    });

    let (status, _stdout, stderr) = run_claude_hook_with_home(root.path(), home.path(), &payload);
    assert_eq!(status, 0, "stderr={stderr}");

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "log", "--limit", "5", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"runtime\":\"claude\""))
        .stdout(predicate::str::contains("\"event_kind\":\"post_tool_use\""))
        .stdout(predicate::str::contains("hook-telemetry-session"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event_count\":1"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "dashboard",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"hook_event_history\":1"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["memory", "pending", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pending_extraction_count\":1"));
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["transcript", "search", "post tool facts", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hook-telemetry-session"))
        .stdout(predicate::str::contains(
            "auto extraction stores post tool facts",
        ));
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["memory", "pending", "process", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"extracted_count\":1"))
        .stdout(predicate::str::contains("\"deleted_count\":1"));
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "post tool facts",
            "--project",
            "coverage-hook",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "auto extraction stores post tool facts",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn test_hook_telemetry_failure_output_is_searchable_transcript_context() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let payload = json!({
        "hook_event_name":"PostToolUseFailure",
        "task_id":"hook-failure-task",
        "session_id":"hook-failure-session",
        "project":"coverage-hook",
        "matcher":"Bash",
        "tool_name":"Bash",
        "tool_input":{"command":"cargo test failing_case"},
        "error":"failure transcript keeps compiler diagnostic E0425"
    });

    let (status, _stdout, stderr) = run_claude_hook_with_home(root.path(), home.path(), &payload);
    assert_eq!(status, 0, "stderr={stderr}");

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["transcript", "search", "compiler diagnostic", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hook-failure-session"))
        .stdout(predicate::str::contains("E0425"))
        .stdout(predicate::str::contains("\"source\":\"packet28-hook\""));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"event_kind\":\"post_tool_use_failure\"",
        ));
}

#[test]
fn test_hook_telemetry_session_end_is_recorded_in_local_lifecycle_log() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let payload = json!({
        "hook_event_name":"SessionEnd",
        "task_id":"hook-session-end-task",
        "session_id":"hook-session-end-session",
        "matcher":"session",
        "cwd": root.path().display().to_string(),
    });

    let (status, _stdout, stderr) = run_claude_hook_with_home(root.path(), home.path(), &payload);
    assert_eq!(status, 0, "stderr={stderr}");

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "log", "--limit", "5", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event_kind\":\"session_end\""))
        .stdout(predicate::str::contains("hook-session-end-session"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event_kind\":\"session_end\""))
        .stdout(predicate::str::contains("\"event_count\":1"));
}
