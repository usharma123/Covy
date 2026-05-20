use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn covy_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("covy")
}

#[test]
fn test_shard_plan_json_and_file_outputs() {
    let dir = TempDir::new().unwrap();
    let tests_file = dir.path().join("tests.txt");
    let out_dir = dir.path().join("shards");
    std::fs::write(&tests_file, "t1\nt2\nt3\n").unwrap();

    covy_cmd()
        .args([
            "shard",
            "plan",
            "--shards",
            "2",
            "--tests-file",
            tests_file.to_str().unwrap(),
            "--write-files",
            out_dir.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"shards\""))
        .stdout(predicate::str::contains("\"imbalance_ratio\""))
        .stdout(predicate::str::contains("\"parallel_efficiency\""));

    assert!(out_dir.join("shard-1.txt").exists());
    assert!(out_dir.join("shard-2.txt").exists());
}

#[test]
fn test_shard_plan_supports_python_nodeids() {
    let dir = TempDir::new().unwrap();
    let tests_file = dir.path().join("py-tests.txt");
    std::fs::write(
        &tests_file,
        "tests/test_mod.py::test_one\ntests/test_mod.py::test_two\n",
    )
    .unwrap();

    covy_cmd()
        .args([
            "shard",
            "plan",
            "--shards",
            "2",
            "--tests-file",
            tests_file.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("tests/test_mod.py::test_one"))
        .stdout(predicate::str::contains("tests/test_mod.py::test_two"));
}

#[test]
fn test_shard_plan_supports_tasks_json() {
    let dir = TempDir::new().unwrap();
    let tasks_file = dir.path().join("tasks.json");
    let payload = serde_json::json!({
        "schema_version": 1,
        "tasks": [
            {"id": "com.foo.BarTest", "selector": "com.foo.BarTest", "est_ms": 1000},
            {"id": "tests/test_mod.py::test_one", "selector": "tests/test_mod.py::test_one", "est_ms": 800}
        ]
    });
    std::fs::write(&tasks_file, serde_json::to_string(&payload).unwrap()).unwrap();

    covy_cmd()
        .args([
            "shard",
            "plan",
            "--shards",
            "2",
            "--tasks-json",
            tasks_file.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("com.foo.BarTest"))
        .stdout(predicate::str::contains("tests/test_mod.py::test_one"));
}

#[test]
fn test_shard_plan_accepts_whale_lpt_algorithm() {
    let dir = TempDir::new().unwrap();
    let tests_file = dir.path().join("tests.txt");
    std::fs::write(&tests_file, "com.foo.A\ncom.foo.B\ncom.foo.C\n").unwrap();

    covy_cmd()
        .args([
            "shard",
            "plan",
            "--shards",
            "2",
            "--tests-file",
            tests_file.to_str().unwrap(),
            "--algorithm",
            "whale-lpt",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"shards\""))
        .stdout(predicate::str::contains("com.foo.A"));
}

#[test]
fn test_shard_plan_rejects_invalid_algorithm() {
    let dir = TempDir::new().unwrap();
    let tests_file = dir.path().join("tests.txt");
    std::fs::write(&tests_file, "com.foo.A\n").unwrap();

    covy_cmd()
        .args([
            "shard",
            "plan",
            "--shards",
            "1",
            "--tests-file",
            tests_file.to_str().unwrap(),
            "--algorithm",
            "bad-algo",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn test_shard_plan_pr_tier_excludes_slow_tagged_tasks() {
    let dir = TempDir::new().unwrap();
    let tasks_file = dir.path().join("tasks.json");
    let payload = serde_json::json!({
        "schema_version": 1,
        "tasks": [
            {"id": "fast-test", "selector": "fast-test", "est_ms": 1000, "tags": ["unit"]},
            {"id": "slow-test", "selector": "slow-test", "est_ms": 2000, "tags": ["slow"]}
        ]
    });
    std::fs::write(&tasks_file, serde_json::to_string(&payload).unwrap()).unwrap();

    covy_cmd()
        .args([
            "shard",
            "plan",
            "--shards",
            "2",
            "--tasks-json",
            tasks_file.to_str().unwrap(),
            "--tier",
            "pr",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("fast-test"))
        .stdout(predicate::str::contains("slow-test").not());
}

#[test]
fn test_shard_plan_nightly_tier_keeps_slow_tagged_tasks() {
    let dir = TempDir::new().unwrap();
    let tasks_file = dir.path().join("tasks.json");
    let payload = serde_json::json!({
        "schema_version": 1,
        "tasks": [
            {"id": "fast-test", "selector": "fast-test", "est_ms": 1000, "tags": ["unit"]},
            {"id": "slow-test", "selector": "slow-test", "est_ms": 2000, "tags": ["slow"]}
        ]
    });
    std::fs::write(&tasks_file, serde_json::to_string(&payload).unwrap()).unwrap();

    covy_cmd()
        .args([
            "shard",
            "plan",
            "--shards",
            "2",
            "--tasks-json",
            tasks_file.to_str().unwrap(),
            "--tier",
            "nightly",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("fast-test"))
        .stdout(predicate::str::contains("slow-test"));
}

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
