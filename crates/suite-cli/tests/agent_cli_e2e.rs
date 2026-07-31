#[path = "support/agent_core.rs"]
mod agent_core;
#[path = "support/agent.rs"]
mod agent_support;
#[expect(
    dead_code,
    reason = "this integration binary exercises a focused subset of the shared harness"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use agent_core::{agent_cmd, ensure_packet28d_built, init_repo, suite_cmd, write_repo_fixture};
use agent_support::setup_changed_repo;

fn parse_broker_response(output: &[u8]) -> Value {
    let value: Value = serde_json::from_slice(output).unwrap();
    assert!(value.get("context_version").is_some());
    assert!(value.get("brief").is_some());
    value
}

#[test]
#[cfg(unix)]
fn test_agent_cli_prompt_outputs_all_supported_fragments() {
    for format in ["claude", "agents", "cursor"] {
        let output = suite_cmd()
            .args(["agent-prompt", "--format", format])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let rendered = String::from_utf8(output).unwrap();
        assert!(!rendered.trim().is_empty());
        assert!(rendered.contains("Packet28 mcp serve"));
        assert!(rendered.contains("packet28.write_intention"));
        assert!(rendered.contains("p28` instant grep"));
        assert!(rendered.contains("packet28.read_regions"));
        assert!(!rendered.contains("packet28.search"));
        assert!(rendered.to_ascii_lowercase().contains("handoff"));
        assert!(rendered
            .to_ascii_lowercase()
            .contains("fall back to direct file reads"));
    }
}

#[test]
fn test_agent_cli_prompt_root_is_reflected_in_command_example() {
    suite_cmd()
        .args(["agent-prompt", "--format", "claude", "--root", "repo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--root \"repo\""));
}

#[test]
#[cfg(unix)]
fn test_agent_cli_persists_bootstrap_and_exports_env() {
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let task_text = "trace Alpha";
    let env_dump = dir.path().join("env.txt");

    agent_cmd()
        .current_dir(dir.path())
        .args([
            "--task",
            task_text,
            "--",
            "sh",
            "-c",
            "printf '%s\\n%s\\n' \"$PACKET28_ROOT\" \"$PACKET28_BOOTSTRAP_PATH\" > \"$1\"",
            "sh",
            env_dump.to_str().unwrap(),
        ])
        .assert()
        .success();

    let persisted_path = dir
        .path()
        .join(".packet28")
        .join("agent")
        .join("latest-bootstrap.json");
    assert!(persisted_path.exists());

    let env_lines = fs::read_to_string(&env_dump)
        .unwrap()
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        PathBuf::from(&env_lines[0]).canonicalize().unwrap(),
        dir.path().canonicalize().unwrap()
    );
    assert_eq!(
        PathBuf::from(&env_lines[1]).canonicalize().unwrap(),
        persisted_path.canonicalize().unwrap()
    );

    let value = parse_broker_response(&fs::read(&persisted_path).unwrap());
    assert!(value["brief"]
        .as_str()
        .unwrap()
        .contains("fresh session bootstrap"));
    assert_eq!(value["response_mode"], "full");
}

#[test]
#[cfg(unix)]
fn test_agent_cli_bootstraps_broker_session() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    let task_text = "design auth broker";
    let task_id = suite_cli::broker_client::derive_task_id(task_text);

    let output = agent_cmd()
        .current_dir(dir.path())
        .args([
            "--task",
            task_text,
            "--",
            "sh",
            "-c",
            "printf '%s\n%s\n%s\n%s\n%s\n%s\n%s\n%s\n' \"$PACKET28_BOOTSTRAP_MODE\" \"$PACKET28_TASK_ID\" \"$PACKET28_BROKER_BRIEF_PATH\" \"$PACKET28_BROKER_STATE_PATH\" \"$PACKET28_MCP_COMMAND\" \"$PACKET28_BROKER_WINDOW_MODE\" \"$PACKET28_BROKER_SUPERSESSION\" \"$PACKET28_BROKER_PREPARE_HANDOFF_TOOL\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let lines = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 8);
    assert_eq!(lines[0], "fresh");
    assert_eq!(lines[1], task_id);
    assert!(Path::new(&lines[2]).exists(), "brief path should exist");
    assert!(Path::new(&lines[3]).exists(), "state path should exist");
    assert!(lines[4].contains("Packet28 mcp serve --root"));
    assert_eq!(lines[5], "replace");
    assert_eq!(lines[6], "1");
    assert_eq!(lines[7], "packet28.prepare_handoff");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_agent_cli_returns_child_exit_code() {
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let task_text = "trace Alpha";

    agent_cmd()
        .current_dir(dir.path())
        .args(["--task", task_text, "--", "sh", "-c", "exit 7"])
        .assert()
        .code(7);
}

#[test]
fn test_agent_cli_requires_child_command() {
    agent_cmd()
        .args(["--task", "review alpha.rs change"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("Usage:"));
}
