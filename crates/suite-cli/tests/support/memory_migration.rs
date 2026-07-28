use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};

pub fn write_legacy_memory_db(home: &Path) -> PathBuf {
    let packet28_dir = home.join(".packet28");
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
    db_path
}
