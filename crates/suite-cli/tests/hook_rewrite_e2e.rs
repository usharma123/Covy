#[path = "support/hook_rewrite.rs"]
mod hook_rewrite;

use serde_json::{json, Value};
use tempfile::TempDir;

use hook_rewrite::{
    ensure_packet28d_built, init_repo, run_hook_raw, suite_cmd, write_repo_fixture,
};

fn run_claude_hook(root: &std::path::Path, payload: &Value) -> (i32, String) {
    let (status, stdout, _) =
        run_hook_raw("claude", root, &serde_json::to_string(payload).unwrap());
    (status, stdout)
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
