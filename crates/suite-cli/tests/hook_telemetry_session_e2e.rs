#[path = "support/hook_telemetry.rs"]
mod hook_telemetry;

use hook_telemetry::{run_hook_raw_with_env, suite_cmd};
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

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
