#[path = "support/mcp_native.rs"]
mod mcp_native;
#[path = "support/mcp_native_read.rs"]
mod mcp_native_read;

use mcp_native::{
    ensure_packet28d_built, init_repo, initialize_mcp_session, read_mcp_message_for_id,
    start_mcp_server, suite_cmd, write_mcp_message, write_repo_fixture,
};
use mcp_native_read::{
    git, run_claude_hook, write_cached_coverage_state, write_cached_testmap_state,
};
use serde_json::{json, Value};
use std::io::BufReader;
use std::process::{ChildStdin, ChildStdout};
use tempfile::TempDir;

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

#[test]
#[cfg(unix)]
fn test_mcp_native_read_auto_captures_regions() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    git(dir.path(), &["add", "src/alpha.rs", "src/beta.rs"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "init",
        ],
    );
    write_cached_coverage_state(dir.path());
    write_cached_testmap_state(dir.path());

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);
    let _ = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        2,
        "task-native-read",
        "Locate the Alpha definition",
        "investigating",
        &["src/alpha.rs"],
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PostToolUse",
            "task_id":"task-native-read",
            "session_id":"session-native-read",
            "tool_name":"Read",
            "tool_input":{"file_path":"src/alpha.rs","offset":4,"limit":1},
            "tool_response":{"content":"fn alpha() {}\nstruct Alpha;\n","symbols":["Alpha"],"regions":["src/alpha.rs:4-5"]}
        }),
    );
    assert_eq!(status, 0);
    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"Stop",
            "task_id":"task-native-read",
            "session_id":"session-native-read",
        }),
    );
    assert_eq!(status, 0);

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.prepare_handoff",
                "arguments":{
                    "task_id":"task-native-read",
                    "query":"Where is Alpha defined?",
                    "response_mode":"full"
                }
            }
        }),
    );
    let inspect = read_mcp_message_for_id(&mut stdout, 3);
    let inspect_payload = &inspect["result"]["structuredContent"]["context"];
    assert!(inspect["result"]["structuredContent"]["handoff_ready"]
        .as_bool()
        .unwrap());
    assert!(inspect_payload["recent_tool_invocations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["tool_name"] == "Read"
                && item["regions"].as_array().is_some_and(|regions| {
                    regions.iter().any(|region| region == "src/alpha.rs:4-5")
                })
        }));
    assert!(inspect_payload["discovered_paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item == "src/alpha.rs"));
    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
