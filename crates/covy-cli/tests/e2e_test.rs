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
fn test_help() {
    covy_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Universal code coverage tool"));
}

#[test]
fn test_report_no_data() {
    covy_cmd()
        .args(["report", "--input", "/nonexistent/path.bin"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No coverage data found"));
}

#[test]
fn test_report_min_coverage_fail() {
    let dir = TempDir::new().unwrap();
    let state_file = dir.path().join("state.bin");

    covy_cmd()
        .args([
            "ingest",
            &fixture("lcov/basic.info"),
            "--output",
            state_file.to_str().unwrap(),
        ])
        .assert()
        .success();

    covy_cmd()
        .args([
            "report",
            "--input",
            state_file.to_str().unwrap(),
            "--min-coverage",
            "95.0",
            "--color",
            "never",
        ])
        .assert()
        .code(1);
}

#[test]
fn test_diff_returns_failure_exit_code_when_gate_fails() {
    covy_cmd()
        .args([
            "diff",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--fail-under-total",
            "101",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--report",
            "json",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"passed\": false"));
}

#[test]
fn test_testmap_build_writes_test_to_files_index() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let output = dir.path().join("testmap.bin");

    let line = format!(
        "{{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    std::fs::write(&manifest, line).unwrap();

    covy_cmd()
        .args([
            "testmap",
            "build",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ])
        .assert()
        .success();

    let bytes = std::fs::read(&output).unwrap();
    let map = suite_foundation_core::cache::deserialize_testmap(&bytes).unwrap();
    assert!(map.test_to_files.contains_key("com.foo.BarTest"));
    assert!(!map.test_to_files["com.foo.BarTest"].is_empty());
    let covered_file = map.test_to_files["com.foo.BarTest"]
        .iter()
        .next()
        .unwrap()
        .clone();
    assert!(map.file_to_tests.contains_key(&covered_file));
    assert!(map.file_to_tests[&covered_file].contains("com.foo.BarTest"));
    assert!(map.metadata.generated_at > 0);
}

#[test]
fn test_impact_json_runs_with_diff_integration() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");

    let line = format!(
        "{{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    std::fs::write(&manifest, line).unwrap();

    covy_cmd()
        .args([
            "testmap",
            "build",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success();

    covy_cmd()
        .args([
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"selected_tests\""));
}

#[test]
fn test_impact_record_builds_v2_testmap() {
    let dir = TempDir::new().unwrap();
    let per_test_dir = dir.path().join("per-test-lcov");
    std::fs::create_dir_all(&per_test_dir).unwrap();
    std::fs::copy(
        fixture("lcov/basic.info"),
        per_test_dir.join("com.foo.BarTest.info"),
    )
    .unwrap();

    let testmap = dir.path().join("testmap.bin");
    let summary = dir.path().join("testmap.json");

    covy_cmd()
        .args([
            "impact",
            "record",
            "--base-ref",
            "HEAD",
            "--out",
            testmap.to_str().unwrap(),
            "--per-test-lcov-dir",
            per_test_dir.to_str().unwrap(),
            "--summary-json",
            summary.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(testmap.exists());
    assert!(summary.exists());

    let bytes = std::fs::read(&testmap).unwrap();
    let map = suite_foundation_core::cache::deserialize_testmap(&bytes).unwrap();
    assert_eq!(
        map.metadata.schema_version,
        suite_foundation_core::cache::TESTMAP_SCHEMA_VERSION
    );
    assert!(!map.tests.is_empty());
    assert!(!map.file_index.is_empty());
    assert_eq!(map.tests.len(), map.coverage.len());
}

#[test]
fn test_impact_plan_outputs_stable_json_schema() {
    let dir = TempDir::new().unwrap();
    let per_test_dir = dir.path().join("per-test-lcov");
    std::fs::create_dir_all(&per_test_dir).unwrap();
    std::fs::copy(
        fixture("lcov/basic.info"),
        per_test_dir.join("com.foo.BarTest.info"),
    )
    .unwrap();

    let testmap = dir.path().join("testmap.bin");

    covy_cmd()
        .args([
            "impact",
            "record",
            "--base-ref",
            "HEAD",
            "--out",
            testmap.to_str().unwrap(),
            "--per-test-lcov-dir",
            per_test_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    covy_cmd()
        .args([
            "impact",
            "plan",
            "--base-ref",
            "HEAD",
            "--head-ref",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
            "--max-tests",
            "5",
            "--target-coverage",
            "0.9",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"changed_lines_total\""))
        .stdout(predicate::str::contains(
            "\"changed_lines_covered_by_plan\"",
        ))
        .stdout(predicate::str::contains("\"plan_coverage_pct\""))
        .stdout(predicate::str::contains("\"tests\""))
        .stdout(predicate::str::contains("\"uncovered_blocks\""))
        .stdout(predicate::str::contains("\"next_command\""));
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
fn test_impact_print_command_outputs_helper() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");

    let line = format!(
        "{{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    std::fs::write(&manifest, line).unwrap();

    covy_cmd()
        .args([
            "testmap",
            "build",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success();

    covy_cmd()
        .args([
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
            "--print-command",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("echo \"no impacted tests\""));
}

#[test]
fn test_impact_legacy_mode_still_works_without_warning_noise() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");

    let line = format!(
        "{{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    std::fs::write(&manifest, line).unwrap();

    covy_cmd()
        .args([
            "testmap",
            "build",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success();

    covy_cmd()
        .args([
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("deprecated").not());
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
fn test_merge_non_strict_skips_corrupt_artifacts() {
    let dir = TempDir::new().unwrap();
    let bad = dir.path().join("bad.bin");
    std::fs::write(&bad, b"broken").unwrap();

    covy_cmd()
        .args([
            "merge",
            "--coverage",
            bad.to_str().unwrap(),
            "--strict",
            "false",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"skipped_inputs\": 1"))
        .stdout(predicate::str::contains("\"strict_mode\": false"))
        .stdout(predicate::str::contains("\"output_coverage_path\""));
}

#[test]
fn test_merge_strict_fails_on_corrupt_artifacts() {
    let dir = TempDir::new().unwrap();
    let bad = dir.path().join("bad.bin");
    std::fs::write(&bad, b"broken").unwrap();

    covy_cmd()
        .args([
            "merge",
            "--coverage",
            bad.to_str().unwrap(),
            "--strict",
            "true",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Failed to merge coverage input"));
}

#[test]
fn test_merge_writes_output_coverage_state() {
    let dir = TempDir::new().unwrap();
    let shard = dir.path().join("shard.bin");
    let merged = dir.path().join("merged.bin");

    covy_cmd()
        .args([
            "ingest",
            &fixture("lcov/basic.info"),
            "--output",
            shard.to_str().unwrap(),
        ])
        .assert()
        .success();

    covy_cmd()
        .args([
            "merge",
            "--coverage",
            shard.to_str().unwrap(),
            "--output-coverage",
            merged.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    assert!(merged.exists());
}

#[test]
fn test_merge_writes_output_issues_state() {
    let dir = TempDir::new().unwrap();
    let shard = dir.path().join("issues-shard.bin");
    let merged = dir.path().join("issues-merged.bin");

    covy_cmd()
        .current_dir(dir.path())
        .args(["ingest", "--issues", &fixture("sarif/basic.sarif")])
        .assert()
        .success();

    std::fs::copy(dir.path().join(".covy/state/issues.bin"), &shard).unwrap();

    covy_cmd()
        .args([
            "merge",
            "--issues",
            shard.to_str().unwrap(),
            "--output-issues",
            merged.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    assert!(merged.exists());
}

#[test]
fn test_testmap_build_supports_python_language_metadata() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let output = dir.path().join("testmap.bin");

    let line = format!(
        "{{\"test_id\":\"tests/test_mod.py::test_case\",\"language\":\"python\",\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    std::fs::write(&manifest, line).unwrap();

    covy_cmd()
        .args([
            "testmap",
            "build",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ])
        .assert()
        .success();

    let bytes = std::fs::read(&output).unwrap();
    let map = suite_foundation_core::cache::deserialize_testmap(&bytes).unwrap();
    assert_eq!(
        map.test_language["tests/test_mod.py::test_case"],
        "python".to_string()
    );
}

#[test]
fn test_testmap_build_writes_timings_output() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let output = dir.path().join("testmap.bin");
    let timings_output = dir.path().join("testtimings.bin");

    let line = format!(
        "{{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"duration_ms\":1234,\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    std::fs::write(&manifest, line).unwrap();

    covy_cmd()
        .args([
            "testmap",
            "build",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--timings-output",
            timings_output.to_str().unwrap(),
        ])
        .assert()
        .success();

    let bytes = std::fs::read(&timings_output).unwrap();
    let timings = suite_foundation_core::cache::deserialize_test_timings(&bytes).unwrap();
    assert_eq!(timings.duration_ms.get("com.foo.BarTest"), Some(&1234));
    assert_eq!(timings.sample_count.get("com.foo.BarTest"), Some(&1));
}

#[test]
fn test_testmap_build_json_outputs_summary() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let output = dir.path().join("testmap.bin");

    let line = format!(
        "{{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    std::fs::write(&manifest, line).unwrap();

    covy_cmd()
        .args([
            "testmap",
            "build",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"manifest_files\""))
        .stdout(predicate::str::contains("\"output_testmap_path\""));
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

#[test]
fn test_impact_and_shard_and_testmap_schema_flags() {
    covy_cmd()
        .args(["impact", "run", "--schema"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tests\""))
        .stdout(predicate::str::contains("\"changed_lines_total\""))
        .stdout(predicate::str::contains("\"total_changed_lines\"").not());

    covy_cmd()
        .args(["impact", "record", "--schema"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"example_line\""));

    covy_cmd()
        .args(["shard", "plan", "--schema"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tasks_json\""))
        .stdout(predicate::str::contains("\"impact_json\""));

    covy_cmd()
        .args(["testmap", "build", "--schema"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"test_id\""))
        .stdout(predicate::str::contains("\"coverage_report\""));
}

#[test]
fn test_init_defaults_to_cwd_not_git_root() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("README.md"), "init\n").unwrap();
    setup_git_repo(dir.path());

    let sub = dir.path().join("subproject");
    std::fs::create_dir_all(&sub).unwrap();

    covy_cmd()
        .current_dir(&sub)
        .args(["init"])
        .assert()
        .success();

    assert!(sub.join("covy.toml").exists());
    assert!(sub.join(".covy/state").exists());
    assert!(sub.join(".covy/cache").exists());
    assert!(!dir.path().join("covy.toml").exists());
}
