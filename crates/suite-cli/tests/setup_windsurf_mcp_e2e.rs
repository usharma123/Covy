#![cfg(unix)]

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn setup_windsurf_mcp_e2e_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

fn write_fake_packet28_mcp_binary(path: &Path) {
    let script = format!(
        "#!/bin/sh\n\
exec \"{}\" mcp serve \"$@\"\n",
        env!("CARGO_BIN_EXE_Packet28")
    );
    fs::write(path, script).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn install_windsurf_config(root: &TempDir, home: &TempDir, bin_dir: &TempDir) {
    write_fake_packet28_mcp_binary(&bin_dir.path().join("packet28-mcp"));
    fs::create_dir_all(home.path().join(".codeium").join("windsurf")).unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", bin_dir.path().display()),
        )
        .args([
            "setup",
            "--root",
            root.path().to_str().unwrap(),
            "--runtime",
            "windsurf",
            "--yes",
        ])
        .assert()
        .success();
}

fn stop_daemon(root: &TempDir, home: &TempDir) {
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn test_setup_windsurf_generated_mcp_config_smoke_test() {
    let _guard = setup_windsurf_mcp_e2e_lock();
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let bin_dir = TempDir::new().unwrap();
    install_windsurf_config(&root, &home, &bin_dir);

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", bin_dir.path().display()),
        )
        .args(["mcp", "smoke-test", "--from-config", "windsurf"])
        .assert()
        .success()
        .stdout(predicate::str::contains("MCP smoke test ok"));

    stop_daemon(&root, &home);
}

#[test]
fn test_setup_windsurf_doctor_passes_with_generated_mcp_config() {
    let _guard = setup_windsurf_mcp_e2e_lock();
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let bin_dir = TempDir::new().unwrap();
    install_windsurf_config(&root, &home, &bin_dir);

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", bin_dir.path().display()),
        )
        .args([
            "doctor",
            "--agent",
            "windsurf",
            "--root",
            root.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("windsurf_mcp_smoke"))
        .stdout(predicate::str::contains("windsurf_rewrite_support"));

    stop_daemon(&root, &home);
}
