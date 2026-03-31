use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
#[cfg(unix)]
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[cfg(unix)]
fn mcp_cmd() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
}

#[cfg(unix)]
fn write_fake_codex_binary(path: &Path, log_path: &Path) {
    let script = format!(
        "#!/bin/sh\n\
set -eu\n\
printf '%s\\n' \"$*\" >> \"{log_path}\"\n\
if [ \"${{1:-}}\" = \"mcp\" ] && [ \"${{2:-}}\" = \"add\" ] && [ \"${{3:-}}\" = \"packet28\" ]; then\n\
  shift 3\n\
  if [ \"${{1:-}}\" = \"--\" ]; then\n\
    shift\n\
  fi\n\
  command_name=\"${{1:-packet28-mcp}}\"\n\
  shift || true\n\
  root=\"\"\n\
  while [ \"$#\" -gt 0 ]; do\n\
    if [ \"$1\" = \"--root\" ]; then\n\
      root=\"$2\"\n\
      break\n\
    fi\n\
    shift\n\
  done\n\
  mkdir -p \"$HOME/.codex\"\n\
  cat > \"$HOME/.codex/config.toml\" <<EOF\n\
[mcp_servers.packet28]\n\
command = \"$command_name\"\n\
args = [\"--root\", \"$root\"]\n\
EOF\n\
  exit 0\n\
fi\n\
exit 0\n",
        log_path = log_path.display(),
    );
    fs::write(path, script).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[cfg(unix)]
fn write_mcp_message(stdin: &mut ChildStdin, value: &Value) {
    let body = serde_json::to_vec(value).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.flush().unwrap();
}

#[cfg(unix)]
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
    std::io::Read::read_exact(stdout, &mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[cfg(unix)]
fn read_mcp_message_for_id(stdout: &mut BufReader<ChildStdout>, expected_id: u64) -> Value {
    loop {
        let value = read_mcp_message(stdout);
        if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
            return value;
        }
    }
}

#[cfg(unix)]
fn start_mcp_server(root: &Path) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = mcp_cmd()
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

#[cfg(unix)]
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

#[cfg(unix)]
fn write_repo_fixture(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub struct AlphaUniqueToken;\npub fn alpha_unique_token() -> &'static str { \"AlphaUniqueToken\" }\n",
    )
    .unwrap();
}

#[test]
#[cfg(unix)]
fn test_setup_only_writes_artifacts_for_detected_runtimes() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let bin_dir = TempDir::new().unwrap();
    let codex_log = home.path().join("codex-cli.log");
    write_fake_codex_binary(&bin_dir.path().join("codex"), &codex_log);

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", bin_dir.path().display()),
        )
        .args(["setup", "--root", root.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Your agent runtimes are configured to use Packet28 control-plane MCP tools.",
        ));

    assert!(root.path().join("AGENTS.md").exists());
    assert!(!root.path().join(".codex").join("hooks.json").exists());
    assert!(!root.path().join("CLAUDE.md").exists());
    assert!(!root.path().join(".cursorrules").exists());
    assert!(home.path().join(".codex").join("config.toml").exists());
    assert!(fs::read_to_string(codex_log)
        .unwrap()
        .contains("mcp add packet28 -- packet28-mcp --root"));
}

#[test]
#[cfg(unix)]
fn test_setup_refuses_to_overwrite_invalid_mcp_json() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let claude_config = root.path().join(".mcp.json");
    fs::write(&claude_config, "{ invalid json").unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "setup",
            "--root",
            root.path().to_str().unwrap(),
            "--runtime",
            "claude",
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "refusing to overwrite invalid JSON",
        ));

    assert_eq!(
        fs::read_to_string(&claude_config).unwrap(),
        "{ invalid json"
    );
}

#[test]
#[cfg(unix)]
fn test_setup_refuses_to_overwrite_invalid_codex_toml() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let codex_config = home.path().join(".codex").join("config.toml");
    fs::create_dir_all(codex_config.parent().unwrap()).unwrap();
    fs::write(&codex_config, "[features").unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "setup",
            "--root",
            root.path().to_str().unwrap(),
            "--runtime",
            "codex",
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "refusing to overwrite invalid TOML",
        ));

    assert_eq!(fs::read_to_string(&codex_config).unwrap(), "[features");
}

#[test]
#[cfg(unix)]
fn test_setup_cursor_writes_rules_hooks_and_mcp_without_legacy_cursorrules() {
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
            "cursor",
            "--yes",
        ])
        .assert()
        .success();

    assert!(root.path().join(".cursor").join("mcp.json").exists());
    assert!(root.path().join(".cursor").join("hooks.json").exists());
    assert!(root
        .path()
        .join(".cursor")
        .join("rules")
        .join("packet28.mdc")
        .exists());
    assert!(!root.path().join(".cursorrules").exists());
}

#[test]
#[cfg(unix)]
fn test_setup_cursor_is_idempotent() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    for _ in 0..2 {
        suite_cmd()
            .current_dir(root.path())
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .args([
                "setup",
                "--root",
                root.path().to_str().unwrap(),
                "--runtime",
                "cursor",
                "--yes",
            ])
            .assert()
            .success();
    }

    let hooks: Value = serde_json::from_str(
        &fs::read_to_string(root.path().join(".cursor").join("hooks.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        hooks["hooks"]["beforeSubmitPrompt"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        hooks["hooks"]["beforeShellExecution"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        hooks["hooks"]["afterShellExecution"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(hooks["hooks"]["stop"].as_array().unwrap().len(), 1);

    let mcp: Value = serde_json::from_str(
        &fs::read_to_string(root.path().join(".cursor").join("mcp.json")).unwrap(),
    )
    .unwrap();
    assert!(mcp["mcpServers"]["packet28"].is_object());
    assert_eq!(mcp["mcpServers"].as_object().unwrap().len(), 1);
}

#[test]
#[cfg(unix)]
fn test_setup_codex_writes_mcp_and_agents_without_hooks() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let bin_dir = TempDir::new().unwrap();
    let codex_log = home.path().join("codex-cli.log");
    write_fake_codex_binary(&bin_dir.path().join("codex"), &codex_log);

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", bin_dir.path().display()),
        )
        .args([
            "setup",
            "--root",
            root.path().to_str().unwrap(),
            "--runtime",
            "codex",
            "--yes",
        ])
        .assert()
        .success();

    assert!(root.path().join("AGENTS.md").exists());
    assert!(!root.path().join(".codex").join("hooks.json").exists());
    let config = fs::read_to_string(home.path().join(".codex").join("config.toml")).unwrap();
    assert!(config.contains("[mcp_servers.packet28]"));
    assert!(!config.contains("codex_hooks = true"));
    assert!(fs::read_to_string(codex_log)
        .unwrap()
        .contains("mcp add packet28 -- packet28-mcp --root"));
}

#[test]
#[cfg(unix)]
fn test_setup_windsurf_writes_rules_hooks_and_mcp() {
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
}

#[test]
#[cfg(unix)]
fn test_setup_builds_regex_index_and_search_uses_indexed_backend() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    write_repo_fixture(root.path());

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "setup",
            "--root",
            root.path().to_str().unwrap(),
            "--runtime",
            "cursor",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("index ready"));

    assert!(root
        .path()
        .join(".packet28")
        .join("index")
        .join("regex-v1")
        .join("manifest.json")
        .exists());

    let status_output = suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "daemon",
            "index",
            "status",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: Value = serde_json::from_slice(&status_output).unwrap();
    assert_eq!(status.get("ready").and_then(Value::as_bool), Some(true));
    assert_eq!(
        status
            .get("manifest")
            .and_then(|manifest| manifest.get("regex_status"))
            .and_then(Value::as_str),
        Some("ready")
    );

    let (mut child, mut stdin, mut stdout) = start_mcp_server(root.path());
    initialize_mcp_session(&mut stdin, &mut stdout);
    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"packet28.search",
                "arguments":{
                    "task_id":"task-setup-regex-index",
                    "query":"AlphaUniqueToken",
                    "fixed_string":true,
                    "response_mode":"full"
                }
            }
        }),
    );
    let search = read_mcp_message_for_id(&mut stdout, 2);
    assert_eq!(
        search["result"]["structuredContent"]["engine"]["engine"].as_str(),
        Some("indexed_regex")
    );
    assert!(
        search["result"]["structuredContent"]["match_count"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    );

    let _ = child.kill();
    let _ = child.wait();
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
