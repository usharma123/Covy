use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn covy_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("covy")
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

fn setup_git_repo(dir: &Path) {
    let init_status = std::process::Command::new("git")
        .current_dir(dir)
        .args(["init"])
        .status()
        .expect("failed to execute `git init`");
    assert!(
        init_status.success(),
        "`git init` exited with {init_status}"
    );

    let add_status = std::process::Command::new("git")
        .current_dir(dir)
        .args(["add", "README.md"])
        .status()
        .expect("failed to execute `git add README.md`");
    assert!(
        add_status.success(),
        "`git add README.md` exited with {add_status}"
    );

    let commit_status = std::process::Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "init",
        ])
        .status()
        .expect("failed to execute initial git commit");
    assert!(
        commit_status.success(),
        "`git commit -m init` exited with {commit_status}"
    );
}

#[test]
fn test_comment_writes_markdown_artifact() {
    let dir = TempDir::new().unwrap();
    let comment_path = dir.path().join("comment.md");
    std::fs::write(dir.path().join("README.md"), "init\n").unwrap();
    setup_git_repo(dir.path());

    covy_cmd()
        .current_dir(dir.path())
        .args(["ingest", &fixture("lcov/basic.info")])
        .assert()
        .success();

    covy_cmd()
        .current_dir(dir.path())
        .args([
            "comment",
            "--base-ref",
            "HEAD",
            "--head-ref",
            "HEAD",
            "--format",
            "markdown",
            "--out",
            comment_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let content = std::fs::read_to_string(comment_path).unwrap();
    assert!(content.contains("gate:"));
    assert!(content.contains("<!-- covy -->"));
}

#[test]
fn test_annotate_writes_sarif_artifact() {
    let dir = TempDir::new().unwrap();
    let sarif_path = dir.path().join("covy.sarif");
    std::fs::write(dir.path().join("README.md"), "init\n").unwrap();
    setup_git_repo(dir.path());

    covy_cmd()
        .current_dir(dir.path())
        .args(["ingest", &fixture("lcov/basic.info")])
        .assert()
        .success();

    covy_cmd()
        .current_dir(dir.path())
        .args([
            "annotate",
            "--base-ref",
            "HEAD",
            "--head-ref",
            "HEAD",
            "--out",
            sarif_path.to_str().unwrap(),
            "--max-findings",
            "200",
        ])
        .assert()
        .success();

    let content = std::fs::read_to_string(sarif_path).unwrap();
    assert!(content.contains("\"version\": \"2.1.0\""));
    assert!(content.contains("covy/coverage/changed-line-uncovered"));
}

#[test]
fn test_pr_writes_both_artifacts() {
    let dir = TempDir::new().unwrap();
    let comment_path = dir.path().join("comment.md");
    let sarif_path = dir.path().join("covy.sarif");

    std::fs::write(dir.path().join("README.md"), "init\n").unwrap();
    setup_git_repo(dir.path());

    covy_cmd()
        .current_dir(dir.path())
        .args(["ingest", &fixture("lcov/basic.info")])
        .assert()
        .success();

    covy_cmd()
        .current_dir(dir.path())
        .args([
            "pr",
            "--base-ref",
            "HEAD",
            "--head-ref",
            "HEAD",
            "--out-comment",
            comment_path.to_str().unwrap(),
            "--out-sarif",
            sarif_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(comment_path.exists());
    assert!(sarif_path.exists());
}

#[test]
fn test_pr_json_stdout_is_pure_json() {
    let dir = TempDir::new().unwrap();
    let comment_path = dir.path().join("comment.md");
    let sarif_path = dir.path().join("covy.sarif");

    std::fs::write(dir.path().join("README.md"), "init\n").unwrap();
    setup_git_repo(dir.path());

    covy_cmd()
        .current_dir(dir.path())
        .args(["ingest", &fixture("lcov/basic.info")])
        .assert()
        .success();

    covy_cmd()
        .current_dir(dir.path())
        .args([
            "pr",
            "--base-ref",
            "HEAD",
            "--head-ref",
            "HEAD",
            "--output-comment",
            comment_path.to_str().unwrap(),
            "--output-sarif",
            sarif_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"comment\""))
        .stdout(predicate::str::contains("\"sarif\""))
        .stdout(predicate::str::contains("Wrote SARIF").not());
}

#[test]
fn test_github_comment_still_works_without_warning_noise() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("README.md"), "init\n").unwrap();
    setup_git_repo(dir.path());

    covy_cmd()
        .current_dir(dir.path())
        .args([
            "github-comment",
            &fixture("lcov/basic.info"),
            "--dry-run",
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("## Coverage Report"))
        .stderr(predicate::str::contains("deprecated").not());
}

#[test]
fn test_pr_help_shows_canonical_output_flags() {
    covy_cmd()
        .args(["pr", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--output-comment"))
        .stdout(predicate::str::contains("--output-sarif"));
}

#[test]
fn test_pr_typo_hint_prefers_output_comment_canonical() {
    covy_cmd()
        .args([
            "pr",
            "--comment-out",
            "/tmp/x.md",
            "--output-sarif",
            "/tmp/x.sarif",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--output-comment"))
        .stderr(predicate::str::contains("--out-comment").not());
}
