use assert_cmd::Command;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;
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

fn init_repo(root: &Path) {
    git(root, &["init"]);
}

fn run_claude_hook(root: &Path, payload: &Value) -> (i32, String) {
    let (status, stdout, _) =
        run_hook_raw("claude", root, &serde_json::to_string(payload).unwrap());
    (status, stdout)
}

fn run_hook_raw(runtime: &str, root: &Path, stdin_payload: &str) -> (i32, String, String) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
        .current_dir(root)
        .args(["hook", runtime, "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_payload.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn assert_claude_rewrite(command: &str, family: &str, kind: &str) {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":format!("task-pretool-{family}-rewrite"),
            "session_id":format!("session-pretool-{family}-rewrite"),
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":command}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains(&format!("--family {family}")));
    assert!(rewritten.contains(&format!("--kind {kind}")));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_hook_rewrite_cli_degrades_gracefully_on_bad_json_and_no_rewrite() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());

    let (status, stdout, stderr) = run_hook_raw("claude", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout, stderr) = run_hook_raw("cursor", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout, stderr) = run_hook_raw("copilot", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout, stderr) = run_hook_raw("gemini", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-no-rewrite",
            "session_id":"session-pretool-no-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"definitely-unsupported-packet28-tool --flag"}
        }),
    );
    assert_eq!(status, 0);
    assert!(matches!(stdout.trim(), "" | "{}"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_hook_rewrite_cli_is_idempotent_and_ignores_non_bash_tools() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let base_payload = json!({
        "hook_event_name":"PreToolUse",
        "task_id":"task-pretool-idempotent",
        "session_id":"session-pretool-idempotent",
        "cwd":dir.path().to_str().unwrap(),
        "tool_name":"Bash",
        "tool_input":{"command":"git status --short src/alpha.rs"}
    });
    let (status, stdout) = run_claude_hook(dir.path(), &base_payload);
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-idempotent",
            "session_id":"session-pretool-idempotent",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command": rewritten}
        }),
    );
    assert_eq!(status, 0);
    assert!(matches!(stdout.trim(), "" | "{}"));

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-non-bash",
            "session_id":"session-pretool-idempotent",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Read",
            "tool_input":{"file_path":"src/alpha.rs"}
        }),
    );
    assert_eq!(status, 0);
    assert!(matches!(stdout.trim(), "" | "{}"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_hook_rewrite_cli_cursor_pretool_rewrites_and_returns_empty_json_on_noop() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let payloads = [
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "command":"git status --short src/alpha.rs"
        }),
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-tool-input-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"command":"git status --short src/alpha.rs"}
        }),
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-command-line-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "command_line":"git status --short src/alpha.rs"
        }),
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-shell-command-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "shell_command":"git status --short src/alpha.rs"
        }),
    ];
    let mut first_rewritten = String::new();
    for payload in payloads {
        let (status, stdout, _stderr) = run_hook_raw(
            "cursor",
            dir.path(),
            &serde_json::to_string(&payload).unwrap(),
        );
        assert_eq!(status, 0);
        let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(rendered["permission"].as_str(), Some("allow"));
        let rewritten = rendered["updated_input"]["command"].as_str().unwrap();
        assert!(rewritten.contains("hook reducer-runner"));
        assert!(rewritten.contains("--family git"));
        assert!(rewritten.contains("--kind git_status"));
        if first_rewritten.is_empty() {
            first_rewritten = rewritten.to_string();
        }
    }

    let (status, stdout, _stderr) = run_hook_raw(
        "cursor",
        dir.path(),
        &serde_json::to_string(&json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-idempotent",
            "cwd":dir.path().to_str().unwrap(),
            "command":first_rewritten
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    assert_eq!(stdout.trim(), "{}");

    let (status, stdout, _stderr) = run_hook_raw(
        "cursor",
        dir.path(),
        &serde_json::to_string(&json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-noop",
            "cwd":dir.path().to_str().unwrap(),
            "command":"definitely-unsupported-packet28-tool --flag"
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    assert_eq!(stdout.trim(), "{}");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_hook_rewrite_cli_gemini_before_tool_rewrites_shell_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout, _stderr) = run_hook_raw(
        "gemini",
        dir.path(),
        &serde_json::to_string(&json!({
            "tool_name":"run_shell_command",
            "session_id":"gemini-session-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"command":"git status --short src/alpha.rs"}
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(rendered["decision"].as_str(), Some("allow"));
    let rewritten = rendered["hookSpecificOutput"]["tool_input"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family git"));
    assert!(rewritten.contains("--kind git_status"));

    let (status, stdout, _stderr) = run_hook_raw(
        "gemini",
        dir.path(),
        &serde_json::to_string(&json!({
            "tool_name":"read_file",
            "session_id":"gemini-session-noop",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"path":"src/alpha.rs"}
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(rendered["decision"].as_str(), Some("allow"));
    assert!(rendered.get("hookSpecificOutput").is_none());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_hook_rewrite_cli_copilot_rewrites_vscode_and_denies_cli_with_suggestion() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout, _stderr) = run_hook_raw(
        "copilot",
        dir.path(),
        &serde_json::to_string(&json!({
            "tool_name":"Bash",
            "session_id":"copilot-vscode-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"command":"git status --short src/alpha.rs"}
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        rendered["hookSpecificOutput"]["permissionDecision"].as_str(),
        Some("allow")
    );
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family git"));
    assert!(rewritten.contains("--kind git_status"));

    let tool_args = serde_json::to_string(&json!({
        "command":"git status --short src/alpha.rs"
    }))
    .unwrap();
    let (status, stdout, _stderr) = run_hook_raw(
        "copilot",
        dir.path(),
        &serde_json::to_string(&json!({
            "toolName":"bash",
            "toolArgs":tool_args
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(rendered["permissionDecision"].as_str(), Some("deny"));
    let reason = rendered["permissionDecisionReason"].as_str().unwrap();
    assert!(reason.contains("hook reducer-runner"));
    assert!(reason.contains("Packet28"));

    let (status, stdout, _stderr) = run_hook_raw(
        "copilot",
        dir.path(),
        &serde_json::to_string(&json!({
            "toolName":"view",
            "toolArgs":"{}"
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_hook_rewrite_cli_rewrites_supported_git_command() {
    assert_claude_rewrite("git status --short src/alpha.rs", "git", "git_status");
}

#[test]
#[cfg(unix)]
fn test_hook_rewrite_cli_rewrites_supported_github_command() {
    assert_claude_rewrite("gh pr list --limit 5", "github", "gh_pr_list");
}

#[test]
#[cfg(unix)]
fn test_hook_rewrite_cli_rewrites_supported_python_command() {
    assert_claude_rewrite("python3 -m pytest tests", "python", "python_pytest");
}

#[test]
#[cfg(unix)]
fn test_hook_rewrite_cli_rewrites_supported_javascript_command() {
    assert_claude_rewrite("npx tsc --noEmit", "javascript", "javascript_tsc");
}

#[test]
#[cfg(unix)]
fn test_hook_rewrite_cli_rewrites_supported_go_command() {
    assert_claude_rewrite("go test ./...", "go", "go_test");
}

#[test]
#[cfg(unix)]
fn test_hook_rewrite_cli_rewrites_supported_infra_command() {
    assert_claude_rewrite("kubectl get pods", "infra", "kubectl_get");
}
