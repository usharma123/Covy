use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

pub(crate) fn table_count(conn: &Connection, table: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    conn.query_row(&sql, [], |row| row.get(0))
        .map_err(Into::into)
}

pub(crate) fn expanded_filter_limit(limit: usize, has_filters: bool) -> usize {
    if has_filters {
        limit.max(1).saturating_mul(20).min(10_000)
    } else {
        limit.max(1)
    }
}

pub(crate) fn open_memory_db() -> Result<Connection> {
    let path = packet28_db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create '{}'", parent.display()))?;
    }
    let conn =
        Connection::open(&path).with_context(|| format!("failed to open '{}'", path.display()))?;
    initialize_schema(&conn)?;
    Ok(conn)
}

fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL,
            payload_json TEXT NOT NULL DEFAULT '{}',
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS commands (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            command TEXT NOT NULL,
            cwd TEXT,
            exit_code INTEGER,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS reductions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            command_id INTEGER,
            family TEXT,
            raw_est_tokens INTEGER NOT NULL DEFAULT 0,
            reduced_est_tokens INTEGER NOT NULL DEFAULT 0,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS memories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content TEXT NOT NULL,
            tags TEXT,
            topic TEXT NOT NULL DEFAULT 'general',
            importance TEXT NOT NULL DEFAULT 'medium',
            keywords TEXT,
            project TEXT,
            source TEXT,
            raw_excerpt TEXT,
            weight REAL NOT NULL DEFAULT 1.0,
            access_count INTEGER NOT NULL DEFAULT 0,
            last_accessed_unix_ms INTEGER NOT NULL DEFAULT 0,
            created_at_unix_ms INTEGER NOT NULL,
            updated_at_unix_ms INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS memory_chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL,
            chunk_index INTEGER NOT NULL,
            content TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS memory_embeddings (
            memory_id INTEGER NOT NULL,
            model TEXT NOT NULL,
            dimensions INTEGER NOT NULL,
            embedding_json TEXT NOT NULL,
            created_at_unix_ms INTEGER NOT NULL,
            PRIMARY KEY (memory_id, model)
        );
        CREATE TABLE IF NOT EXISTS concepts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            description TEXT,
            memoir_name TEXT NOT NULL DEFAULT 'default',
            labels TEXT NOT NULL DEFAULT '[]',
            confidence REAL NOT NULL DEFAULT 0.5,
            revision INTEGER NOT NULL DEFAULT 1,
            source_ids TEXT NOT NULL DEFAULT '[]',
            updated_at_unix_ms INTEGER NOT NULL DEFAULT 0,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS memoirs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            description TEXT,
            created_at_unix_ms INTEGER NOT NULL,
            updated_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS relations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_concept_id INTEGER NOT NULL,
            target_concept_id INTEGER NOT NULL,
            relation TEXT NOT NULL,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS feedback (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            subject TEXT NOT NULL,
            correction TEXT NOT NULL,
            topic TEXT NOT NULL DEFAULT 'general',
            context TEXT,
            predicted TEXT,
            reason TEXT,
            source TEXT,
            project TEXT,
            applied_count INTEGER NOT NULL DEFAULT 0,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS agent_sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent TEXT,
            started_at_unix_ms INTEGER NOT NULL,
            ended_at_unix_ms INTEGER
        );
        CREATE TABLE IF NOT EXISTS transcript_sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_key TEXT NOT NULL UNIQUE,
            agent TEXT,
            started_at_unix_ms INTEGER NOT NULL,
            updated_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS transcript_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id INTEGER NOT NULL,
            role TEXT NOT NULL DEFAULT 'assistant',
            content TEXT NOT NULL,
            source TEXT,
            project TEXT,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS mcp_calls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tool_name TEXT NOT NULL,
            arguments_json TEXT NOT NULL DEFAULT '{}',
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS hook_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            runtime TEXT NOT NULL,
            event_kind TEXT NOT NULL,
            session_id TEXT,
            task_id TEXT,
            matcher TEXT,
            payload_json TEXT NOT NULL DEFAULT '{}',
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS pending_extractions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project TEXT NOT NULL DEFAULT 'project',
            tool_name TEXT NOT NULL DEFAULT 'unknown',
            raw_output TEXT NOT NULL,
            captured_at_unix_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_pending_extractions_captured
            ON pending_extractions(captured_at_unix_ms);

        CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
            content,
            tags,
            content='memories',
            content_rowid='id'
        );
        CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid, content, tags)
            VALUES (new.id, new.content, new.tags);
        END;
        CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, content, tags)
            VALUES ('delete', old.id, old.content, old.tags);
        END;
        CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE OF content, tags ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, content, tags)
            VALUES ('delete', old.id, old.content, old.tags);
            INSERT INTO memories_fts(rowid, content, tags)
            VALUES (new.id, new.content, new.tags);
        END;

        CREATE VIRTUAL TABLE IF NOT EXISTS concepts_fts USING fts5(
            name,
            description,
            content='concepts',
            content_rowid='id'
        );
        CREATE TRIGGER IF NOT EXISTS concepts_ai AFTER INSERT ON concepts BEGIN
            INSERT INTO concepts_fts(rowid, name, description)
            VALUES (new.id, new.name, new.description);
        END;
        CREATE TRIGGER IF NOT EXISTS concepts_ad AFTER DELETE ON concepts BEGIN
            INSERT INTO concepts_fts(concepts_fts, rowid, name, description)
            VALUES ('delete', old.id, old.name, old.description);
        END;
        CREATE TRIGGER IF NOT EXISTS concepts_au AFTER UPDATE OF name, description ON concepts BEGIN
            INSERT INTO concepts_fts(concepts_fts, rowid, name, description)
            VALUES ('delete', old.id, old.name, old.description);
            INSERT INTO concepts_fts(rowid, name, description)
            VALUES (new.id, new.name, new.description);
        END;

        CREATE VIRTUAL TABLE IF NOT EXISTS feedback_fts USING fts5(
            subject,
            correction,
            content='feedback',
            content_rowid='id'
        );
        CREATE TRIGGER IF NOT EXISTS feedback_ai AFTER INSERT ON feedback BEGIN
            INSERT INTO feedback_fts(rowid, subject, correction)
            VALUES (new.id, new.subject, new.correction);
        END;
        CREATE TRIGGER IF NOT EXISTS feedback_ad AFTER DELETE ON feedback BEGIN
            INSERT INTO feedback_fts(feedback_fts, rowid, subject, correction)
            VALUES ('delete', old.id, old.subject, old.correction);
        END;
        CREATE TRIGGER IF NOT EXISTS feedback_au AFTER UPDATE OF subject, correction ON feedback BEGIN
            INSERT INTO feedback_fts(feedback_fts, rowid, subject, correction)
            VALUES ('delete', old.id, old.subject, old.correction);
            INSERT INTO feedback_fts(rowid, subject, correction)
            VALUES (new.id, new.subject, new.correction);
        END;

        CREATE VIRTUAL TABLE IF NOT EXISTS transcript_messages_fts USING fts5(
            role,
            content,
            source,
            content='transcript_messages',
            content_rowid='id'
        );
        CREATE TRIGGER IF NOT EXISTS transcript_messages_ai AFTER INSERT ON transcript_messages BEGIN
            INSERT INTO transcript_messages_fts(rowid, role, content, source)
            VALUES (new.id, new.role, new.content, new.source);
        END;
        CREATE TRIGGER IF NOT EXISTS transcript_messages_ad AFTER DELETE ON transcript_messages BEGIN
            INSERT INTO transcript_messages_fts(transcript_messages_fts, rowid, role, content, source)
            VALUES ('delete', old.id, old.role, old.content, old.source);
        END;
        CREATE TRIGGER IF NOT EXISTS transcript_messages_au AFTER UPDATE OF role, content, source ON transcript_messages BEGIN
            INSERT INTO transcript_messages_fts(transcript_messages_fts, rowid, role, content, source)
            VALUES ('delete', old.id, old.role, old.content, old.source);
            INSERT INTO transcript_messages_fts(rowid, role, content, source)
            VALUES (new.id, new.role, new.content, new.source);
        END;
        ",
    )?;
    add_column_if_missing(conn, "memories", "topic", "TEXT NOT NULL DEFAULT 'general'")?;
    add_column_if_missing(
        conn,
        "memories",
        "importance",
        "TEXT NOT NULL DEFAULT 'medium'",
    )?;
    add_column_if_missing(conn, "memories", "keywords", "TEXT")?;
    add_column_if_missing(conn, "memories", "project", "TEXT")?;
    add_column_if_missing(conn, "memories", "source", "TEXT")?;
    add_column_if_missing(conn, "memories", "raw_excerpt", "TEXT")?;
    add_column_if_missing(conn, "memories", "weight", "REAL NOT NULL DEFAULT 1.0")?;
    add_column_if_missing(
        conn,
        "memories",
        "access_count",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        conn,
        "memories",
        "last_accessed_unix_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        conn,
        "memories",
        "updated_at_unix_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    conn.execute(
        "UPDATE memories SET updated_at_unix_ms = created_at_unix_ms WHERE updated_at_unix_ms = 0",
        [],
    )?;
    conn.execute(
        "UPDATE memories SET last_accessed_unix_ms = updated_at_unix_ms WHERE last_accessed_unix_ms = 0",
        [],
    )?;
    add_column_if_missing(
        conn,
        "concepts",
        "memoir_name",
        "TEXT NOT NULL DEFAULT 'default'",
    )?;
    add_column_if_missing(conn, "concepts", "labels", "TEXT NOT NULL DEFAULT '[]'")?;
    add_column_if_missing(conn, "concepts", "confidence", "REAL NOT NULL DEFAULT 0.5")?;
    add_column_if_missing(conn, "concepts", "revision", "INTEGER NOT NULL DEFAULT 1")?;
    add_column_if_missing(conn, "concepts", "source_ids", "TEXT NOT NULL DEFAULT '[]'")?;
    add_column_if_missing(
        conn,
        "concepts",
        "updated_at_unix_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    conn.execute(
        "UPDATE concepts SET updated_at_unix_ms = created_at_unix_ms WHERE updated_at_unix_ms = 0",
        [],
    )?;
    conn.execute(
        "INSERT INTO memoirs (name, description, created_at_unix_ms, updated_at_unix_ms)
         SELECT 'default', 'Default Packet28 graph memoir', ?1, ?1
         WHERE NOT EXISTS (SELECT 1 FROM memoirs WHERE name = 'default')",
        params![timestamp_unix_ms()],
    )?;
    add_column_if_missing(conn, "feedback", "topic", "TEXT NOT NULL DEFAULT 'general'")?;
    add_column_if_missing(conn, "feedback", "context", "TEXT")?;
    add_column_if_missing(conn, "feedback", "predicted", "TEXT")?;
    add_column_if_missing(conn, "feedback", "reason", "TEXT")?;
    add_column_if_missing(conn, "feedback", "source", "TEXT")?;
    add_column_if_missing(conn, "feedback", "project", "TEXT")?;
    add_column_if_missing(
        conn,
        "feedback",
        "applied_count",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS feedback_fts_all USING fts5(
            subject,
            correction,
            topic,
            context,
            predicted,
            reason,
            source,
            content='feedback',
            content_rowid='id'
        );
        CREATE TRIGGER IF NOT EXISTS feedback_all_ai AFTER INSERT ON feedback BEGIN
            INSERT INTO feedback_fts_all(rowid, subject, correction, topic, context, predicted, reason, source)
            VALUES (new.id, new.subject, new.correction, new.topic, new.context, new.predicted, new.reason, new.source);
        END;
        CREATE TRIGGER IF NOT EXISTS feedback_all_ad AFTER DELETE ON feedback BEGIN
            INSERT INTO feedback_fts_all(feedback_fts_all, rowid, subject, correction, topic, context, predicted, reason, source)
            VALUES ('delete', old.id, old.subject, old.correction, old.topic, old.context, old.predicted, old.reason, old.source);
        END;
        CREATE TRIGGER IF NOT EXISTS feedback_all_au AFTER UPDATE OF subject, correction, topic, context, predicted, reason, source ON feedback BEGIN
            INSERT INTO feedback_fts_all(feedback_fts_all, rowid, subject, correction, topic, context, predicted, reason, source)
            VALUES ('delete', old.id, old.subject, old.correction, old.topic, old.context, old.predicted, old.reason, old.source);
            INSERT INTO feedback_fts_all(rowid, subject, correction, topic, context, predicted, reason, source)
            VALUES (new.id, new.subject, new.correction, new.topic, new.context, new.predicted, new.reason, new.source);
        END;
        ",
    )?;
    add_column_if_missing(conn, "transcript_messages", "project", "TEXT")?;
    rebuild_fts_table(conn, "memories_fts")?;
    rebuild_fts_table(conn, "feedback_fts")?;
    rebuild_fts_table(conn, "feedback_fts_all")?;
    rebuild_fts_table(conn, "concepts_fts")?;
    rebuild_fts_table(conn, "transcript_messages_fts")?;
    Ok(())
}

pub(crate) fn fts_match_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .filter_map(|term| {
            let term = term.trim_matches('-').trim();
            (!term.is_empty()).then(|| format!("\"{}\"", term.replace('"', "\"\"")))
        })
        .take(8)
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(());
        }
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}

fn rebuild_fts_table(conn: &Connection, table: &str) -> Result<()> {
    conn.execute(
        &format!("INSERT INTO {table}({table}) VALUES('rebuild')"),
        [],
    )?;
    Ok(())
}

pub(crate) fn normalize_non_empty(value: Option<&str>, default: &str) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn packet28_db_path() -> PathBuf {
    dirs_home().join(".packet28").join("packet28.db")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

pub(crate) fn timestamp_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
