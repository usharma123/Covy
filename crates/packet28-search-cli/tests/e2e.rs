use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use packet28_daemon_core::{
    read_socket_message, ready_path, socket_path, write_socket_message, DaemonRequest,
    DaemonResponse,
};
use predicates::prelude::*;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;

fn cli() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("p28"))
}

fn daemon_bin() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let status = ProcessCommand::new("cargo")
        .args(["build", "-p", "packet28d"])
        .current_dir(workspace)
        .status()
        .expect("build packet28d");
    assert!(status.success(), "packet28d build failed");
    workspace.join("target/debug/packet28d")
}

fn cli_with_daemon_env() -> Command {
    let mut command = cli();
    command.env("CARGO_BIN_EXE_packet28d", daemon_bin());
    command
}

fn write_fixture(root: &Path) {
    fs::create_dir_all(root.join("src/nested")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub struct Alpha;\npub fn alpha_service() {}\nconst ALPHA: &str = \"Alpha\";\n",
    )
    .unwrap();
    fs::write(
        root.join("src/nested/mod.rs"),
        "pub enum Beta { AlphaVariant }\nfn handle_value() { println!(\"beta\"); }\n",
    )
    .unwrap();
    for idx in 0..10 {
        fs::write(
            root.join("src").join(format!("filler_{idx}.rs")),
            format!("pub fn filler_{idx}() {{ println!(\"beta_{idx}\"); }}\n"),
        )
        .unwrap();
    }
}

fn write_fake_fff_mcp(root: &Path) -> PathBuf {
    let fake_fff = root.join("fake-fff-mcp.sh");
    fs::write(
        &fake_fff,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake-fff","version":"0"}}}'
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"→ Read src/lib.rs (best match)\nsrc/lib.rs\n 1: pub struct Alpha;\nsrc/nested/mod.rs\n 1: pub enum Beta { AlphaVariant }"}]}}'
      ;;
  esac
done
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&fake_fff).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_fff, perms).unwrap();
    fake_fff
}

struct DaemonHandle {
    child: Child,
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[allow(clippy::zombie_processes)]
fn start_daemon(root: &Path) -> DaemonHandle {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut child = ProcessCommand::new(daemon_bin())
        .args(["serve", "--root", canonical_root.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        if ready_path(&canonical_root).exists() && socket_path(&canonical_root).exists() {
            return DaemonHandle { child };
        }
        if let Some(status) = child.try_wait().unwrap() {
            let mut stderr = String::new();
            if let Some(mut stream) = child.stderr.take() {
                let _ = stream.read_to_string(&mut stderr);
            }
            let mut stdout = String::new();
            if let Some(mut stream) = child.stdout.take() {
                let _ = stream.read_to_string(&mut stdout);
            }
            panic!(
                "packet28d exited early for {} with status {status}; stdout={stdout:?} stderr={stderr:?}",
                canonical_root.display()
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let mut stderr = String::new();
    if let Some(mut stream) = child.stderr.take() {
        let _ = stream.read_to_string(&mut stderr);
    }
    let mut stdout = String::new();
    if let Some(mut stream) = child.stdout.take() {
        let _ = stream.read_to_string(&mut stdout);
    }
    panic!(
        "packet28d did not become ready for {}; stdout={stdout:?} stderr={stderr:?}",
        canonical_root.display()
    );
}

fn stop_daemon(root: &Path) {
    let socket = socket_path(root);
    if !socket.exists() {
        return;
    }
    if let Ok(stream) = UnixStream::connect(&socket) {
        let reader_stream = stream.try_clone().unwrap();
        let mut writer = std::io::BufWriter::new(stream);
        let mut reader = std::io::BufReader::new(reader_stream);
        let _ = write_socket_message(&mut writer, &DaemonRequest::Stop);
        let _ = read_socket_message::<_, DaemonResponse>(&mut reader);
    }
}

fn output(mut command: Command) -> Output {
    command.output().expect("command output")
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn debug_build_prints_generation_and_file_count() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("build_ms="))
        .stdout(predicate::str::contains("generation="))
        .stdout(predicate::str::contains("files="));
}

#[test]
fn p28_searches_from_repo_root_with_rg_style_output() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .current_dir(dir.path())
        .args(["Alpha", "--fixed-strings"])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"));
}

#[test]
fn p28_filters_paths_from_current_directory() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .current_dir(dir.path())
        .args(["handle_value", "src/nested/mod.rs", "--fixed-strings"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "src/nested/mod.rs:2:fn handle_value() { println!(\"beta\"); }",
        ))
        .stdout(predicate::str::contains("src/lib.rs").not());
}

#[test]
fn p28_stats_go_to_stderr_while_hits_stay_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let output = output({
        let mut command = cli();
        command
            .current_dir(dir.path())
            .args(["Alpha", "--fixed-strings", "--stats"]);
        command
    });

    assert!(output.status.success());
    let stdout = stdout_text(&output);
    let stderr = stderr_text(&output);
    assert!(stdout.contains("src/lib.rs:1:pub struct Alpha;"));
    assert!(!stdout.contains("backend="));
    assert!(stderr.contains("p28_ms="));
    assert!(stderr.contains("transport="));
    assert!(stderr.contains("backend="));
}

#[test]
fn p28_fff_engine_adapts_mcp_grep_results() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let fake_fff = write_fake_fff_mcp(dir.path());

    cli()
        .current_dir(dir.path())
        .env("P28_FFF_MCP_BIN", &fake_fff)
        .args(["Alpha", "--engine", "fff", "--fixed-strings", "--stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"))
        .stdout(predicate::str::contains(
            "src/nested/mod.rs:1:pub enum Beta { AlphaVariant }",
        ))
        .stderr(predicate::str::contains("backend=fff_mcp"));
}

#[test]
fn p28_auto_uses_fff_for_broad_index_fallback_when_available() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let fake_fff = write_fake_fff_mcp(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cli()
        .current_dir(dir.path())
        .env("P28_FFF_MCP_BIN", &fake_fff)
        .args(["fn", "--transport", "inproc", "--stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"))
        .stderr(predicate::str::contains("backend=fff_mcp"));
}

#[test]
fn p28_auto_can_prefer_fff_when_configured() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let fake_fff = write_fake_fff_mcp(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cli()
        .current_dir(dir.path())
        .env("P28_FFF_MCP_BIN", &fake_fff)
        .env("P28_FFF_AUTO", "prefer")
        .args(["Alpha", "--transport", "inproc", "--stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"))
        .stderr(predicate::str::contains("backend=fff_mcp"));
}

#[test]
fn p28_handles_anchored_line_start_regexes() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn build() {\n    SearchRequest {\n        query: pattern,\n    };\n}\n",
    )
    .unwrap();

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cli()
        .current_dir(dir.path())
        .args([r"^\s*SearchRequest\s*\{", "--stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "src/main.rs:2:    SearchRequest {",
        ))
        .stderr(predicate::str::contains("backend="));
}

#[test]
fn debug_bench_prints_packet28_and_legacy_timings() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cli()
        .args([
            "debug",
            "bench",
            dir.path().to_str().unwrap(),
            "Alpha",
            "--fixed-strings",
            "--transport",
            "inproc",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("guard=index"))
        .stdout(predicate::str::contains("parity=exact"))
        .stdout(predicate::str::contains("p28_ms="))
        .stdout(predicate::str::contains("legacy_rg_ms="));
}

#[test]
fn p28_supports_daemon_transport_for_subtree_roots() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    fs::create_dir_all(workspace.join(".git")).unwrap();
    let subtree = workspace.join("crates/search-sample");
    write_fixture(&subtree);

    cli()
        .args(["debug", "build", workspace.to_str().unwrap()])
        .assert()
        .success();

    let daemon = start_daemon(workspace);

    cli()
        .current_dir(&subtree)
        .args([
            "Alpha",
            "--fixed-strings",
            "--transport",
            "daemon",
            "--stats",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"))
        .stderr(predicate::str::contains("transport=daemon"))
        .stderr(predicate::str::contains("backend=indexed_regex"));

    drop(daemon);
}

#[test]
fn indexed_engine_mode_is_enforced_over_daemon_transport() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    fs::create_dir_all(workspace.join(".git")).unwrap();
    let subtree = workspace.join("crates/search-sample");
    write_fixture(&subtree);

    cli()
        .args(["debug", "build", workspace.to_str().unwrap()])
        .assert()
        .success();

    let daemon = start_daemon(workspace);

    cli()
        .current_dir(&subtree)
        .args([".+", "--engine", "indexed", "--transport", "daemon"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("planner could not derive"));

    drop(daemon);
}

#[test]
fn debug_guard_reports_daemon_fallback_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    fs::create_dir_all(workspace.join(".git")).unwrap();
    let subtree = workspace.join("crates/search-sample");
    write_fixture(&subtree);

    cli()
        .args(["debug", "build", workspace.to_str().unwrap()])
        .assert()
        .success();

    let daemon = start_daemon(workspace);

    cli()
        .args([
            "debug",
            "guard",
            subtree.to_str().unwrap(),
            ".+",
            "--transport",
            "daemon",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("mode=fallback"))
        .stdout(predicate::str::contains("reason="));

    drop(daemon);
}

#[test]
fn p28_auto_starts_daemon_and_waits_for_indexed_backend() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    fs::create_dir_all(workspace.join(".git")).unwrap();
    let subtree = workspace.join("crates/search-sample");
    write_fixture(&subtree);

    let first = output({
        let mut command = cli_with_daemon_env();
        command
            .current_dir(&subtree)
            .args(["Alpha", "--fixed-strings", "--stats"]);
        command
    });

    assert!(first.status.success());
    assert!(stdout_text(&first).contains("src/lib.rs:1:pub struct Alpha;"));
    let first_stderr = stderr_text(&first);
    assert!(first_stderr.contains("transport=daemon"));
    assert!(first_stderr.contains("backend=indexed_regex"));

    stop_daemon(workspace);
}
