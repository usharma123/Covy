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

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 remembers a second local context",
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
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Foreign project context",
            "--topic",
            "foreign-parity",
            "--project",
            "coverage-foreign",
            "--json",
        ])
        .assert()
        .success();

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

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 can prune low-weight local context",
            "--topic",
            "prune-test",
            "--importance",
            "low",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 preserves high-importance local context during prune",
            "--topic",
            "prune-test",
            "--importance",
            "high",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "decay", "--factor", "0.1", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"decayed_count\":2"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "prune",
            "--threshold",
            "0.6",
            "--dry-run",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"candidate_count\":1"))
        .stdout(predicate::str::contains("\"deleted_count\":0"))
        .stdout(predicate::str::contains("\"skipped_protected_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "prune", "--threshold", "0.6", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"candidate_count\":1"))
        .stdout(predicate::str::contains("\"deleted_count\":1"))
        .stdout(predicate::str::contains("\"skipped_protected_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "recall", "high-importance", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("preserves high-importance"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Access-aware decay keeps frequently recalled context",
            "--topic",
            "access-decay",
            "--importance",
            "medium",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Dormant decay comparison note",
            "--topic",
            "access-decay",
            "--importance",
            "medium",
            "--json",
        ])
        .assert()
        .success();
    for _ in 0..5 {
        suite_cmd()
            .env("HOME", home.path())
            .args([
                "memory",
                "recall",
                "frequently recalled context",
                "--topic",
                "access-decay",
                "--json",
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("frequently recalled context"));
    }
    let accessed_count: i64 = conn
        .query_row(
            "SELECT access_count FROM memories WHERE content LIKE 'Access-aware decay%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(accessed_count, 5);
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "decay", "--factor", "0.5", "--json"])
        .assert()
        .success();
    let accessed_weight: f64 = conn
        .query_row(
            "SELECT weight FROM memories WHERE content LIKE 'Access-aware decay%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let unaccessed_weight: f64 = conn
        .query_row(
            "SELECT weight FROM memories WHERE content LIKE 'Dormant decay%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        accessed_weight > unaccessed_weight,
        "accessed weight {accessed_weight} should exceed unaccessed weight {unaccessed_weight}"
    );
}
