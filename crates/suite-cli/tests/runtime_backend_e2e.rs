#[expect(
    dead_code,
    reason = "platform-specific tests exercise a focused subset of the shared harness"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use assert_cmd::Command;
use predicates::prelude::*;
#[cfg(target_os = "linux")]
use std::fs;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[cfg(target_os = "linux")]
fn ensure_packet28d_built() {
    process_harness::ensure_packet28d_built();
}

#[cfg(target_os = "linux")]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[cfg(not(target_os = "linux"))]
#[test]
fn test_runtime_backend_cli_shell_command_reports_linux_only_support_on_macos() {
    let dir = tempfile::tempdir().unwrap();
    suite_cmd()
        .current_dir(dir.path())
        .args(["shell", "--root", "."])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Packet28 shell is only supported on Linux in Phase A",
        ));
}

#[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
#[test]
fn test_runtime_backend_cli_run_command_auto_backend_reports_missing_platform_backend() {
    let dir = tempfile::tempdir().unwrap();
    suite_cmd()
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", "sh", "-c", "printf ok"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Packet28 run --backend linux-oci is not implemented yet",
        ));
}

#[cfg(target_os = "linux")]
#[test]
fn test_runtime_backend_cli_shell_command_injects_ld_preload_for_explicit_commands() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    process_harness::build_workspace_package("context-instruct-shim");

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "shell",
            "--root",
            ".",
            "--",
            "sh",
            "-c",
            "printf '%s' \"$LD_PRELOAD\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(stdout.contains("libcontext_instruct_shim.so"));
}

#[cfg(target_os = "linux")]
#[test]
fn test_runtime_backend_cli_run_command_linux_preload_sets_backend_and_agent_family() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    fs::write(
        &claude,
        "#!/bin/sh\nprintf '%s|%s|%s' \"$LD_PRELOAD\" \"$PACKET28_RUNTIME_BACKEND\" \"$PACKET28_AGENT_FAMILY\"\n",
    )
    .unwrap();
    make_executable(&claude);
    process_harness::build_workspace_package("context-instruct-shim");

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "run",
            "--root",
            ".",
            "--backend",
            "linux-preload",
            "--",
            claude.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(stdout.contains("libcontext_instruct_shim.so"));
    assert!(stdout.contains("linux_preload"));
    assert!(stdout.contains("claude"));
}
