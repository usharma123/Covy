#![cfg(unix)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn setup_windsurf_e2e_lock() -> MutexGuard<'static, ()> {
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

#[test]
fn test_setup_windsurf_writes_rules_hooks_and_mcp() {
    let _guard = setup_windsurf_e2e_lock();
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join(".codeium").join("windsurf")).unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
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

    assert!(root.path().join(".windsurf").join("hooks.json").exists());
    assert!(root
        .path()
        .join(".windsurf")
        .join("rules")
        .join("packet28.md")
        .exists());
    assert!(home
        .path()
        .join(".codeium")
        .join("windsurf")
        .join("mcp_config.json")
        .exists());
    let rules = fs::read_to_string(
        root.path()
            .join(".windsurf")
            .join("rules")
            .join("packet28.md"),
    )
    .unwrap();
    assert!(rules.contains("Windsurf command rewrite is not guaranteed"));
}

#[test]
fn test_setup_windsurf_preserves_existing_mcp_servers_and_hooks() {
    let _guard = setup_windsurf_e2e_lock();
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let windsurf_home = home.path().join(".codeium").join("windsurf");
    fs::create_dir_all(&windsurf_home).unwrap();
    fs::create_dir_all(root.path().join(".windsurf")).unwrap();

    let mcp_config_path = windsurf_home.join("mcp_config.json");
    fs::write(
        &mcp_config_path,
        serde_json::to_string_pretty(&json!({
            "mcpServers": {
                "existing": {
                    "command": "existing-mcp",
                    "args": ["--flag"]
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let hooks_path = root.path().join(".windsurf").join("hooks.json");
    fs::write(
        &hooks_path,
        serde_json::to_string_pretty(&json!({
            "hooks": {
                "pre_run_command": [
                    {"command": "existing-pre-run"}
                ],
                "custom_event": [
                    {"command": "existing-custom"}
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
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

    let mcp_config: Value =
        serde_json::from_str(&fs::read_to_string(mcp_config_path).unwrap()).unwrap();
    assert_eq!(
        mcp_config["mcpServers"]["existing"]["command"],
        "existing-mcp"
    );
    assert_eq!(mcp_config["mcpServers"]["existing"]["args"][0], "--flag");
    assert_eq!(mcp_config["mcpServers"]["packet28"]["args"][0], "--root");

    let hooks: Value = serde_json::from_str(&fs::read_to_string(hooks_path).unwrap()).unwrap();
    let pre_run = hooks["hooks"]["pre_run_command"].as_array().unwrap();
    assert!(pre_run
        .iter()
        .any(|entry| entry["command"] == "existing-pre-run"));
    assert!(pre_run.iter().any(|entry| entry["command"]
        .as_str()
        .is_some_and(|command| command.contains("hook windsurf"))));
    assert_eq!(
        hooks["hooks"]["custom_event"][0]["command"],
        "existing-custom"
    );
}

#[test]
fn test_setup_windsurf_generated_mcp_config_smoke_test() {
    let _guard = setup_windsurf_e2e_lock();
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let bin_dir = TempDir::new().unwrap();
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

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn test_setup_windsurf_doctor_passes_with_generated_mcp_config() {
    let _guard = setup_windsurf_e2e_lock();
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let bin_dir = TempDir::new().unwrap();
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

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
