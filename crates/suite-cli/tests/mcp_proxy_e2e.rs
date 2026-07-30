#[expect(
    dead_code,
    reason = "shared lifecycle fixtures support native and proxy MCP test binaries"
)]
#[cfg(unix)]
#[path = "support/mcp_lifecycle.rs"]
mod mcp_lifecycle;
#[path = "support/mcp_proxy.rs"]
mod mcp_proxy;
#[path = "support/mcp_proxy_fake.rs"]
mod mcp_proxy_fake;
#[expect(
    dead_code,
    reason = "shared integration harness APIs are exercised by sibling test binaries"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use mcp_proxy_fake::{
    write_bidirectional_server, write_colliding_tool_server, write_compact_read_server,
    write_concurrent_tool_server, write_cyclic_resource_server, write_dynamic_resource_server,
    write_newline_only_server, write_paginated_resource_server, write_slow_initialize_server,
    write_upstream_batch_server,
};
use packet28_daemon_protocol::context_store::{ContextStoreGetRequest, ContextStoreListRequest};
use process_harness::McpHarness;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[cfg(unix)]
use mcp_lifecycle::{
    corrupt_task_event_log, large_response_batch, read_content_length_message,
    small_buffered_stdout_pair, wait_for_child, wait_for_file, wait_for_stdout_backpressure,
    write_content_length_message,
};
use mcp_proxy::{
    close_mcp_stdin, ensure_packet28d_built, init_repo, initialize_mcp_session,
    read_mcp_message_for_id, start_mcp_proxy_server, start_mcp_proxy_server_with_tool,
    stop_mcp_server, suite_cmd, write_mcp_message, write_repo_fixture,
};

const MCP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

fn read_next_mcp_response(server: &mut McpHarness) -> serde_json::Value {
    let deadline = Instant::now() + MCP_RESPONSE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for an MCP response after {MCP_RESPONSE_TIMEOUT:?}"
        );
        let value = server
            .receive(remaining)
            .unwrap_or_else(|error| panic!("failed to read MCP response: {error}"));
        if value.get("id").is_some() {
            return value;
        }
    }
}

fn read_until(
    server: &mut McpHarness,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = Instant::now() + MCP_RESPONSE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for matching MCP message after {MCP_RESPONSE_TIMEOUT:?}"
        );
        let value = server
            .receive(remaining)
            .unwrap_or_else(|error| panic!("failed to read MCP message: {error}"));
        if predicate(&value) {
            return value;
        }
    }
}

fn load_agent_state_events(root: &Path, task_id: &str) -> Vec<Value> {
    let root_string = root.to_string_lossy().into_owned();
    let entries = suite_cli::cmd_daemon::execute_context_store_list(
        root,
        ContextStoreListRequest {
            root: root_string.clone(),
            target: Some("agenty.state.write".to_string()),
            limit: 100,
            ..ContextStoreListRequest::default()
        },
    )
    .unwrap()
    .entries;

    entries
        .into_iter()
        .filter_map(|entry| {
            suite_cli::cmd_daemon::execute_context_store_get(
                root,
                ContextStoreGetRequest {
                    root: root_string.clone(),
                    key: entry.cache_key,
                },
            )
            .unwrap()
            .entry
        })
        .flat_map(|detail| detail.entry.packets)
        .filter_map(|packet| packet.body["payload"].as_object().cloned())
        .map(Value::Object)
        .filter(|event| event["task_id"] == task_id)
        .collect()
}

fn assert_failed_tool_lifecycle(
    events: &[Value],
    tool_name: &str,
    error_class: &str,
    retryable: bool,
) {
    let failed = events
        .iter()
        .find(|event| {
            event["data"]["type"] == "tool_invocation_failed"
                && event["data"]["tool_name"] == tool_name
        })
        .unwrap_or_else(|| panic!("missing failed lifecycle event for {tool_name}"));
    let invocation_id = failed["data"]["invocation_id"].as_str().unwrap();
    let started = events
        .iter()
        .find(|event| {
            event["data"]["type"] == "tool_invocation_started"
                && event["data"]["invocation_id"] == invocation_id
        })
        .unwrap_or_else(|| panic!("missing start event for failed invocation {invocation_id}"));

    assert_eq!(started["data"]["sequence"], failed["data"]["sequence"]);
    assert_eq!(started["data"]["tool_name"], failed["data"]["tool_name"]);
    assert_eq!(
        started["data"]["server_name"],
        failed["data"]["server_name"]
    );
    assert_eq!(
        started["data"]["request_fingerprint"],
        failed["data"]["request_fingerprint"]
    );
    assert_eq!(failed["data"]["error_class"], error_class);
    assert_eq!(failed["data"]["retryable"], retryable);
    assert!(
        failed["data"]["duration_ms"].as_u64().is_some(),
        "failed invocation must record duration_ms"
    );
    assert!(
        failed["data"]["error_message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "failed invocation must record a non-empty error message"
    );
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_defaults_upstream_initialize_tool_and_reverse_calls_to_newline_json() {
    use packet28_daemon_core::task_store_lease::try_acquire_task_store_retention_lease;

    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("newline_only_mcp.py");
    write_newline_only_server(&script_path);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "newline": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let mut server = start_mcp_proxy_server(dir.path(), &config_path, "task-proxy-newline");
    initialize_mcp_session(&mut server);

    write_mcp_message(
        &mut server,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    let tools = read_mcp_message_for_id(&mut server, 2);
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "newline.echo"));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{"name":"newline.echo","arguments":{}}
        }),
    );
    let roots_request = read_until(&mut server, |message| message["method"] == "roots/list");
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":roots_request["id"],
            "result":{"roots":[{"uri":"file:///tmp/repo","name":"repo"}]}
        }),
    );
    let tool_response = read_mcp_message_for_id(&mut server, 3);
    assert_eq!(
        tool_response["result"]["structuredContent"]["root_count"],
        1
    );

    write_mcp_message(
        &mut server,
        &json!([{
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{"name":"newline.echo","arguments":{}}
        }]),
    );
    let batched_roots_request =
        read_until(&mut server, |message| message["method"] == "roots/list");
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":batched_roots_request["id"],
            "result":{"roots":[]}
        }),
    );
    let batched_tool_response = read_until(&mut server, |message| {
        message
            .as_array()
            .is_some_and(|responses| responses.iter().any(|response| response["id"] == json!(4)))
    });
    assert_eq!(
        batched_tool_response[0]["result"]["structuredContent"]["root_count"],
        0
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
    let retention_deadline = Instant::now() + Duration::from_secs(5);
    let retention = loop {
        if let Some(retention) = try_acquire_task_store_retention_lease(dir.path()).unwrap() {
            break retention;
        }
        assert!(
            Instant::now() < retention_deadline,
            "idle MCP proxy session retained the task-store writer lease"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    drop(retention);

    write_mcp_message(
        &mut server,
        &json!({"jsonrpc":"2.0","id":5,"method":"prompts/list"}),
    );
    assert_eq!(read_mcp_message_for_id(&mut server, 5)["id"], 5);

    stop_mcp_server(server);
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_routes_server_requests_and_json_rpc_batches() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("bidirectional_mcp.py");
    write_bidirectional_server(&script_path);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "bidirectional": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "framing": "content_length"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let mut server = start_mcp_proxy_server(dir.path(), &config_path, "task-proxy-bidirectional");
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{
                "protocolVersion":"2025-03-26",
                "capabilities":{"roots":{}},
                "clientInfo":{"name":"bidirectional-test","version":"1"}
            }
        }),
    );
    let initialized = read_mcp_message_for_id(&mut server, 1);
    assert_eq!(initialized["result"]["protocolVersion"], "2025-03-26");

    write_mcp_message(
        &mut server,
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
    let roots_request = read_until(&mut server, |message| message["method"] == "roots/list");
    let proxy_request_id = roots_request["id"].clone();
    assert!(proxy_request_id
        .as_str()
        .is_some_and(|id| id.starts_with("packet28-upstream:bidirectional:")));
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":proxy_request_id,
            "result":{"roots":[{"uri":"file:///tmp/repo","name":"repo"}]}
        }),
    );
    let acknowledgement = read_until(&mut server, |message| {
        message["method"] == "notifications/message" && message["params"]["data"]["root_count"] == 1
    });
    assert_eq!(acknowledgement["params"]["upstream"], "bidirectional");

    write_mcp_message(
        &mut server,
        &json!([
            {"jsonrpc":"2.0","id":2,"method":"tools/list"},
            {"jsonrpc":"2.0","method":"notifications/initialized"},
            {"jsonrpc":"2.0","id":3,"method":"prompts/list"}
        ]),
    );
    let batch = read_until(&mut server, serde_json::Value::is_array);
    let batch = batch.as_array().unwrap();
    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0]["id"], 2);
    assert!(batch[0]["result"]["tools"].is_array());
    assert_eq!(batch[1]["id"], 3);
    assert!(batch[1]["result"]["prompts"].is_array());

    write_mcp_message(&mut server, &json!([]));
    let empty_batch = read_until(&mut server, |message| message["error"]["code"] == -32600);
    assert_eq!(empty_batch["id"], serde_json::Value::Null);

    stop_mcp_server(server);
    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_routes_mixed_upstream_batch_and_diagnoses_invalid_batches() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("upstream_batch_mcp.py");
    write_upstream_batch_server(&script_path);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "upstream-batch": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "framing": "content_length"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let mut server = start_mcp_proxy_server(dir.path(), &config_path, "task-proxy-upstream-batch");
    initialize_mcp_session(&mut server);
    write_mcp_message(
        &mut server,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );

    let deadline = Instant::now() + MCP_RESPONSE_TIMEOUT;
    let mut forwarded_batch = None;
    let mut tools_response = None;
    while forwarded_batch.is_none() || tools_response.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for mixed upstream batch routing"
        );
        let message = server
            .receive(remaining)
            .unwrap_or_else(|error| panic!("failed to read MCP message: {error}"));
        if message
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["method"] == "roots/list"))
        {
            forwarded_batch = Some(message);
        } else if message["id"] == 2 {
            tools_response = Some(message);
        }
    }

    assert!(tools_response.unwrap()["result"]["tools"].is_array());
    let forwarded_batch = forwarded_batch.unwrap();
    let forwarded = forwarded_batch.as_array().unwrap();
    assert_eq!(forwarded.len(), 2);
    assert_eq!(forwarded[0]["method"], "roots/list");
    assert!(forwarded[0]["id"]
        .as_str()
        .is_some_and(|id| id.starts_with("packet28-upstream:upstream-batch:")));
    assert_eq!(forwarded[1]["method"], "notifications/message");
    assert_eq!(forwarded[1]["params"]["data"]["kind"], "mixed");
    assert_eq!(forwarded[1]["params"]["upstream"], "upstream-batch");

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":forwarded[0]["id"],
            "result":{"roots":[]}
        }),
    );
    let reverse_array_diagnostic = read_until(&mut server, |message| {
        message["params"]["data"]["diagnostic"] == "reverse-array"
    });
    assert_eq!(
        reverse_array_diagnostic["params"]["upstream"],
        "upstream-batch"
    );
    assert_eq!(reverse_array_diagnostic["params"]["data"]["count"], 1);
    let empty_diagnostic = read_until(&mut server, |message| {
        message["params"]["data"]["diagnostic"] == "empty"
    });
    assert_eq!(empty_diagnostic["params"]["upstream"], "upstream-batch");
    assert_eq!(empty_diagnostic["params"]["data"]["count"], 1);
    let invalid_diagnostic = read_until(&mut server, |message| {
        message["params"]["data"]["diagnostic"] == "invalid"
    });
    assert_eq!(invalid_diagnostic["params"]["upstream"], "upstream-batch");
    assert_eq!(invalid_diagnostic["params"]["data"]["count"], 3);

    stop_mcp_server(server);
    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_cli_namespaces_colliding_tools() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_alpha = dir.path().join("alpha_mcp.py");
    write_colliding_tool_server(&script_alpha, "alpha");

    let script_beta = dir.path().join("beta_mcp.py");
    write_colliding_tool_server(&script_beta, "beta");

    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "alpha": {
                    "command": "python3",
                    "args": ["-u", script_alpha.to_str().unwrap()],
                    "framing": "content_length"
                },
                "beta": {
                    "command": "python3",
                    "args": ["-u", script_beta.to_str().unwrap()],
                    "framing": "content_length"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let mut server = start_mcp_proxy_server(dir.path(), &config_path, "task-proxy-collision");

    initialize_mcp_session(&mut server);

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/list"
        }),
    );
    let tools = read_mcp_message_for_id(&mut server, 2);
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "alpha.shared.read"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "beta.shared.read"));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"beta.shared.read",
                "arguments":{}
            }
        }),
    );
    let response = read_mcp_message_for_id(&mut server, 3);
    assert_eq!(
        response["result"]["structuredContent"]["owner"]
            .as_str()
            .unwrap(),
        "beta"
    );

    stop_mcp_server(server);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_unites_exact_and_template_resource_ownership() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_alpha = dir.path().join("alpha_resources.py");
    write_colliding_tool_server(&script_alpha, "alpha");
    let script_beta = dir.path().join("beta_resources.py");
    write_colliding_tool_server(&script_beta, "beta");
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "alpha": {
                    "command": "python3",
                    "args": ["-u", script_alpha.to_str().unwrap()],
                    "framing": "content_length"
                },
                "beta": {
                    "command": "python3",
                    "args": ["-u", script_beta.to_str().unwrap()],
                    "framing": "content_length"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let mut server =
        start_mcp_proxy_server(dir.path(), &config_path, "task-proxy-resource-routing");
    initialize_mcp_session(&mut server);

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"templated-read",
            "method":"resources/read",
            "params":{"uri":"alpha://items/42"}
        }),
    );
    let templated = read_until(&mut server, |message| message["id"] == "templated-read");
    assert_eq!(templated["id"], "templated-read");
    assert_eq!(templated["result"]["contents"][0]["text"], "alpha resource");

    for (id, uri) in [
        ("duplicate-exact", "shared://resource"),
        ("exact-template-union", "union://items/42"),
    ] {
        write_mcp_message(
            &mut server,
            &json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"resources/read",
                "params":{"uri":uri}
            }),
        );
        let duplicate = read_until(&mut server, |message| message["id"] == id);
        assert_eq!(duplicate["id"], id);
        assert_eq!(duplicate["error"]["code"], -32000);
        assert_eq!(
            duplicate["error"]["message"],
            format!("resource '{uri}' is advertised by multiple upstreams: alpha, beta")
        );
    }

    stop_mcp_server(server);
    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_exhausts_upstream_resource_pages_and_pages_stable_downstream_snapshot() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script = dir.path().join("paginated_resources.py");
    write_paginated_resource_server(&script);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "paginated": {
                    "command": "python3",
                    "args": ["-u", script.to_str().unwrap()],
                    "framing": "content_length"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let mut server =
        start_mcp_proxy_server(dir.path(), &config_path, "task-proxy-paginated-resources");
    initialize_mcp_session(&mut server);

    let mut cursor = None;
    let mut listed_uris = Vec::new();
    for page in 1..=2 {
        let id = format!("resources-page-{page}");
        let params = cursor
            .as_ref()
            .map_or_else(|| json!({}), |cursor| json!({"cursor":cursor}));
        write_mcp_message(
            &mut server,
            &json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"resources/list",
                "params":params
            }),
        );
        let response = read_until(&mut server, |message| message["id"] == id);
        assert_eq!(response["id"], id);
        listed_uris.extend(
            response["result"]["resources"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|resource| resource["uri"].as_str().map(str::to_string)),
        );
        cursor = response["result"]["nextCursor"]
            .as_str()
            .map(str::to_string);
    }
    assert!(cursor.is_none(), "second downstream page must be terminal");
    let unique_uris = listed_uris.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(unique_uris.len(), listed_uris.len());
    assert!(unique_uris.contains("paged://static/299"));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"templates",
            "method":"resources/templates/list",
            "params":{}
        }),
    );
    let templates = read_until(&mut server, |message| message["id"] == "templates");
    assert!(templates["result"]["resourceTemplates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["uriTemplate"] == "paged://other/{id}"));

    for (id, uri) in [
        ("read-page-two-static", "paged://static/299"),
        ("read-page-two-template", "paged://other/42"),
    ] {
        write_mcp_message(
            &mut server,
            &json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"resources/read",
                "params":{"uri":uri}
            }),
        );
        let response = read_until(&mut server, |message| message["id"] == id);
        assert_eq!(response["id"], id);
        assert_eq!(
            response["result"]["contents"][0]["text"],
            "paginated resource"
        );
    }

    stop_mcp_server(server);
    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_invalidates_dynamic_resource_routes_and_rejects_unadvertised_subscriptions() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script = dir.path().join("dynamic_resources.py");
    write_dynamic_resource_server(&script);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "dynamic": {
                    "command": "python3",
                    "args": ["-u", script.to_str().unwrap()],
                    "framing": "content_length"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let mut server =
        start_mcp_proxy_server(dir.path(), &config_path, "task-proxy-dynamic-resources");
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"initialize-dynamic",
            "method":"initialize",
            "params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1"}}
        }),
    );
    let initialized = read_until(&mut server, |message| message["id"] == "initialize-dynamic");
    assert_eq!(
        initialized["result"]["capabilities"]["resources"]["listChanged"],
        true
    );
    assert!(
        initialized["result"]["capabilities"]["resources"]
            .get("subscribe")
            .is_none(),
        "proxy must not advertise subscriptions it does not route"
    );

    for (id, method) in [
        (json!("subscribe-original-id"), "resources/subscribe"),
        (json!(41), "resources/unsubscribe"),
    ] {
        write_mcp_message(
            &mut server,
            &json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":method,
                "params":{"uri":"dynamic://old"}
            }),
        );
        let response = read_until(&mut server, |message| message["id"] == id);
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], -32000);
        assert_eq!(
            response["error"]["message"],
            format!(
                "MCP proxy does not advertise resource subscriptions; method '{method}' is unsupported"
            )
        );
        assert!(response.get("result").is_none());
    }

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"list-old",
            "method":"resources/list",
            "params":{}
        }),
    );
    let old_catalog = read_until(&mut server, |message| message["id"] == "list-old");
    assert!(old_catalog["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|resource| resource["uri"] == "dynamic://old"));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"read-old",
            "method":"resources/read",
            "params":{"uri":"dynamic://old"}
        }),
    );
    let mut old_read = None;
    let mut saw_list_changed = false;
    let deadline = Instant::now() + MCP_RESPONSE_TIMEOUT;
    while old_read.is_none() || !saw_list_changed {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for old read response and list_changed notification"
        );
        let message = server
            .receive(remaining)
            .unwrap_or_else(|error| panic!("failed to read dynamic-resource message: {error}"));
        if message["id"] == "read-old" {
            old_read = Some(message);
        } else if message["method"] == "notifications/resources/list_changed" {
            assert_eq!(message["params"]["upstream"], "dynamic");
            saw_list_changed = true;
        }
    }
    assert_eq!(
        old_read.unwrap()["result"]["contents"][0]["text"],
        "dynamic://old"
    );

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"list-new",
            "method":"resources/list",
            "params":{}
        }),
    );
    let new_catalog = read_until(&mut server, |message| message["id"] == "list-new");
    assert!(new_catalog["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|resource| resource["uri"] == "dynamic://new"));
    assert!(!new_catalog["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|resource| resource["uri"] == "dynamic://old"));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"read-new",
            "method":"resources/read",
            "params":{"uri":"dynamic://new"}
        }),
    );
    let new_read = read_until(&mut server, |message| message["id"] == "read-new");
    assert_eq!(new_read["id"], "read-new");
    assert_eq!(new_read["result"]["contents"][0]["text"], "dynamic://new");

    stop_mcp_server(server);
    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_rejects_upstream_resource_cursor_cycles_with_original_id() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script = dir.path().join("cyclic_resources.py");
    write_cyclic_resource_server(&script);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "cyclic": {
                    "command": "python3",
                    "args": ["-u", script.to_str().unwrap()],
                    "framing": "content_length"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let mut server =
        start_mcp_proxy_server(dir.path(), &config_path, "task-proxy-cyclic-resources");
    initialize_mcp_session(&mut server);
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"cyclic-list",
            "method":"resources/list",
            "params":{}
        }),
    );

    let response = read_until(&mut server, |message| message["id"] == "cyclic-list");
    assert_eq!(response["id"], "cyclic-list");
    assert!(response["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("repeated cursor \"loop\"")));

    stop_mcp_server(server);
    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_cli_compacts_allowlisted_read_tool_results() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("compact_mcp.py");
    write_compact_read_server(&script_path);

    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "compact": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "framing": "content_length",
                    "compact_tools": ["compact.read"]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut server, tools) = start_mcp_proxy_server_with_tool(
        dir.path(),
        &config_path,
        "task-proxy-compact",
        "compact.read",
    );
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "compact.read"));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"compact.read",
                "arguments":{}
            }
        }),
    );
    let compact = read_mcp_message_for_id(&mut server, 2);
    let compact_payload = &compact["result"]["structuredContent"];
    assert_eq!(compact_payload["response_mode"], "slim");
    assert_eq!(compact_payload["original_tool"], "compact.read");
    assert!(compact_payload["artifact_id"].as_str().is_some());
    let artifact_id = compact_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.fetch_tool_result",
                "arguments":{
                    "task_id":"task-proxy-compact",
                    "artifact_id": artifact_id
                }
            }
        }),
    );
    let fetched = read_mcp_message_for_id(&mut server, 3);
    let fetched_payload = &fetched["result"]["structuredContent"];
    assert_eq!(fetched_payload["structuredContent"]["path"], "src/alpha.rs");
    assert!(fetched_payload["structuredContent"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line == "pub struct Alpha;"));

    stop_mcp_server(server);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_routes_concurrent_and_late_responses_by_id() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("concurrent_mcp.py");
    write_concurrent_tool_server(&script_path);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "concurrent": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "framing": "content_length",
                    "timeout_ms": 500
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut server, _) = start_mcp_proxy_server_with_tool(
        dir.path(),
        &config_path,
        "task-proxy-concurrent",
        "concurrent.echo",
    );
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"slow",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"barrier":true,"delay_ms":100,"value":"slow"}
            }
        }),
    );
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"fast",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"barrier":true,"delay_ms":5,"value":"fast"}
            }
        }),
    );

    let first = read_next_mcp_response(&mut server);
    let second = read_next_mcp_response(&mut server);
    assert_eq!(first["id"], "fast");
    assert_eq!(first["result"]["structuredContent"]["value"], "fast");
    assert_eq!(second["id"], "slow");
    assert_eq!(second["result"]["structuredContent"]["value"], "slow");

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"will-time-out",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"delay_ms":800,"value":"late"}
            }
        }),
    );
    let timeout = read_next_mcp_response(&mut server);
    assert_eq!(timeout["id"], "will-time-out");
    assert!(timeout["error"]["message"]
        .as_str()
        .unwrap()
        .contains("500ms"));
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"timeout-status",
            "method":"tools/call",
            "params":{
                "name":"packet28.task_status",
                "arguments":{"task_id":"task-proxy-concurrent"}
            }
        }),
    );
    let status = read_until(&mut server, |message| message["id"] == "timeout-status");
    assert_eq!(
        status["result"]["structuredContent"]["latest_context_reason"],
        "state_write:tool_invocation_failed"
    );
    assert_eq!(
        status["result"]["structuredContent"]["task"]["last_event_seq"],
        6
    );
    let events = load_agent_state_events(dir.path(), "task-proxy-concurrent");
    assert_failed_tool_lifecycle(&events, "concurrent.echo", "timeout", true);

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"after-timeout",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"delay_ms":5,"value":"not-poisoned"}
            }
        }),
    );
    let after_timeout = read_next_mcp_response(&mut server);
    assert_eq!(after_timeout["id"], "after-timeout");
    assert_eq!(
        after_timeout["result"]["structuredContent"]["value"],
        "not-poisoned"
    );

    std::thread::sleep(std::time::Duration::from_millis(350));
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"after-late",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"delay_ms":5,"value":"still-correct"}
            }
        }),
    );
    let after_late = read_next_mcp_response(&mut server);
    assert_eq!(after_late["id"], "after-late");
    assert_eq!(
        after_late["result"]["structuredContent"]["value"],
        "still-correct"
    );

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"half-close",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"delay_ms":50,"value":"drained-before-shutdown"}
            }
        }),
    );
    close_mcp_stdin(&mut server);
    let drained = read_next_mcp_response(&mut server);
    assert_eq!(drained["id"], "half-close");
    assert_eq!(
        drained["result"]["structuredContent"]["value"],
        "drained-before-shutdown"
    );
    stop_mcp_server(server);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_records_failed_terminal_state_when_upstream_disconnects() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("disconnecting_mcp.py");
    write_concurrent_tool_server(&script_path);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "concurrent": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "framing": "content_length",
                    "timeout_ms": 5_000
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut server, _) = start_mcp_proxy_server_with_tool(
        dir.path(),
        &config_path,
        "task-proxy-disconnect",
        "concurrent.echo",
    );
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"will-disconnect",
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{"exit":true}
            }
        }),
    );
    let disconnected = read_until(&mut server, |message| message["id"] == "will-disconnect");
    assert!(disconnected["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("exited")));

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"disconnect-status",
            "method":"tools/call",
            "params":{
                "name":"packet28.task_status",
                "arguments":{"task_id":"task-proxy-disconnect"}
            }
        }),
    );
    let status = read_until(&mut server, |message| message["id"] == "disconnect-status");
    assert_eq!(
        status["result"]["structuredContent"]["latest_context_reason"],
        "state_write:tool_invocation_failed"
    );
    assert_eq!(
        status["result"]["structuredContent"]["task"]["last_event_seq"],
        2
    );
    let events = load_agent_state_events(dir.path(), "task-proxy-disconnect");
    assert_failed_tool_lifecycle(&events, "concurrent.echo", "generic", false);

    stop_mcp_server(server);
    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_overload_preserves_single_and_batch_request_ids() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("saturated_mcp.py");
    write_concurrent_tool_server(&script_path);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "concurrent": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "framing": "content_length",
                    "timeout_ms": 5_000
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut server, _) = start_mcp_proxy_server_with_tool(
        dir.path(),
        &config_path,
        "task-proxy-saturated",
        "concurrent.echo",
    );
    for request_id in 0..64 {
        write_mcp_message(
            &mut server,
            &json!({
                "jsonrpc":"2.0",
                "id":format!("occupied-{request_id}"),
                "method":"tools/call",
                "params":{
                    "name":"concurrent.echo",
                    "arguments":{"delay_ms":1_500,"value":"occupied"}
                }
            }),
        );
    }

    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":"saturated-single",
            "method":"prompts/list"
        }),
    );
    let single = read_until(&mut server, |message| message["id"] == "saturated-single");
    assert_eq!(single["error"]["code"], -32000);

    write_mcp_message(
        &mut server,
        &json!([
            {"jsonrpc":"2.0","id":"batch-string","method":"prompts/list"},
            {"jsonrpc":"2.0","method":"notifications/initialized"},
            {"jsonrpc":"2.0","id":7,"method":"prompts/list"},
            {"jsonrpc":"2.0","id":"client-response","result":{}}
        ]),
    );
    let batch = read_until(&mut server, |message| {
        message.as_array().is_some_and(|responses| {
            responses
                .first()
                .is_some_and(|response| response["id"] == "batch-string")
        })
    });
    let responses = batch.as_array().unwrap();
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["id"], "batch-string");
    assert_eq!(responses[1]["id"], 7);
    assert!(responses
        .iter()
        .all(|response| response["error"]["code"] == -32000));

    let mut completed = [false; 64];
    for _ in 0..completed.len() {
        let response = read_next_mcp_response(&mut server);
        let request_id = response["id"]
            .as_str()
            .and_then(|id| id.strip_prefix("occupied-"))
            .and_then(|index| index.parse::<usize>().ok())
            .filter(|index| *index < completed.len())
            .unwrap_or_else(|| panic!("unexpected saturated-request response: {response}"));
        assert!(
            !completed[request_id],
            "duplicate saturated-request response for occupied-{request_id}"
        );
        completed[request_id] = true;
    }

    stop_mcp_server(server);
    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_exits_when_poller_fails_during_upstream_initialize() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let marker = dir.path().join("upstream-initialize-started");
    let script_path = dir.path().join("slow_initialize_mcp.py");
    write_slow_initialize_server(&script_path);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "slow-init": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "framing": "content_length",
                    "env": {
                        "P28_TEST_INIT_MARKER": marker.to_str().unwrap()
                    }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let task_id = "task-proxy-poller-failed-during-init";
    let mut server = start_mcp_proxy_server(dir.path(), &config_path, task_id);
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{
                "protocolVersion":"2025-03-26",
                "capabilities":{},
                "clientInfo":{"name":"poller-init-test","version":"1"}
            }
        }),
    );
    wait_for_file(&marker, MCP_RESPONSE_TIMEOUT);
    corrupt_task_event_log(dir.path(), task_id);

    let output = server
        .wait(Duration::from_secs(4))
        .expect("proxy ignored poller failure while upstream initialize was blocked");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MCP notification event-log read failed"),
        "unexpected proxy failure: {stderr}"
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_exits_when_poller_fails_during_upstream_tool_call() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let marker = dir.path().join("upstream-tool-started");
    let script_path = dir.path().join("slow_tool_mcp.py");
    write_concurrent_tool_server(&script_path);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "concurrent": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "framing": "content_length"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let task_id = "task-proxy-poller-failed-during-tool";
    let mut server = start_mcp_proxy_server(dir.path(), &config_path, task_id);
    initialize_mcp_session(&mut server);
    write_mcp_message(
        &mut server,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"concurrent.echo",
                "arguments":{
                    "delay_ms":30_000,
                    "started_path":marker.to_str().unwrap(),
                    "value":"must be cancelled"
                }
            }
        }),
    );
    wait_for_file(&marker, MCP_RESPONSE_TIMEOUT);
    corrupt_task_event_log(dir.path(), task_id);

    let output = server
        .wait(Duration::from_secs(4))
        .expect("proxy ignored poller failure while an upstream tool call was active");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MCP notification event-log read failed"),
        "unexpected proxy failure: {stderr}"
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_mcp_proxy_fatal_poller_cleanup_aborts_backpressured_stdout() {
    use std::io::{BufReader, BufWriter, Read as _};
    use std::os::fd::OwnedFd;
    use std::process::{Command, Stdio};

    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("backpressured_stdout_mcp.py");
    write_concurrent_tool_server(&script_path);
    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "concurrent": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "framing": "content_length"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let task_id = "task-proxy-poller-failed-with-blocked-stdout";
    let (child_stdout, parent_stdout) = small_buffered_stdout_pair();
    let child_stdout: OwnedFd = child_stdout.into();
    let mut command = Command::new(env!("CARGO_BIN_EXE_Packet28"));
    command
        .current_dir(dir.path())
        .args([
            "mcp",
            "proxy",
            "--root",
            dir.path().to_str().unwrap(),
            "--upstream-config",
            config_path.to_str().unwrap(),
            "--task-id",
            task_id,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::from(child_stdout))
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut stdin = BufWriter::new(child.stdin.take().unwrap());
    let mut stdout = BufReader::new(parent_stdout);

    write_content_length_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{
                "protocolVersion":"2025-03-26",
                "capabilities":{},
                "clientInfo":{"name":"stdout-backpressure-test","version":"1"}
            }
        }),
    );
    assert_eq!(read_content_length_message(&mut stdout)["id"], 1);

    let batch = large_response_batch();
    let response_lower_bound = write_content_length_message(&mut stdin, &batch);
    wait_for_stdout_backpressure(
        stdout.get_ref(),
        response_lower_bound,
        Duration::from_secs(3),
    );
    assert!(
        child.try_wait().unwrap().is_none(),
        "proxy exited before the poller failure was injected"
    );
    corrupt_task_event_log(dir.path(), task_id);

    let status = wait_for_child(&mut child, Duration::from_secs(4));
    assert!(!status.success());
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        stderr.contains("MCP notification event-log read failed"),
        "unexpected proxy failure: {stderr}"
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
