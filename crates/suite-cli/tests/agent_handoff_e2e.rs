#[path = "support/agent_core.rs"]
mod agent_core;
#[path = "support/mcp_io.rs"]
mod mcp_io;

use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use tempfile::TempDir;

use agent_core::{agent_cmd, ensure_packet28d_built, init_repo, suite_cmd, write_repo_fixture};
use mcp_io::{initialize_mcp_session, read_mcp_message_for_id, write_mcp_message};

fn start_mcp_server(root: &Path) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
        .current_dir(root)
        .args(["mcp", "serve", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (child, stdin, stdout)
}

fn write_intention_via_mcp(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    id: u64,
    task_id: &str,
    text: &str,
    step_id: &str,
    paths: &[&str],
) -> Value {
    write_mcp_message(
        stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":"tools/call",
            "params":{
                "name":"packet28.write_intention",
                "arguments":{
                    "task_id":task_id,
                    "text":text,
                    "step_id":step_id,
                    "paths":paths,
                }
            }
        }),
    );
    read_mcp_message_for_id(stdout, id)
}

fn run_claude_hook(root: &Path, payload: &Value) -> (i32, String) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
        .current_dir(root)
        .args(["hook", "claude", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(serde_json::to_string(payload).unwrap().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).to_string(),
    )
}

#[cfg(unix)]
fn seed_checkpointed_handoff_task(dir: &Path, task_id: &str, intention_text: &str) {
    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir);
    initialize_mcp_session(&mut stdin, &mut stdout);
    let _ = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        2,
        task_id,
        intention_text,
        "investigating",
        &["src/alpha.rs"],
    );
    let _ = child.kill();
    let _ = child.wait();
    let (status, _) = run_claude_hook(
        dir,
        &json!({
            "hook_event_name":"Stop",
            "task_id":task_id,
            "session_id": format!("session-{task_id}"),
        }),
    );
    assert_eq!(status, 0);
}

#[test]
#[cfg(unix)]
fn test_agent_handoff_resumes_from_checkpoint() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-handoff-agent",
        "Resume from checkpointed Alpha investigation",
    );

    let output = agent_cmd()
        .current_dir(dir.path())
        .args([
            "--wait-for-handoff",
            "--handoff-timeout-secs",
            "5",
            "--task-id",
            "task-handoff-agent",
            "--",
            "sh",
            "-c",
            "printf '%s\n%s\n%s\n%s\n%s\n%s\n' \"$PACKET28_BOOTSTRAP_MODE\" \"$PACKET28_BOOTSTRAP_PATH\" \"$PACKET28_HANDOFF_PATH\" \"$PACKET28_HANDOFF_ARTIFACT_ID\" \"$PACKET28_HANDOFF_CHECKPOINT_ID\" \"$PACKET28_BROKER_PREPARE_HANDOFF_TOOL\"",
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
    assert_eq!(lines.len(), 6);
    assert_eq!(lines[0], "handoff");
    assert!(Path::new(&lines[1]).exists(), "bootstrap path should exist");
    assert!(Path::new(&lines[2]).exists(), "handoff path should exist");
    assert!(
        !lines[3].is_empty(),
        "handoff artifact id should be exported"
    );
    assert!(lines[4].is_empty());
    assert_eq!(lines[5], "packet28.prepare_handoff");

    let bootstrap: Value = serde_json::from_str(&fs::read_to_string(&lines[1]).unwrap()).unwrap();
    assert_eq!(
        bootstrap["latest_intention"]["text"],
        "Resume from checkpointed Alpha investigation"
    );
    assert_eq!(bootstrap["response_mode"], "full");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_agent_handoff_wait_times_out_when_checkpoint_missing() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    agent_cmd()
        .current_dir(dir.path())
        .args([
            "--wait-for-handoff",
            "--handoff-timeout-secs",
            "1",
            "--handoff-poll-ms",
            "50",
            "--task-id",
            "task-timeout-handoff",
            "--",
            "sh",
            "-c",
            "exit 0",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "timed out waiting for Packet28 handoff",
        ));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
