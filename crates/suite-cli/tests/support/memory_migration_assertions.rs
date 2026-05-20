use rusqlite::Connection;

pub fn assert_migrated_schema(conn: &Connection) {
    assert_table_columns(
        conn,
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
        conn,
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
    assert_table_columns(conn, "transcript_messages", &["source", "project"]);
    assert_table_columns(
        conn,
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
}

pub fn assert_migrated_fts(conn: &Connection) {
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

    assert_eq!(fts_row_count(conn, "memories_fts"), 1);
    assert_eq!(fts_row_count(conn, "feedback_fts"), 1);
    assert_eq!(fts_row_count(conn, "feedback_fts_all"), 1);
    assert_eq!(fts_row_count(conn, "concepts_fts"), 1);
    assert_eq!(fts_row_count(conn, "transcript_messages_fts"), 1);
}

pub fn assert_migrated_memory_defaults(conn: &Connection) {
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
