#[path = "support/mcp_proxy.rs"]
mod mcp_proxy;

use serde_json::json;
use std::fs;
use tempfile::TempDir;

use mcp_proxy::{
    ensure_packet28d_built, init_repo, read_mcp_message_for_id, start_mcp_proxy_server_with_tool,
    suite_cmd, write_mcp_message, write_repo_fixture,
};

#[test]
#[cfg(unix)]
fn test_mcp_proxy_cache_respects_timeout_ms() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let counter_path = dir.path().join("tools-list-count.txt");
    let script_path = dir.path().join("slow_mcp.py");
    fs::write(
        &script_path,
        format!(
            r#"import json, pathlib, sys, time

COUNTER = pathlib.Path({counter:?})

def read_message():
    headers = {{}}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    try:
        sys.stdout.buffer.write(f"Content-Length: {{len(body)}}\r\n\r\n".encode("utf-8"))
        sys.stdout.buffer.write(body)
        sys.stdout.buffer.flush()
    except BrokenPipeError:
        sys.exit(0)

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    if method == "initialize":
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"protocolVersion": "2024-11-05", "capabilities": {{"tools": {{}}, "resources": {{}}}}, "serverInfo": {{"name": "slow", "version": "1"}}}}}})
    elif method == "tools/list":
        count = 0
        if COUNTER.exists():
            count = int(COUNTER.read_text() or "0")
        COUNTER.write_text(str(count + 1))
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"tools": [{{"name": "slow.read", "description": "slow tool", "inputSchema": {{"type": "object", "properties": {{}}}}}}]}}}})
    elif method == "resources/list":
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"resources": []}}}})
    elif method == "resources/templates/list":
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"resourceTemplates": []}}}})
    elif method == "tools/call":
        time.sleep(0.4)
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"content": [{{"type": "text", "text": "slow ok"}}]}}}})
    else:
        write_message({{"jsonrpc": "2.0", "id": msg_id, "error": {{"code": -32601, "message": "unknown method"}}}})
"#,
            counter = counter_path,
        ),
    )
    .unwrap();

    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "slow": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "timeout_ms": 100
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut child, mut stdin, mut stdout, tools) = start_mcp_proxy_server_with_tool(
        dir.path(),
        &config_path,
        "task-proxy-timeout",
        "slow.read",
    );
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "slow.read"));
    let catalog_refresh_count = fs::read_to_string(&counter_path)
        .unwrap()
        .trim()
        .parse::<u64>()
        .unwrap();
    assert!(catalog_refresh_count >= 1);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":10,
            "method":"tools/call",
            "params":{
                "name":"slow.read",
                "arguments":{}
            }
        }),
    );
    let timeout = read_mcp_message_for_id(&mut stdout, 10);
    assert!(timeout["error"]["message"]
        .as_str()
        .unwrap()
        .contains("100ms"));
    assert!(timeout["error"]["message"]
        .as_str()
        .unwrap()
        .contains("python3 -u"));
    assert_eq!(
        fs::read_to_string(&counter_path)
            .unwrap()
            .trim()
            .parse::<u64>()
            .unwrap(),
        catalog_refresh_count
    );

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
