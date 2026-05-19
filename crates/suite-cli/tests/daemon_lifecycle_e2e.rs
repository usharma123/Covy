use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
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
