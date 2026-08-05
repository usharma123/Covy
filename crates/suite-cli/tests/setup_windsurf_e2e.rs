#![cfg(unix)]

use assert_cmd::Command;
use serde_json::{json, Value};
use std::fs;
use std::sync::{Mutex, MutexGuard, OnceLock};
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn setup_windsurf_e2e_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
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
    assert_eq!(mcp_config["mcpServers"]["packet28"]["args"][1], ".");

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
