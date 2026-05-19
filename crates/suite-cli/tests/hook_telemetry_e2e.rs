use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed with {status}", args);
}

fn init_repo(root: &Path) {
    git(root, &["init"]);
}

fn write_repo_fixture(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("alpha.rs"),
        r#"
use crate::beta::Beta;

fn alpha() {}
struct Alpha;
"#,
    )
    .unwrap();
    fs::write(
        src.join("beta.rs"),
        r#"
fn beta() {}
enum Beta {
  A,
}
"#,
    )
    .unwrap();
}

fn run_hook_raw_with_env(
    runtime: &str,
    root: &Path,
    stdin_payload: &str,
    envs: &[(&str, &OsStr)],
) -> (i32, String, String) {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    command
        .current_dir(root)
        .args(["hook", runtime, "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_payload.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

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
#[cfg(unix)]
fn test_hook_telemetry_session_start_injects_wakeup_pack() {
    let dir = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    let project = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Session start should inject this Packet28 wakeup fact",
            "--project",
            &project,
            "--topic",
            "session-start",
            "--importance",
            "critical",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Session start has a second Packet28 wakeup fact that proves budgeted hook packs truncate deterministically",
            "--project",
            &project,
            "--topic",
            "session-start",
            "--importance",
            "high",
            "--json",
        ])
        .assert()
        .success();

    let payload = json!({
        "hook_event_name":"SessionStart",
        "task_id":"task-wakeup-hook",
        "session_id":"session-wakeup-hook",
        "cwd": dir.path().display().to_string(),
    });
    let (status, stdout, stderr) = run_hook_raw_with_env(
        "claude",
        dir.path(),
        &serde_json::to_string(&payload).unwrap(),
        &[("HOME", home.path().as_os_str())],
    );
    assert_eq!(status, 0, "stderr={stderr}");
    let rendered: Value = serde_json::from_str(&stdout).unwrap();
    let additional_context = rendered["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(additional_context.contains("Packet28 Wake-Up Pack"));
    assert!(additional_context.contains("Session start should inject this Packet28 wakeup fact"));
    assert!(additional_context.contains("Critical memories"));
    let (budget_status, budget_stdout, budget_stderr) = run_hook_raw_with_env(
        "claude",
        dir.path(),
        &serde_json::to_string(&payload).unwrap(),
        &[
            ("HOME", home.path().as_os_str()),
            ("PACKET28_HOOK_WAKEUP_TOKENS", OsStr::new("12")),
        ],
    );
    assert_eq!(budget_status, 0, "stderr={budget_stderr}");
    let budget_rendered: Value = serde_json::from_str(&budget_stdout).unwrap();
    let budget_context = budget_rendered["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(budget_context.contains("Packet28 Wake-Up Pack"));
    assert!(budget_context.contains("budget:"));
    assert!(budget_context.contains("truncated"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
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
