mod support;

use std::fs;
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use packet28_daemon_core::{
    read_socket_message, ready_path, socket_path, write_socket_message, DaemonRequest,
    DaemonResponse,
};
use predicates::prelude::*;
use support::{cli, output, stderr_text, stdout_text, write_fixture};

fn daemon_bin() -> PathBuf {
    static DAEMON_BIN: OnceLock<PathBuf> = OnceLock::new();
    DAEMON_BIN
        .get_or_init(|| {
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
        })
        .clone()
}

fn cli_with_daemon_env() -> assert_cmd::Command {
    let mut command = cli();
    command.env("CARGO_BIN_EXE_packet28d", daemon_bin());
    command
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
