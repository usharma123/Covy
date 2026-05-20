#[path = "support/shard.rs"]
mod shard;
#[path = "support/shard_plan.rs"]
mod shard_plan;

use predicates::prelude::*;
use shard::covy_cmd;
use shard_plan::{write_basic_tasks_file, write_tests_file, write_tier_tasks_file};
use tempfile::TempDir;

#[test]
fn test_shard_plan_json_and_file_outputs() {
    let dir = TempDir::new().unwrap();
    let tests_file = write_tests_file(dir.path(), "tests.txt", "t1\nt2\nt3\n");
    let out_dir = dir.path().join("shards");

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
    let tests_file = write_tests_file(
        dir.path(),
        "py-tests.txt",
        "tests/test_mod.py::test_one\ntests/test_mod.py::test_two\n",
    );

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
    let tasks_file = write_basic_tasks_file(dir.path());

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
    let tests_file = write_tests_file(dir.path(), "tests.txt", "com.foo.A\ncom.foo.B\ncom.foo.C\n");

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
    let tests_file = write_tests_file(dir.path(), "tests.txt", "com.foo.A\n");

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
    let tasks_file = write_tier_tasks_file(dir.path());

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
    let tasks_file = write_tier_tasks_file(dir.path());

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
