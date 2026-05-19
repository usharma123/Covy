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
