#![cfg(unix)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_setup_runtime_hooks_copilot_writes_instructions_and_pretool_hook() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "setup",
            "--root",
            root.path().to_str().unwrap(),
            "--runtime",
            "copilot",
            "--yes",
        ])
        .assert()
        .success();

    assert!(root
        .path()
        .join(".github")
        .join("copilot-instructions.md")
        .exists());
    let hook_path = root
        .path()
        .join(".github")
        .join("hooks")
        .join("packet28-rewrite.json");
    let settings: Value = serde_json::from_str(&fs::read_to_string(hook_path).unwrap()).unwrap();
    let hooks = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(hooks.len(), 1);
    let command = hooks[0]["command"].as_str().unwrap();
    assert!(command.contains(" hook copilot "));
    assert!(command.contains(root.path().to_str().unwrap()));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "doctor",
            "--root",
            root.path().to_str().unwrap(),
            "--agent",
            "copilot",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("copilot_hook_config"));
}

#[test]
fn test_setup_runtime_hooks_opencode_writes_instructions_and_rewrite_plugin() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "setup",
            "--root",
            root.path().to_str().unwrap(),
            "--runtime",
            "opencode",
            "--yes",
        ])
        .assert()
        .success();

    assert!(root.path().join("AGENTS.md").exists());
    let plugin_path = home
        .path()
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("packet28.ts");
    let plugin = fs::read_to_string(plugin_path).unwrap();
    assert!(plugin.contains("Packet28 rewrite"));
    assert!(plugin.contains("tool.execute.before"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "doctor",
            "--root",
            root.path().to_str().unwrap(),
            "--agent",
            "opencode",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("opencode_plugin"));
}

#[test]
fn test_setup_runtime_hooks_hermes_writes_instructions_plugin_and_config() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "setup",
            "--root",
            root.path().to_str().unwrap(),
            "--runtime",
            "hermes",
            "--yes",
        ])
        .assert()
        .success();

    assert!(root.path().join("AGENTS.md").exists());
    let plugin_dir = home
        .path()
        .join(".hermes")
        .join("plugins")
        .join("packet28-rewrite");
    let init = fs::read_to_string(plugin_dir.join("__init__.py")).unwrap();
    let manifest = fs::read_to_string(plugin_dir.join("plugin.yaml")).unwrap();
    let config = fs::read_to_string(home.path().join(".hermes").join("config.yaml")).unwrap();
    assert!(init.contains("Packet28 rewrite"));
    assert!(manifest.contains("packet28-rewrite"));
    assert!(config.contains("packet28-rewrite"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "doctor",
            "--root",
            root.path().to_str().unwrap(),
            "--agent",
            "hermes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("hermes_plugin"));
}

#[test]
fn test_setup_runtime_hooks_gemini_writes_before_tool_hook_and_prompt() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "setup",
            "--root",
            root.path().to_str().unwrap(),
            "--runtime",
            "gemini",
            "--yes",
        ])
        .assert()
        .success();

    assert!(root.path().join("GEMINI.md").exists());
    let settings_path = home.path().join(".gemini").join("settings.json");
    let settings: Value =
        serde_json::from_str(&fs::read_to_string(settings_path).unwrap()).unwrap();
    let hooks = settings["hooks"]["BeforeTool"].as_array().unwrap();
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0]["matcher"].as_str(), Some("run_shell_command"));
    let command = hooks[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(command.contains(" hook gemini "));
    assert!(command.contains(root.path().to_str().unwrap()));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "doctor",
            "--root",
            root.path().to_str().unwrap(),
            "--agent",
            "gemini",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("gemini_hook_config"))
        .stdout(predicate::str::contains("runtime_rewrite_support"));
}
