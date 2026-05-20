#[path = "support/shard.rs"]
mod shard;

use predicates::prelude::*;
use shard::covy_cmd;
use tempfile::TempDir;

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
