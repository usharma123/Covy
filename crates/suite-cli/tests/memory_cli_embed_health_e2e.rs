use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn store_memory(home: &TempDir, content: &str) {
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            content,
            "--topic",
            "updated-parity",
            "--keywords",
            "second,context",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success();
}

#[test]
fn test_memory_cli_consolidate_embed_health_and_forget_topic() {
    let home = TempDir::new().unwrap();
    let db_path = home.path().join(".packet28").join("packet28.db");

    store_memory(&home, "Packet28 remembers updated local context");
    store_memory(&home, "Packet28 remembers a second local context");

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "consolidate",
            "--topic",
            "updated-parity",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\":\"consolidated\""))
        .stdout(predicate::str::contains("\"source_count\":2"))
        .stdout(predicate::str::contains("Consolidated memory for topic"));

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"memory_count\":1"));

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "embed", "--all", "--dimensions", "16", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"model\":\"packet28-local-lexical-v2\"",
        ))
        .stdout(predicate::str::contains("\"dimensions\":16"))
        .stdout(predicate::str::contains("\"embedded_count\":1"));
    let conn = Connection::open(&db_path).unwrap();
    let embedding_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_embeddings", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(embedding_rows, 1);

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "updated second",
            "--project",
            "coverage-b",
            "--format",
            "toon",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("memories[1]{score,id,topic"))
        .stdout(predicate::str::contains("Consolidated memory for topic"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "updted secnd",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Consolidated memory for topic"))
        .stdout(predicate::str::contains("\"recall_score\""));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "health",
            "--topic",
            "updated-parity",
            "--consolidation-threshold",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"topic_filter\":\"updated-parity\"",
        ))
        .stdout(predicate::str::contains("\"total_memories\":1"))
        .stdout(predicate::str::contains(
            "\"topics_needing_consolidation\":1",
        ))
        .stdout(predicate::str::contains("\"avg_weight\""))
        .stdout(predicate::str::contains("\"avg_access_count\""))
        .stdout(predicate::str::contains("\"consolidation_needed\":true"));

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "forget", "--topic", "updated-parity", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"deleted\":1"));
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM memory_chunks", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM memory_embeddings", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}
