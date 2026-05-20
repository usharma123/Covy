use assert_cmd::Command;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
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

#[test]
#[cfg(unix)]
fn test_daemon_task_submit_returns_watch_id_and_watch_list() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let spec_path = dir.path().join("task-spec.json");
    fs::write(
        &spec_path,
        serde_json::to_string_pretty(&json!({
            "task_id": "task-watch",
            "sequence": {
                "steps": [
                    {
                        "id": "map",
                        "target": "mapy.repo",
                        "depends_on": [],
                        "input_packets": [],
                        "policy_context": {
                            "task_id": "task-watch"
                        },
                        "reducer_input": {
                            "repo_root": dir.path(),
                            "focus_paths": [],
                            "focus_symbols": [],
                            "max_files": 10,
                            "max_symbols": 20,
                            "include_tests": false
                        },
                        "budget": {}
                    }
                ],
                "budget": {},
                "reactive": {
                    "enabled": true,
                    "task_id": "task-watch",
                    "append_focused_map": true
                }
            },
            "watches": [
                {
                    "kind": "file",
                    "task_id": "task-watch",
                    "root": dir.path(),
                    "paths": ["src"],
                    "include_globs": ["src/**"],
                    "exclude_globs": []
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let submit_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "submit",
            "--root",
            dir.path().to_str().unwrap(),
            "--spec",
            spec_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let submit: Value = serde_json::from_slice(&submit_output).unwrap();
    let watch_id = submit
        .get("watches")
        .and_then(Value::as_array)
        .and_then(|watches| watches.first())
        .and_then(|watch| watch.get("watch_id"))
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    let watches_output = suite_cmd()
        .args([
            "daemon",
            "watch",
            "list",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-watch",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let watches: Value = serde_json::from_slice(&watches_output).unwrap();
    assert_eq!(
        watches
            .as_array()
            .and_then(|watches| watches.first())
            .and_then(|watch| watch.get("watch_id"))
            .and_then(Value::as_str),
        Some(watch_id.as_str())
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
