#[path = "support/daemon_lifecycle.rs"]
mod daemon_lifecycle;

use daemon_lifecycle::process_harness::{HarnessLimits, ProcessHarness};
use daemon_lifecycle::{ensure_packet28d_built, init_repo, suite_cmd, write_repo_fixture};
use serde_json::Value;
use std::fs;
use std::net::TcpListener;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::time::Duration;
use tempfile::TempDir;

#[test]
#[cfg(unix)]
fn test_daemon_lifecycle_cli_stop_does_not_start_missing_daemon() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    assert!(!dir.path().join(".packet28").exists());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout("stopping\n");

    assert!(!dir.path().join(".packet28").exists());
}

#[test]
#[cfg(unix)]
fn test_daemon_lifecycle_cli_start_status_stop_cycle() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    init_repo(dir.path());

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let status_output = suite_cmd()
        .args([
            "daemon",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: Value = serde_json::from_slice(&status_output).unwrap();
    let expected_root = fs::canonicalize(dir.path()).unwrap();
    assert_eq!(
        status.get("workspace_root").and_then(Value::as_str),
        expected_root.to_str()
    );
    assert!(status.get("pid").and_then(Value::as_u64).unwrap() > 0);
    assert!(status.get("ready_at_unix").and_then(Value::as_u64).unwrap() > 0);
    assert!(status
        .get("log_path")
        .and_then(Value::as_str)
        .is_some_and(|path| Path::new(path).exists()));
    assert!(dir.path().join(".packet28/daemon/ready").exists());
    assert!(dir.path().join(".packet28/daemon/packet28d.log").exists());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_concurrent_daemon_clients_share_one_workspace_process() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    init_repo(dir.path());

    let clients = 16;
    let barrier = Arc::new(Barrier::new(clients));
    let root = Arc::new(dir.path().to_path_buf());
    let workers = (0..clients)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let root = Arc::clone(&root);
            std::thread::spawn(move || {
                Barrier::wait(&barrier);
                let mut command =
                    std::process::Command::new(assert_cmd::cargo::cargo_bin!("Packet28"));
                command.args([
                    "daemon",
                    "status",
                    "--root",
                    root.to_str().unwrap(),
                    "--json",
                ]);
                let output = ProcessHarness::run(
                    &mut command,
                    &[],
                    Duration::from_secs(45),
                    HarnessLimits::default(),
                )
                .expect("run lifecycle client within deadline");
                assert!(
                    output.status.success(),
                    "concurrent daemon client failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                serde_json::from_slice::<Value>(&output.stdout)
                    .unwrap()
                    .get("pid")
                    .and_then(Value::as_u64)
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();

    let pids = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert!(pids.iter().all(|pid| *pid == pids[0]));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_daemon_lifecycle_forced_tcp_stop_exits_and_releases_endpoint() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    init_repo(dir.path());

    suite_cmd()
        .env("PACKET28D_FORCE_TCP", "1")
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let status_output = suite_cmd()
        .args([
            "daemon",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: Value = serde_json::from_slice(&status_output).unwrap();
    let pid = i32::try_from(status.get("pid").and_then(Value::as_u64).unwrap()).unwrap();
    let endpoint = status.get("socket_path").and_then(Value::as_str).unwrap();
    let address = endpoint
        .strip_prefix("tcp://")
        .expect("forced TCP daemon did not publish a TCP endpoint")
        .to_string();
    let runtime_path = dir.path().join(".packet28/daemon/runtime.json");
    let runtime_mode = fs::metadata(&runtime_path).unwrap().permissions().mode() & 0o777;
    let runtime: Value = serde_json::from_slice(&fs::read(&runtime_path).unwrap()).unwrap();
    assert_eq!(runtime_mode, 0o600);
    assert!(runtime
        .get("transport_auth")
        .and_then(|auth| auth.get("secret"))
        .and_then(Value::as_str)
        .is_some_and(|secret| secret.len() == 64));
    assert!(status.get("transport_auth").is_none());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout("stopping\n");

    let started = std::time::Instant::now();
    while process_exists(pid) && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !process_exists(pid),
        "forced TCP daemon process {pid} did not exit after Stop"
    );
    TcpListener::bind(&address)
        .unwrap_or_else(|error| panic!("TCP endpoint {address} was not released: {error}"));
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    // SAFETY: signal 0 performs a non-mutating process existence check for the
    // positive PID returned by the daemon status response.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[test]
#[cfg(unix)]
fn test_daemon_lifecycle_cli_index_rebuild_and_status() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    init_repo(dir.path());

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let rebuild_output = suite_cmd()
        .args([
            "daemon",
            "index",
            "rebuild",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rebuild: Value = serde_json::from_slice(&rebuild_output).unwrap();
    assert_eq!(rebuild.get("accepted").and_then(Value::as_bool), Some(true));
    assert_eq!(rebuild.get("full").and_then(Value::as_bool), Some(true));

    let start = std::time::Instant::now();
    let mut ready = false;
    while start.elapsed() < Duration::from_secs(5) {
        let status_output = suite_cmd()
            .args([
                "daemon",
                "index",
                "status",
                "--root",
                dir.path().to_str().unwrap(),
                "--json",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let status: Value = serde_json::from_slice(&status_output).unwrap();
        if status.get("ready").and_then(Value::as_bool) == Some(true) {
            ready = true;
            assert!(
                status
                    .get("manifest")
                    .and_then(|manifest| manifest.get("indexed_files"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
            );
            assert!(
                status
                    .get("manifest")
                    .and_then(|manifest| manifest.get("regex_weight_table_version"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
            );
            assert_eq!(
                status
                    .get("manifest")
                    .and_then(|manifest| manifest.get("regex_status"))
                    .and_then(Value::as_str),
                Some("ready")
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ready, "expected daemon index to become ready");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn uninstall_stops_workspace_services_and_stale_hooks_cannot_restart_them() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let root = dir.path().to_str().unwrap();
    write_repo_fixture(dir.path());
    init_repo(dir.path());
    suite_cmd()
        .env("HOME", home.path())
        .args(["setup", "--root", root, "--runtime", "claude", "--yes"])
        .assert()
        .success();
    let config_path = dir.path().join(".packet28/daemon/hook-runtime-v1.json");
    let config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    let port = config["http_hook_port"].as_u64().unwrap() as u16;
    assert!(TcpListener::bind(("127.0.0.1", port)).is_err());

    let output = suite_cmd()
        .env("HOME", home.path())
        .args(["uninstall", "--root", root])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.contains("Claude HTTP hook server: stopped"),
        "{output}"
    );
    assert!(output.contains("packet28d: stopped"), "{output}");
    assert!(TcpListener::bind(("127.0.0.1", port)).is_ok());
    for event in ["SessionStart", "SubagentStart", "SubagentStop", "Stop"] {
        suite_cmd()
            .env("HOME", home.path())
            .args(["hook", "claude", "--root", root])
            .write_stdin(
                serde_json::json!({"hook_event_name": event, "session_id": "stale"}).to_string(),
            )
            .assert()
            .success()
            .stdout("");
    }
    assert!(TcpListener::bind(("127.0.0.1", port)).is_ok());
    let stopped = std::time::Instant::now();
    while dir.path().join(".packet28/daemon/runtime.json").exists()
        && stopped.elapsed() < Duration::from_secs(10)
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!dir.path().join(".packet28/daemon/runtime.json").exists());
    let settings: Value =
        serde_json::from_slice(&fs::read(dir.path().join(".claude/settings.json")).unwrap())
            .unwrap();
    assert!(settings.get("hooks").is_none());
    let mcp: Value =
        serde_json::from_slice(&fs::read(dir.path().join(".mcp.json")).unwrap()).unwrap();
    assert!(mcp["mcpServers"].get("packet28").is_none());
    suite_cmd()
        .env("HOME", home.path())
        .args(["uninstall", "--root", root])
        .assert()
        .success();
}
