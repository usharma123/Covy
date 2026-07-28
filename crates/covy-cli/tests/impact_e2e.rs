#[path = "support/impact.rs"]
mod impact;

use impact::{build_basic_testmap, covy_cmd, fixture};
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn test_impact_json_runs_with_diff_integration() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    build_basic_testmap(&manifest, &testmap);

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
