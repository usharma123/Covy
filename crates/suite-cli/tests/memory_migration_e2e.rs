use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_memory_migration_store_migrates_legacy_sqlite_schema() {
    let home = TempDir::new().unwrap();
    let packet28_dir = home.path().join(".packet28");
    fs::create_dir_all(&packet28_dir).unwrap();
    let db_path = packet28_dir.join("packet28.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE memories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content TEXT NOT NULL,
            tags TEXT,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE feedback (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            subject TEXT NOT NULL,
            correction TEXT NOT NULL,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE concepts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            description TEXT,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE transcript_sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_key TEXT NOT NULL UNIQUE,
            agent TEXT,
            started_at_unix_ms INTEGER NOT NULL,
            updated_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE transcript_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id INTEGER NOT NULL,
            role TEXT NOT NULL DEFAULT 'assistant',
            content TEXT NOT NULL,
            source TEXT,
            created_at_unix_ms INTEGER NOT NULL
        );
        INSERT INTO memories (content, tags, created_at_unix_ms)
            VALUES ('legacy Packet28 durable context', 'legacy', 1700000000000);
        INSERT INTO feedback (subject, correction, created_at_unix_ms)
            VALUES ('legacy feedback subject', 'legacy correction body', 1700000000001);
        INSERT INTO concepts (name, description, created_at_unix_ms)
            VALUES ('LegacyConcept', 'legacy graph description', 1700000000002);
        INSERT INTO transcript_sessions (session_key, agent, started_at_unix_ms, updated_at_unix_ms)
            VALUES ('legacy-session', 'codex', 1700000000003, 1700000000003);
        INSERT INTO transcript_messages (session_id, role, content, source, created_at_unix_ms)
            VALUES (1, 'user', 'legacy transcript context', 'legacy-test', 1700000000004);
        ",
    )
    .unwrap();
    drop(conn);

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"memory_count\":1"));

    let conn = Connection::open(&db_path).unwrap();
    assert_table_columns(
        &conn,
        "memories",
        &[
            "topic",
            "importance",
            "keywords",
            "project",
            "source",
            "raw_excerpt",
            "weight",
            "access_count",
            "last_accessed_unix_ms",
            "updated_at_unix_ms",
        ],
    );
    assert_table_columns(
        &conn,
        "feedback",
        &[
            "topic",
            "context",
            "predicted",
            "reason",
            "source",
            "project",
            "applied_count",
        ],
    );
    assert_table_columns(&conn, "transcript_messages", &["source", "project"]);
    assert_table_columns(
        &conn,
        "concepts",
        &[
            "memoir_name",
            "labels",
            "confidence",
            "revision",
            "source_ids",
            "updated_at_unix_ms",
        ],
    );

    let fts_tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='table'
             AND name IN (
                'memories_fts',
                'feedback_fts',
                'feedback_fts_all',
                'concepts_fts',
                'transcript_messages_fts'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fts_tables, 5);
    let trigger_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='trigger'
             AND name IN (
                'memories_ai',
                'memories_ad',
                'memories_au',
                'feedback_all_ai',
                'feedback_all_ad',
                'feedback_all_au',
                'concepts_ai',
                'concepts_ad',
                'concepts_au',
                'transcript_messages_ai',
                'transcript_messages_ad',
                'transcript_messages_au'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trigger_count, 12);

    let migrated_memory: (String, String, f64, i64, i64) = conn
        .query_row(
            "SELECT topic, importance, weight, access_count, last_accessed_unix_ms
             FROM memories WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(migrated_memory.0, "general");
    assert_eq!(migrated_memory.1, "medium");
    assert_eq!(migrated_memory.2, 1.0);
    assert_eq!(migrated_memory.3, 0);
    assert_eq!(migrated_memory.4, 1700000000000);

    assert_eq!(fts_row_count(&conn, "memories_fts"), 1);
    assert_eq!(fts_row_count(&conn, "feedback_fts"), 1);
    assert_eq!(fts_row_count(&conn, "feedback_fts_all"), 1);
    assert_eq!(fts_row_count(&conn, "concepts_fts"), 1);
    assert_eq!(fts_row_count(&conn, "transcript_messages_fts"), 1);

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "recall", "legacy durable", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("legacy Packet28 durable context"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "search", "legacy correction", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("legacy correction body"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "search", "LegacyConcept", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("LegacyConcept"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "search", "legacy transcript", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("legacy transcript context"));
}

fn assert_table_columns(conn: &Connection, table: &str, expected: &[&str]) {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(|row| row.unwrap())
        .collect::<Vec<_>>();
    for column in expected {
        assert!(
            columns.iter().any(|existing| existing == column),
            "expected column {table}.{column}; found {columns:?}"
        );
    }
}

fn fts_row_count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}
