mod support;

#[path = "support/daemon.rs"]
mod daemon_support;

use daemon_support::{
    cli_with_daemon_env, start_daemon, start_daemon_forced_tcp, start_daemon_workspace_fallback,
    stop_daemon,
};
use packet28_daemon_protocol::message::DaemonRuntimeInfo;
use packet28_daemon_protocol::paths::{runtime_path, workspace_socket_path};
use std::fs;

use predicates::prelude::*;
use support::{cli, output, stderr_text, stdout_text, write_fixture};

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
fn p28_uses_authenticated_forced_tcp_runtime_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    fs::create_dir_all(workspace.join(".git")).unwrap();
    let subtree = workspace.join("crates/search-sample");
    write_fixture(&subtree);

    cli()
        .args(["debug", "build", workspace.to_str().unwrap()])
        .assert()
        .success();

    let daemon = start_daemon_forced_tcp(workspace);

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
fn p28_uses_authoritative_workspace_socket_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    fs::create_dir_all(workspace.join(".git")).unwrap();
    let subtree = workspace.join("crates/search-sample");
    write_fixture(&subtree);

    cli()
        .args(["debug", "build", workspace.to_str().unwrap()])
        .assert()
        .success();

    let daemon = start_daemon_workspace_fallback(workspace);
    let runtime: DaemonRuntimeInfo =
        serde_json::from_slice(&fs::read(runtime_path(workspace)).unwrap()).unwrap();
    let canonical_workspace = workspace.canonicalize().unwrap();
    assert_eq!(
        runtime.socket_path,
        workspace_socket_path(&canonical_workspace).to_string_lossy()
    );

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
