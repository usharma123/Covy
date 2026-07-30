use std::fs;
use std::path::Path;

pub fn write_colliding_tool_server(path: &Path, owner: &str) {
    let script = r#"import json, sys

def read_message():
    headers = {}
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
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    if method == "initialize":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}, "resources": {}}, "serverInfo": {"name": "__OWNER__", "version": "1"}}})
    elif method == "tools/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": [{"name": "shared.read", "description": "__OWNER__ shared tool", "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "resources/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resources": []}})
    elif method == "resources/templates/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resourceTemplates": []}})
    elif method == "tools/call":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"content": [{"type": "text", "text": "__OWNER__ ok"}], "structuredContent": {"owner": "__OWNER__"}}})
    else:
        write_message({"jsonrpc": "2.0", "id": msg_id, "error": {"code": -32601, "message": "unknown method"}})
"#;
    fs::write(path, script.replace("__OWNER__", owner)).unwrap();
}

pub fn write_compact_read_server(path: &Path) {
    fs::write(
        path,
        r#"import json, sys

def read_message():
    headers = {}
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
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    if method == "initialize":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}, "resources": {}}, "serverInfo": {"name": "compact", "version": "1"}}})
    elif method == "tools/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": [{"name": "compact.read", "description": "compact test tool", "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "resources/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resources": []}})
    elif method == "resources/templates/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resourceTemplates": []}})
    elif method == "tools/call":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"content": [{"type": "text", "text": "Alpha content line 1\nAlpha content line 2"}], "structuredContent": {"path": "src/alpha.rs", "lines": ["pub struct Alpha;", "impl Alpha {}"], "notes": "verbose upstream payload"}}})
    else:
        write_message({"jsonrpc": "2.0", "id": msg_id, "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .unwrap();
}

pub fn write_concurrent_tool_server(path: &Path) {
    fs::write(
        path,
        r#"import json, os, sys, threading, time

WRITE_LOCK = threading.Lock()
RACE_BARRIER = threading.Barrier(2)

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    return json.loads(sys.stdin.buffer.read(length))

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    with WRITE_LOCK:
        sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
        sys.stdout.buffer.write(body)
        sys.stdout.buffer.flush()

def respond_to_tool(message):
    arguments = message.get("params", {}).get("arguments", {})
    if arguments.get("exit"):
        os._exit(17)
    if arguments.get("barrier"):
        RACE_BARRIER.wait(timeout=5)
    time.sleep(arguments.get("delay_ms", 0) / 1000.0)
    write_message({
        "jsonrpc": "2.0",
        "id": message["id"],
        "result": {
            "content": [{"type": "text", "text": arguments.get("value", "")}],
            "structuredContent": {"value": arguments.get("value", "")}
        }
    })

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    if method == "initialize":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}, "resources": {}}, "serverInfo": {"name": "concurrent", "version": "1"}}})
    elif method == "tools/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": [{"name": "concurrent.echo", "description": "concurrent test tool", "inputSchema": {"type": "object", "properties": {"delay_ms": {"type": "integer"}, "value": {"type": "string"}}}}]}})
    elif method == "resources/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resources": []}})
    elif method == "resources/templates/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resourceTemplates": []}})
    elif method == "tools/call":
        threading.Thread(target=respond_to_tool, args=(message,), daemon=True).start()
    else:
        write_message({"jsonrpc": "2.0", "id": msg_id, "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .unwrap();
}

pub fn write_bidirectional_server(path: &Path) {
    fs::write(
        path,
        r#"import json, sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    return json.loads(sys.stdin.buffer.read(length))

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

requested_roots = False
while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    msg_id = message.get("id")
    if method == "initialize":
        write_message({
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "serverInfo": {"name": "bidirectional", "version": "1"}
            }
        })
    elif method == "notifications/initialized" and not requested_roots:
        requested_roots = True
        write_message({
            "jsonrpc": "2.0",
            "id": "server-roots-1",
            "method": "roots/list",
            "params": {}
        })
    elif method is None and msg_id == "server-roots-1":
        roots = message.get("result", {}).get("roots", [])
        write_message({
            "jsonrpc": "2.0",
            "method": "notifications/message",
            "params": {
                "level": "info",
                "data": {"root_count": len(roots)}
            }
        })
    elif msg_id is not None:
        if method == "tools/list":
            result = {"tools": []}
        elif method == "resources/list":
            result = {"resources": []}
        elif method == "resources/templates/list":
            result = {"resourceTemplates": []}
        else:
            write_message({
                "jsonrpc": "2.0",
                "id": msg_id,
                "error": {"code": -32601, "message": "unknown method"}
            })
            continue
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": result})
"#,
    )
    .unwrap();
}

pub fn write_upstream_batch_server(path: &Path) {
    fs::write(
        path,
        r#"import json, sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    return json.loads(sys.stdin.buffer.read(length))

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

def diagnostic(kind, count):
    write_message({
        "jsonrpc": "2.0",
        "method": "notifications/message",
        "params": {"data": {"diagnostic": kind, "count": count}}
    })

sent_mixed_batch = False
while True:
    message = read_message()
    if message is None:
        break
    if isinstance(message, list):
        if (
            len(message) == 1
            and message[0].get("id") == "server-batch-roots"
            and "result" in message[0]
        ):
            diagnostic("reverse-array", len(message))
            write_message([])
        elif message and all(item.get("error", {}).get("code") == -32600 for item in message):
            diagnostic("invalid", len(message))
        else:
            raise RuntimeError(f"unexpected response batch: {message!r}")
        continue

    method = message.get("method")
    msg_id = message.get("id")
    if method == "initialize":
        write_message({
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "upstream-batch", "version": "1"}
            }
        })
    elif method == "tools/list" and not sent_mixed_batch:
        sent_mixed_batch = True
        write_message([
            {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {"tools": []}
            },
            {
                "jsonrpc": "2.0",
                "id": "server-batch-roots",
                "method": "roots/list",
                "params": {}
            },
            {
                "jsonrpc": "2.0",
                "method": "notifications/message",
                "params": {"data": {"kind": "mixed"}}
            }
        ])
    elif method is None and msg_id == "server-batch-roots":
        raise RuntimeError("reverse batch response arrived as a singleton")
    elif method is None and msg_id is None and message.get("error", {}).get("code") == -32600:
        diagnostic("empty", 1)
        write_message([
            17,
            {
                "jsonrpc": "2.0",
                "id": "invalid-method",
                "method": 17
            },
            {
                "jsonrpc": "2.0",
                "id": "invalid-response"
            }
        ])
    elif msg_id is not None:
        if method == "resources/list":
            result = {"resources": []}
        elif method == "resources/templates/list":
            result = {"resourceTemplates": []}
        else:
            write_message({
                "jsonrpc": "2.0",
                "id": msg_id,
                "error": {"code": -32601, "message": "unknown method"}
            })
            continue
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": result})
"#,
    )
    .unwrap();
}

pub fn write_newline_only_server(path: &Path) {
    fs::write(
        path,
        r#"import json, sys

def read_message():
    line = sys.stdin.buffer.readline()
    if not line:
        return None
    if line.lower().startswith(b"content-length:"):
        raise RuntimeError("legacy Content-Length framing is not accepted")
    return json.loads(line)

def write_message(value):
    body = json.dumps(value, separators=(",", ":")).encode("utf-8")
    sys.stdout.buffer.write(body + b"\n")
    sys.stdout.buffer.flush()

pending_tool_call = None
while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    msg_id = message.get("id")
    if method == "initialize":
        write_message({
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "newline-only", "version": "1"}
            }
        })
    elif method == "tools/list":
        write_message({
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "tools": [{
                    "name": "newline.echo",
                    "description": "strict newline-framed upstream",
                    "inputSchema": {"type": "object", "properties": {}}
                }]
            }
        })
    elif method == "tools/call":
        pending_tool_call = msg_id
        write_message({
            "jsonrpc": "2.0",
            "id": "newline-roots",
            "method": "roots/list",
            "params": {}
        })
    elif method is None and msg_id == "newline-roots":
        roots = message.get("result", {}).get("roots", [])
        write_message({
            "jsonrpc": "2.0",
            "id": pending_tool_call,
            "result": {
                "content": [{"type": "text", "text": "newline ok"}],
                "structuredContent": {"root_count": len(roots)}
            }
        })
        pending_tool_call = None
    elif msg_id is not None:
        write_message({
            "jsonrpc": "2.0",
            "id": msg_id,
            "error": {"code": -32601, "message": "unknown method"}
        })
"#,
    )
    .unwrap();
}
