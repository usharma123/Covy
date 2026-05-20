use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn store_memory(home: &TempDir, content: &str, topic: &str, project: &str) {
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            content,
            "--topic",
            topic,
            "--keywords",
            "second,context",
            "--project",
            project,
            "--json",
        ])
        .assert()
        .success();
}

#[test]
fn test_memory_cli_filters_lists_and_forgets_by_project_scope() {
    let home = TempDir::new().unwrap();

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 remembers updated local context",
            "--tags",
            "packet28,local",
            "--topic",
            "updated-parity",
            "--keywords",
            "context,local",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success();
    store_memory(
        &home,
        "Packet28 remembers a second local context",
        "updated-parity",
        "coverage-b",
    );
    store_memory(
        &home,
        "Foreign project context",
        "foreign-parity",
        "coverage-foreign",
    );

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "context",
            "--topic",
            "updated-parity",
            "--project",
            "coverage-b",
            "--tag",
            "packet28",
            "--keyword",
            "context",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("updated local context"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "Foreign",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::eq("[]\n"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "wakeup",
            "--query",
            "Foreign",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"memories\":[]"))
        .stdout(predicate::str::contains(
            "no Packet28 wake-up context matched",
        ));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "forget", "3", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"deleted\":1"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "list",
            "--topic",
            "updated-parity",
            "--project",
            "coverage-b",
            "--sort",
            "oldest",
            "--all",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Packet28 remembers updated local context",
        ))
        .stdout(predicate::str::contains(
            "Packet28 remembers a second local context",
        ));
}
