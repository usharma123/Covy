#[path = "support/mcp_native.rs"]
mod mcp_native;
#[expect(
    dead_code,
    reason = "this integration binary exercises a focused subset of the shared harness"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use mcp_native::{
    ensure_packet28d_built, init_repo, initialize_mcp_session, read_mcp_message_for_id,
    start_mcp_server, stop_mcp_server, suite_cmd, write_mcp_message, write_repo_fixture,
};
use packet28_daemon_protocol::paths::{task_artifact_dir, TaskStorageId};
use serde_json::json;
use std::fs;
use tempfile::TempDir;

#[test]
#[cfg(unix)]
fn test_mcp_native_artifact_tools_return_slim_results_and_fetch_full_artifacts() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let mut server = start_mcp_server(dir.path());
    initialize_mcp_session(&mut server);

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"packet28_search",
                "arguments":{
                    "task_id":"task-native-tools",
                    "query":"Alpha",
                    "response_mode":"slim"
                }
            }
        }),
    );
    let search = read_mcp_message_for_id(&mut server, 2);
    let search_payload = &search["result"]["structuredContent"];
    assert_eq!(
        search_payload["response_mode"], "slim",
        "unexpected search response: {search:#}"
    );
    assert!(search_payload["artifact_id"].as_str().is_some());
    assert!(search_payload["match_count"].as_u64().unwrap() >= 1);
    assert_eq!(search_payload["search_strategy"], "hybrid");
    assert!(search_payload["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == "src/alpha.rs"));
    assert!(search_payload["regions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|region| region
            .as_str()
            .is_some_and(|value| value.starts_with("src/alpha.rs:"))));
    assert!(search_payload["engine"].is_object());
    assert!(search_payload["hybrid"].is_object());
    let search_artifact = search_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28_fetch_tool_result",
                "arguments":{
                    "task_id":"task-native-tools",
                    "artifact_id": search_artifact
                }
            }
        }),
    );
    let search_full = read_mcp_message_for_id(&mut server, 3);
    let search_full_payload = &search_full["result"]["structuredContent"];
    assert_eq!(search_full_payload["response_mode"], "full");
    assert_eq!(search_full_payload["query"], "Alpha");
    assert_eq!(search_full_payload["search_strategy"], "hybrid");
    assert_eq!(search_full_payload["content_format"], "path:line:text");
    assert!(search_full_payload["groups"].is_null());
    assert!(search_full_payload["content"]
        .as_str()
        .is_some_and(|content| content.contains("src/alpha.rs:")));
    assert!(search_full_payload["engine"].is_object());
    assert!(search_full_payload["hybrid"].is_object());

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28_read_regions",
                "arguments":{
                    "task_id":"task-native-tools",
                    "path":"src/alpha.rs",
                    "line_start":1,
                    "line_end":2,
                    "response_mode":"slim"
                }
            }
        }),
    );
    let read_regions = read_mcp_message_for_id(&mut server, 4);
    let read_payload = &read_regions["result"]["structuredContent"];
    assert_eq!(read_payload["response_mode"], "slim");
    assert!(read_payload["artifact_id"].as_str().is_some());
    let read_artifact = read_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28_fetch_tool_result",
                "arguments":{
                    "task_id":"task-native-tools",
                    "artifact_id": read_artifact
                }
            }
        }),
    );
    let read_full = read_mcp_message_for_id(&mut server, 5);
    let read_full_payload = &read_full["result"]["structuredContent"];
    assert_eq!(read_full_payload["response_mode"], "full");
    assert_eq!(read_full_payload["path"], "src/alpha.rs");
    assert_eq!(read_full_payload["line_count"], 2);
    assert!(read_full_payload["content"]
        .as_str()
        .is_some_and(|content| content.contains("2: use crate::beta::Beta;")));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"packet28_glob",
                "arguments":{
                    "task_id":"task-native-tools",
                    "pattern":"src/*.rs",
                    "response_mode":"slim"
                }
            }
        }),
    );
    let glob = read_mcp_message_for_id(&mut server, 6);
    let glob_payload = &glob["result"]["structuredContent"];
    assert_eq!(glob_payload["response_mode"], "slim");
    assert!(glob_payload["artifact_id"].as_str().is_some());
    let glob_artifact = glob_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":7,
            "method":"tools/call",
            "params":{
                "name":"packet28_fetch_tool_result",
                "arguments":{
                    "task_id":"task-native-tools",
                    "artifact_id": glob_artifact
                }
            }
        }),
    );
    let glob_full = read_mcp_message_for_id(&mut server, 7);
    let glob_full_payload = &glob_full["result"]["structuredContent"];
    assert_eq!(glob_full_payload["response_mode"], "full");
    assert_eq!(glob_full_payload["pattern"], "src/*.rs");
    assert!(glob_full_payload["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == "src/alpha.rs"));

    stop_mcp_server(server);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_native_artifact_admission_failure_creates_no_evidence() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    let task_id = "task-native-artifact-rejected";
    let task_storage_id = TaskStorageId::try_from(task_id).unwrap();
    let task_dir = task_artifact_dir(dir.path(), &task_storage_id);
    fs::create_dir_all(&task_dir).unwrap();

    let mut server = start_mcp_server(dir.path());
    initialize_mcp_session(&mut server);
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"packet28_search",
                "arguments":{
                    "task_id":task_id,
                    "query":"Alpha",
                    "response_mode":"slim"
                }
            }
        }),
    );
    let response = read_mcp_message_for_id(&mut server, 2);

    stop_mcp_server(server);
    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    assert!(response["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("cannot adopt pre-existing managed entry")));
    assert!(
        fs::read_dir(task_dir).unwrap().next().is_none(),
        "admission failure must not create tool evidence"
    );
}
