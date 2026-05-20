#[path = "support/diff_analyze.rs"]
mod diff_analyze;

use diff_analyze::{fixture, suite_cmd};
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

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
fn test_diff_analyze_cli_task_id_propagates_focus_to_map_repo() {
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());

    suite_cmd()
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
            "--task-id",
            "task-diff",
            "--json",
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "map",
            "repo",
            "--repo-root",
            ".",
            "--task-id",
            "task-diff",
            "--json",
            "--packet-detail",
            "rich",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.map.repo.v1");
    let files = value
        .get("packet")
        .and_then(|packet| packet.get("files"))
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(
        files
            .first()
            .and_then(|file| file.get("path"))
            .and_then(Value::as_str),
        Some("src/alpha.rs")
    );
    assert!(
        files[0].get("relevance").and_then(Value::as_f64).unwrap()
            > files[1].get("relevance").and_then(Value::as_f64).unwrap()
    );
}
