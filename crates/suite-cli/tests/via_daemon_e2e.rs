use assert_cmd::Command;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
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

fn fixture(rel: &str) -> String {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    workspace
        .join("tests")
        .join("fixtures")
        .join(rel)
        .to_string_lossy()
        .to_string()
}

fn write_manifest(path: &Path) {
    let line = format!(
        "{{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    fs::write(path, line).unwrap();
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

fn setup_changed_repo(root: &Path) {
    write_repo_fixture(root);
    git(root, &["init"]);
    git(root, &["add", "src/alpha.rs", "src/beta.rs"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "init",
        ],
    );
    fs::write(
        root.join("src/alpha.rs"),
        r#"
use crate::beta::Beta;

fn alpha() -> i32 { 2 }
struct Alpha;
"#,
    )
    .unwrap();
    git(root, &["add", "src/alpha.rs"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "change alpha",
        ],
    );
}

fn init_repo(root: &Path) {
    git(root, &["init"]);
}

fn parse_packet_wrapper(output: &[u8], packet_type: &str) -> Value {
    let value: Value = serde_json::from_slice(output).unwrap();
    assert_eq!(
        value.get("schema_version").and_then(Value::as_str),
        Some("suite.packet.v1")
    );
    assert_eq!(
        value.get("packet_type").and_then(Value::as_str),
        Some(packet_type)
    );
    assert!(value.get("packet").is_some());
    value
}

fn packet_payload(wrapper: &Value) -> &Value {
    wrapper
        .get("packet")
        .and_then(|packet| packet.get("payload"))
        .expect("packet.payload should exist")
}

#[test]
#[cfg(unix)]
fn test_via_daemon_cli_diff_analyze_matches_packet_shape() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let local_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let via_daemon_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let local = parse_packet_wrapper(&local_output, "suite.diff.analyze.v1");
    let remote = parse_packet_wrapper(&via_daemon_output, "suite.diff.analyze.v1");
    assert_eq!(
        packet_payload(&local)
            .get("gate_result")
            .and_then(|value| value.get("passed"))
            .and_then(Value::as_bool),
        packet_payload(&remote)
            .get("gate_result")
            .and_then(|value| value.get("passed"))
            .and_then(Value::as_bool)
    );
    assert_eq!(
        packet_payload(&local).get("diffs"),
        packet_payload(&remote).get("diffs")
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_via_daemon_cli_uses_explicit_daemon_root_for_map_repo() {
    ensure_packet28d_built();
    let daemon_root = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    write_repo_fixture(daemon_root.path());
    init_repo(daemon_root.path());
    write_repo_fixture(repo_root.path());

    let output = suite_cmd()
        .current_dir(repo_root.path())
        .args([
            "--via-daemon",
            "--daemon-root",
            daemon_root.path().to_str().unwrap(),
            "map",
            "repo",
            "--repo-root",
            repo_root.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.map.repo.v1");
    assert!(packet_payload(&value).get("files_ranked").is_some());
    assert!(daemon_root
        .path()
        .join(".packet28/daemon/runtime.json")
        .exists());
    assert!(!repo_root
        .path()
        .join(".packet28/daemon/runtime.json")
        .exists());

    suite_cmd()
        .args([
            "daemon",
            "stop",
            "--root",
            daemon_root.path().to_str().unwrap(),
        ])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_via_daemon_cli_honors_daemon_root_env() {
    ensure_packet28d_built();
    let daemon_root = TempDir::new().unwrap();
    let work_root = TempDir::new().unwrap();
    init_repo(daemon_root.path());
    init_repo(work_root.path());
    let manifest = work_root.path().join("manifest.jsonl");
    let testmap = work_root.path().join("testmap.bin");
    let timings = work_root.path().join("testtimings.bin");
    write_manifest(&manifest);

    suite_cmd()
        .current_dir(work_root.path())
        .env("PACKET28_DAEMON_ROOT", daemon_root.path().to_str().unwrap())
        .args([
            "--via-daemon",
            "test",
            "map",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
            "--timings-output",
            timings.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    assert!(daemon_root
        .path()
        .join(".packet28/daemon/runtime.json")
        .exists());
    assert!(!work_root
        .path()
        .join(".packet28/daemon/runtime.json")
        .exists());

    suite_cmd()
        .args([
            "daemon",
            "stop",
            "--root",
            daemon_root.path().to_str().unwrap(),
        ])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_via_daemon_cli_diff_wrapper_surfaces_cache_hit() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());

    let first = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "--daemon-root",
            dir.path().to_str().unwrap(),
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "--daemon-root",
            dir.path().to_str().unwrap(),
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let first_value: Value = serde_json::from_slice(&first).unwrap();
    let second_value: Value = serde_json::from_slice(&second).unwrap();
    assert_eq!(
        first_value.get("cache_hit").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        second_value.get("cache_hit").and_then(Value::as_bool),
        Some(true)
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_via_daemon_cli_test_map_and_shard_auto_start() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    let timings = dir.path().join("testtimings.bin");
    let tasks = dir.path().join("tasks.json");
    write_manifest(&manifest);
    fs::write(
        &tasks,
        serde_json::to_string_pretty(&json!({
            "schema_version": 1,
            "tasks": [
                {"id": "com.foo.BarTest", "selector": "com.foo.BarTest", "est_ms": 1200, "tags": ["unit"]},
                {"id": "com.foo.BazTest", "selector": "com.foo.BazTest", "est_ms": 900, "tags": ["unit"]}
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let map_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "test",
            "map",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
            "--timings-output",
            timings.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let map_value: Value = serde_json::from_slice(&map_output).unwrap();
    assert_eq!(map_value.get("records").and_then(Value::as_u64), Some(1));
    assert!(dir.path().join(".packet28/daemon/runtime.json").exists());
    assert!(testmap.exists());
    assert!(timings.exists());

    let shard_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "test",
            "shard",
            "--shards",
            "2",
            "--tasks-json",
            tasks.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shard_value: Value = serde_json::from_slice(&shard_output).unwrap();
    assert_eq!(
        shard_value
            .get("shards")
            .and_then(Value::as_array)
            .map(|value| value.len()),
        Some(2)
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
