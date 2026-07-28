#[path = "support/shard.rs"]
mod shard;

use predicates::prelude::*;
use shard::covy_cmd;
use tempfile::TempDir;

#[test]
fn test_shard_update_ingests_jsonl_timings() {
    let dir = TempDir::new().unwrap();
    let jsonl = dir.path().join("timings.jsonl");
    let timings_bin = dir.path().join("testtimings.bin");
    std::fs::write(
        &jsonl,
        "{\"test_id\":\"com.foo.BarTest\",\"duration_ms\":1200}\n{\"test_id\":\"tests/test_mod.py::test_one\",\"duration_ms\":900}\n",
    )
    .unwrap();

    covy_cmd()
        .args([
            "shard",
            "update",
            "--timings-jsonl",
            jsonl.to_str().unwrap(),
            "--timings",
            timings_bin.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tests_updated\": 2"));

    let bytes = std::fs::read(&timings_bin).unwrap();
    let timings = suite_foundation_core::cache::deserialize_test_timings(&bytes).unwrap();
    assert_eq!(timings.duration_ms.get("com.foo.BarTest"), Some(&1200));
    assert_eq!(
        timings.duration_ms.get("tests/test_mod.py::test_one"),
        Some(&900)
    );
}

#[test]
fn test_shard_update_ingests_junit_xml_timings() {
    let dir = TempDir::new().unwrap();
    let junit = dir.path().join("junit.xml");
    let timings_bin = dir.path().join("testtimings.bin");
    std::fs::write(
        &junit,
        r#"<testsuite><testcase classname="com.foo.BarTest" name="testOne" time="0.250"/></testsuite>"#,
    )
    .unwrap();

    covy_cmd()
        .args([
            "shard",
            "update",
            "--junit-xml",
            junit.to_str().unwrap(),
            "--timings",
            timings_bin.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tests_updated\": 1"));

    let bytes = std::fs::read(&timings_bin).unwrap();
    let timings = suite_foundation_core::cache::deserialize_test_timings(&bytes).unwrap();
    assert_eq!(
        timings.duration_ms.get("com.foo.BarTest.testOne"),
        Some(&250)
    );
}

#[test]
fn test_shard_update_ingests_junit_xml_timings_by_class() {
    let dir = TempDir::new().unwrap();
    let junit = dir.path().join("junit.xml");
    let timings_bin = dir.path().join("testtimings.bin");
    std::fs::write(
        &junit,
        r#"<testsuite>
            <testcase classname="com.foo.BarTest" name="testOne" time="0.250"/>
            <testcase classname="com.foo.BarTest" name="testTwo" time="0.150"/>
          </testsuite>"#,
    )
    .unwrap();

    covy_cmd()
        .args([
            "shard",
            "update",
            "--junit-xml",
            junit.to_str().unwrap(),
            "--timings",
            timings_bin.to_str().unwrap(),
            "--junit-id-granularity",
            "class",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tests_updated\": 1"));

    let bytes = std::fs::read(&timings_bin).unwrap();
    let timings = suite_foundation_core::cache::deserialize_test_timings(&bytes).unwrap();
    assert_eq!(timings.duration_ms.get("com.foo.BarTest"), Some(&400));
    assert!(!timings.duration_ms.contains_key("com.foo.BarTest.testOne"));
}

#[test]
fn test_shard_update_rejects_invalid_junit_id_granularity() {
    let dir = TempDir::new().unwrap();
    let jsonl = dir.path().join("timings.jsonl");
    let timings_bin = dir.path().join("testtimings.bin");
    std::fs::write(
        &jsonl,
        "{\"test_id\":\"com.foo.BarTest\",\"duration_ms\":1200}\n",
    )
    .unwrap();

    covy_cmd()
        .args([
            "shard",
            "update",
            "--timings-jsonl",
            jsonl.to_str().unwrap(),
            "--timings",
            timings_bin.to_str().unwrap(),
            "--junit-id-granularity",
            "suite",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value"));
}
