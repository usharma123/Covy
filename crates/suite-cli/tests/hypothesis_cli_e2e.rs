use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::sync::OnceLock;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn ensure_packet28d_built() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let status = ProcessCommand::new("cargo")
            .args(["build", "-p", "packet28d"])
            .status()
            .unwrap();
        assert!(status.success(), "failed to build packet28d");
    });
}

fn git(root: &Path, args: &[&str]) {
    let status = ProcessCommand::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed with {status}", args);
}

fn init_repo(root: &Path) {
    git(root, &["init"]);
}

fn write_repo_fixture(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("auth.rs"),
        r#"
struct AuthCache;

fn invalidate_auth_cache() {}
"#,
    )
    .unwrap();
}

#[test]
fn test_hypothesis_cli_tracks_active_assumptions() {
    ensure_packet28d_built();
    let root = TempDir::new().unwrap();
    init_repo(root.path());
    write_repo_fixture(root.path());
    let task_id = "task-hypothesis-smoke";

    suite_cmd()
        .current_dir(root.path())
        .args([
            "hypothesis",
            "add",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            task_id,
            "--id",
            "auth-cache",
            "--path",
            "src/auth.rs",
            "--symbol",
            "AuthCache",
            "--artifact-id",
            "artifact-auth-cache",
            "--json",
            "Auth cache invalidation is the regression source",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\":\"active\""))
        .stdout(predicate::str::contains(
            "\"decision_id\":\"hypothesis:auth-cache\"",
        ));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "hypothesis",
            "list",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            task_id,
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"id\":\"auth-cache\""))
        .stdout(predicate::str::contains(
            "\"related_paths\":[\"src/auth.rs\"]",
        ))
        .stdout(predicate::str::contains(
            "\"related_symbols\":[\"AuthCache\"]",
        ))
        .stdout(predicate::str::contains(
            "\"related_artifact_ids\":[\"artifact-auth-cache\"]",
        ))
        .stdout(predicate::str::contains("Auth cache invalidation"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "hypothesis",
            "reject",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            task_id,
            "auth-cache",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("hypothesis auth-cache rejected"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "hypothesis",
            "list",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            task_id,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("active_hypotheses=0"));

    suite_cmd()
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
