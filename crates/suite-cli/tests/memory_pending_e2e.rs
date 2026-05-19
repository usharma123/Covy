use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_memory_pending_extraction_queue_processes_into_memory() {
    let home = TempDir::new().unwrap();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "pending",
            "enqueue",
            "- Packet28 pending extraction stores durable local facts",
            "--project",
            "coverage-a",
            "--tool-name",
            "Bash",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"project\":\"coverage-a\""))
        .stdout(predicate::str::contains("\"tool_name\":\"Bash\""));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "pending", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("durable local facts"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "pending", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pending_extraction_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "pending", "process", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pending_count\":1"))
        .stdout(predicate::str::contains("\"extracted_count\":1"))
        .stdout(predicate::str::contains("\"deleted_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "pending", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pending_extraction_count\":0"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "durable local facts",
            "--project",
            "coverage-a",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "pending extraction stores durable",
        ))
        .stdout(predicate::str::contains("pending-extraction:Bash"));
}
