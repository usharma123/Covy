use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn store_memory(home: &TempDir, content: &str, topic: &str, importance: &str) {
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            content,
            "--topic",
            topic,
            "--importance",
            importance,
            "--json",
        ])
        .assert()
        .success();
}

#[test]
fn test_memory_maintenance_prune_preserves_high_importance_items() {
    let home = TempDir::new().unwrap();

    store_memory(
        &home,
        "Packet28 can prune low-weight local context",
        "prune-test",
        "low",
    );
    store_memory(
        &home,
        "Packet28 preserves high-importance local context during prune",
        "prune-test",
        "high",
    );

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
}

#[test]
fn test_memory_maintenance_access_aware_decay_rewards_recalled_items() {
    let home = TempDir::new().unwrap();
    let db_path = home.path().join(".packet28").join("packet28.db");

    store_memory(
        &home,
        "Access-aware decay keeps frequently recalled context",
        "access-decay",
        "medium",
    );
    store_memory(
        &home,
        "Dormant decay comparison note",
        "access-decay",
        "medium",
    );

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

    let conn = Connection::open(&db_path).unwrap();
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
