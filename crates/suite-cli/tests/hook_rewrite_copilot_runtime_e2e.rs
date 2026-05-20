#[path = "support/hook_rewrite.rs"]
mod hook_rewrite;

use serde_json::{json, Value};
use tempfile::TempDir;

use hook_rewrite::{
    ensure_packet28d_built, init_repo, run_hook_raw, suite_cmd, write_repo_fixture,
};

#[test]
#[cfg(unix)]
fn test_hook_rewrite_copilot_runtime_rewrites_vscode_and_denies_cli_with_suggestion() {
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
