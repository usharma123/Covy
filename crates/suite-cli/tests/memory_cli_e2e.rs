use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_memory_cli_store_recall_uses_sqlite_home_db() {
    let home = TempDir::new().unwrap();
    let db_path = home.path().join(".packet28").join("packet28.db");
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 remembers local context",
            "--tags",
            "packet28,local",
            "--topic",
            "parity",
            "--importance",
            "high",
            "--keywords",
            "context,local",
            "--project",
            "coverage-a",
            "--source",
            "cli-test",
            "--raw",
            "verbatim context",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"content\""))
        .stdout(predicate::str::contains("\"topic\":\"parity\""))
        .stdout(predicate::str::contains("\"importance\":\"high\""))
        .stdout(predicate::str::contains("\"project\":\"coverage-a\""))
        .stdout(predicate::str::contains("\"source\":\"cli-test\""));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "invalid importance should fail",
            "--importance",
            "urgent",
            "--json",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unsupported memory importance"));

    assert!(db_path.exists());
    let conn = Connection::open(&db_path).unwrap();
    let fts_tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('memories_fts', 'feedback_fts')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fts_tables, 2);

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "recall", "local context", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"recall_score\""))
        .stdout(predicate::str::contains("Packet28 remembers local context"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "recall", "local context", "--format", "toon"])
        .assert()
        .success()
        .stdout(predicate::str::contains("memories[1]{score,id,topic"))
        .stdout(predicate::str::contains("Packet28 remembers local context"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "recall", "local context", "--format", "detail"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[score:"))
        .stdout(predicate::str::contains("topic:"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "wakeup",
            "--query",
            "local",
            "--project",
            "coverage-a",
            "--max-tokens",
            "40",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\":\"packet28.wakeup.v1\""))
        .stdout(predicate::str::contains("\"project\":\"coverage-a\""))
        .stdout(predicate::str::contains("\"max_tokens\":40"))
        .stdout(predicate::str::contains("\"pack\""))
        .stdout(predicate::str::contains("\"included_items\""))
        .stdout(predicate::str::contains("Packet28 remembers local context"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "update",
            "1",
            "--content",
            "Packet28 remembers updated local context",
            "--topic",
            "updated-parity",
            "--project",
            "coverage-b",
            "--importance",
            "CRITICAL",
            "--source",
            "cli-update",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("updated local context"))
        .stdout(predicate::str::contains("\"topic\":\"updated-parity\""))
        .stdout(predicate::str::contains("\"importance\":\"critical\""))
        .stdout(predicate::str::contains("\"project\":\"coverage-b\""))
        .stdout(predicate::str::contains("\"source\":\"cli-update\""));

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "topics", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"topic\":\"updated-parity\""))
        .stdout(predicate::str::contains("\"memory_count\":1"));
}
