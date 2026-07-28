use assert_cmd::Command;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn ensure_packet28d_built() {
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

pub fn setup_changed_repo(root: &Path) {
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

pub fn task_spec_with_file_watch(
    root: &Path,
    task_id: &str,
    steps: Vec<Value>,
    watch_kind: &str,
) -> Value {
    json!({
        "task_id": task_id,
        "sequence": {
            "steps": steps,
            "budget": {},
            "reactive": {
                "enabled": true,
                "task_id": task_id,
                "append_focused_map": true
            }
        },
        "watches": [
            {
                "kind": watch_kind,
                "task_id": task_id,
                "root": root,
                "paths": ["src"],
                "include_globs": ["src/**"],
                "exclude_globs": []
            }
        ]
    })
}

pub fn write_task_spec(path: &Path, spec: Value) {
    fs::write(path, serde_json::to_string_pretty(&spec).unwrap()).unwrap();
}
