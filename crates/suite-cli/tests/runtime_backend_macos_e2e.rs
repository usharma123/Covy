#[cfg(target_os = "macos")]
#[path = "support/runtime_backend.rs"]
mod runtime_backend;

#[cfg(target_os = "macos")]
use runtime_backend::{
    ensure_packet28d_built, large_agents_text, suite_cmd, swap_reports,
    wait_for_active_swap_report, write_executable_script,
};
#[cfg(target_os = "macos")]
use serde_json::{json, Value};
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
fn sha256(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

#[cfg(target_os = "macos")]
#[test]
fn test_runtime_backend_macos_run_command_auto_backend_swaps_instruction_file_and_restores_it() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let original = large_agents_text(120);
    fs::write(dir.path().join("AGENTS.md"), &original).unwrap();
    fs::write(
        dir.path().join("packet28-instruction.json"),
        r#"{"schema_version":1,"mode":"adaptive","stable_config":{}}"#,
    )
    .unwrap();

    let claude = dir.path().join("claude");
    write_executable_script(
        &claude,
        "#!/bin/sh\nprintf '%s|%s\\n' \"$PACKET28_RUNTIME_BACKEND\" \"$PACKET28_AGENT_FAMILY\"\ncat AGENTS.md\n",
    );

    let output = suite_cmd()
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", claude.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(stdout.contains("macos_swap|claude"));
    let reports = swap_reports(&dir.path().join(".packet28/runtime/macos-swap"));
    assert_eq!(reports.len(), 1);
    let report: Value = serde_json::from_slice(&fs::read(&reports[0]).unwrap()).unwrap();
    assert!(
        stdout.contains("# [p28:adaptive:v1]"),
        "expected virtualized instruction content, got {stdout:?}; report={report}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(),
        original
    );

    assert_eq!(
        report.get("state").and_then(Value::as_str),
        Some("restored")
    );
    assert_eq!(
        report.get("backend_kind").and_then(Value::as_str),
        Some("macos_swap")
    );
    let files = report.get("files").and_then(Value::as_array).unwrap();
    assert!(files.iter().any(|item| {
        item.get("path").and_then(Value::as_str) == Some("AGENTS.md")
            && item.get("decision").and_then(Value::as_str) == Some("rewrite")
    }));
}

#[cfg(target_os = "macos")]
#[test]
fn test_runtime_backend_macos_run_command_recovers_stale_swap_session_before_launch() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let original = "tiny original\n";
    let swapped = "# [p28:virtual] stale\n";
    let agents = dir.path().join("AGENTS.md");
    fs::write(&agents, swapped).unwrap();
    let backup = dir.path().join("AGENTS.md.p28-backup.demo");
    let temp = dir.path().join("AGENTS.md.p28-rewrite.demo.tmp");
    fs::write(&backup, original).unwrap();
    fs::write(&temp, swapped).unwrap();
    let report_dir = dir.path().join(".packet28/runtime/macos-swap");
    fs::create_dir_all(&report_dir).unwrap();
    fs::write(
        report_dir.join("demo.json"),
        serde_json::to_vec_pretty(&json!({
            "session_id":"demo",
            "workspace_root": dir.path(),
            "command":["claude"],
            "agent_family":"claude",
            "backend_kind":"macos_swap",
            "pid":999999u32,
            "started_at":1u64,
            "state":"active",
            "files":[{
                "path":"AGENTS.md",
                "decision":"rewrite",
                "reason":null,
                "original_sha256":sha256(original),
                "content_sha256":sha256(swapped),
                "task_label":"default",
                "original_bytes":swapped.len(),
                "rewritten_bytes":swapped.len(),
                "backup_path":backup,
                "temp_path":temp
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let claude = dir.path().join("claude");
    write_executable_script(&claude, "#!/bin/sh\ncat AGENTS.md\n");

    let output = suite_cmd()
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", claude.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert_eq!(stdout, original);
    assert_eq!(fs::read_to_string(&agents).unwrap(), original);

    let recovered: Value =
        serde_json::from_slice(&fs::read(report_dir.join("demo.json")).unwrap()).unwrap();
    assert_eq!(
        recovered.get("state").and_then(Value::as_str),
        Some("restored")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_runtime_backend_macos_run_command_restores_files_after_sigterm() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let original = large_agents_text(80);
    let agents = dir.path().join("AGENTS.md");
    fs::write(&agents, &original).unwrap();

    let claude = dir.path().join("claude");
    write_executable_script(
        &claude,
        "#!/bin/sh\nprintf '%s' \"$PACKET28_RUNTIME_BACKEND\" > child-backend.txt\nwhile true; do sleep 1; done\n",
    );

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", claude.to_str().unwrap()])
        .spawn()
        .unwrap();

    let report_dir = dir.path().join(".packet28/runtime/macos-swap");
    wait_for_active_swap_report(&report_dir, Duration::from_secs(10));
    let active_reports = swap_reports(&report_dir);
    assert_eq!(
        active_reports.len(),
        1,
        "active swap report was not published"
    );
    let active: Value = serde_json::from_slice(&fs::read(&active_reports[0]).unwrap()).unwrap();
    assert_eq!(active.get("state").and_then(Value::as_str), Some("active"));

    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let status = child.wait().unwrap();
    assert!(!status.success());
    let restore_start = std::time::Instant::now();
    loop {
        if fs::read_to_string(&agents).ok().as_deref() == Some(original.as_str()) {
            break;
        }
        assert!(
            restore_start.elapsed() < Duration::from_secs(3),
            "timed out waiting for AGENTS.md to be restored"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn test_runtime_backend_macos_serializes_two_workspace_swaps() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let original = large_agents_text(80);
    let agents = dir.path().join("AGENTS.md");
    fs::write(&agents, &original).unwrap();

    let claude = dir.path().join("claude");
    write_executable_script(
        &claude,
        "#!/bin/sh\nprintf ready > first-child-ready.txt\nwhile true; do sleep 1; done\n",
    );

    let mut first = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", claude.to_str().unwrap()])
        .spawn()
        .unwrap();
    let report_dir = dir.path().join(".packet28/runtime/macos-swap");
    wait_for_active_swap_report(&report_dir, Duration::from_secs(10));
    let reports = swap_reports(&report_dir);
    assert_eq!(
        reports.len(),
        1,
        "first swap did not publish an active report"
    );
    let report: Value = serde_json::from_slice(&fs::read(&reports[0]).unwrap()).unwrap();
    assert_eq!(report.get("state").and_then(Value::as_str), Some("active"));

    let second = suite_cmd()
        .current_dir(dir.path())
        .args([
            "run",
            "--root",
            ".",
            "--backend",
            "macos-swap",
            "--",
            "/usr/bin/true",
        ])
        .output()
        .unwrap();

    // SAFETY: `first` is a live child process created by this test.
    unsafe {
        libc::kill(first.id() as i32, libc::SIGTERM);
    }
    let status = first.wait().unwrap();
    assert!(!status.success());
    assert_eq!(fs::read_to_string(&agents).unwrap(), original);
    assert!(
        !second.status.success(),
        "overlapping macOS swap unexpectedly succeeded: {second:?}"
    );
    assert!(
        String::from_utf8_lossy(&second.stderr)
            .contains("another macOS swap session currently owns workspace"),
        "unexpected overlapping-swap stderr: {:?}",
        String::from_utf8_lossy(&second.stderr)
    );
}
