use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
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
