use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::OnceLock;
use std::time::Duration;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn ensure_packet28d_built() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let status = std::process::Command::new("cargo")
            .args(["build", "-p", "packet28d"])
            .status()
            .unwrap();
        assert!(status.success(), "failed to build packet28d");
    });
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

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed with {status}", args);
}

fn write_mcp_message(stdin: &mut ChildStdin, value: &Value) {
    let body = serde_json::to_vec(value).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.flush().unwrap();
}

fn read_mcp_message(stdout: &mut BufReader<ChildStdout>) -> Value {
    let mut content_length = None::<usize>;
    let mut line = String::new();
    loop {
        line.clear();
        stdout.read_line(&mut line).unwrap();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
    }
    let mut body = vec![0_u8; content_length.unwrap()];
    stdout.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn read_mcp_message_for_id(stdout: &mut BufReader<ChildStdout>, expected_id: u64) -> Value {
    loop {
        let value = read_mcp_message(stdout);
        if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
            return value;
        }
    }
}

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

fn initialize_mcp_session(stdin: &mut ChildStdin, stdout: &mut BufReader<ChildStdout>) {
    write_mcp_message(
        stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}
        }),
    );
    let _ = read_mcp_message_for_id(stdout, 1);
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
fn test_daemon_task_cli_await_handoff_reports_ready_status() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init"]);
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-await",
        "Prepare daemon-owned handoff wait",
    );

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-await",
            "--timeout-ms",
            "1000",
            "--poll-ms",
            "50",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["task_status"]["handoff_ready"], true);
    assert!(value["waited_ms"].as_u64().unwrap() <= 1_000);
    assert!(value["polls"].as_u64().unwrap() >= 1);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_daemon_task_cli_launch_agent_spawns_child_from_handoff() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init"]);
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-launch",
        "Launch fresh worker from daemon",
    );

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "launch-agent",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-launch",
            "--json",
            "--",
            "sh",
            "-c",
            "printf '%s\\n%s\\n' \"$PACKET28_BOOTSTRAP_MODE\" \"$PACKET28_TASK_ID\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launch_value: Value = serde_json::from_slice(&output).unwrap();
    let log_path = launch_value["log_path"].as_str().unwrap();
    assert_eq!(launch_value["bootstrap_mode"], "handoff");
    assert!(launch_value["pid"].as_u64().unwrap() > 0);

    let mut log_contents = String::new();
    for _ in 0..40 {
        if let Ok(raw) = fs::read_to_string(log_path) {
            log_contents = raw;
            if log_contents.contains("handoff") && log_contents.contains("task-daemon-launch") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(log_contents.contains("handoff"));
    assert!(log_contents.contains("task-daemon-launch"));

    let status_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-launch",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_value: Value = serde_json::from_slice(&status_output).unwrap();
    assert_eq!(status_value["latest_agent_bootstrap_mode"], "handoff");
    assert_eq!(
        status_value["latest_agent_pid"].as_u64().unwrap(),
        launch_value["pid"].as_u64().unwrap()
    );
    assert_eq!(status_value["latest_agent_log_path"], log_path);
    assert_eq!(
        status_value["latest_agent_handoff_artifact_id"],
        launch_value["handoff_artifact_id"]
    );
    assert_eq!(
        status_value["latest_agent_handoff_checkpoint_id"],
        launch_value["handoff_checkpoint_id"]
    );
    assert!(status_value["latest_agent_context_version"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_daemon_task_cli_await_handoff_can_require_newer_context_version() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init"]);
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-newer-handoff",
        "Prepare initial handoff",
    );
    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    let launch_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "launch-agent",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--json",
            "--",
            "sh",
            "-c",
            "printf '%s\\n' \"$PACKET28_BOOTSTRAP_MODE\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launch_value: Value = serde_json::from_slice(&launch_output).unwrap();
    let launched_context_version = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launched_status: Value = serde_json::from_slice(&launched_context_version).unwrap();
    let previous_context_version = launched_status["latest_agent_context_version"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(launch_value["bootstrap_mode"], "handoff");

    suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--after-context-version",
            &previous_context_version,
            "--timeout-ms",
            "100",
            "--poll-ms",
            "20",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "newer handoff than context version",
        ));

    let _ = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        4,
        "task-daemon-newer-handoff",
        "Resume from a newer handoff",
        "editing",
        &["src/beta.rs"],
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreCompact",
            "task_id":"task-daemon-newer-handoff",
            "session_id":"session-daemon-newer-handoff",
        }),
    );
    assert_eq!(status, 0);

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--after-context-version",
            &previous_context_version,
            "--timeout-ms",
            "1000",
            "--poll-ms",
            "50",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["task_status"]["handoff_ready"], true);
    assert_ne!(
        value["task_status"]["latest_context_version"]
            .as_str()
            .unwrap(),
        previous_context_version
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
