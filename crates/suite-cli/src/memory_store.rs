use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryRecord {
    pub(crate) id: i64,
    pub(crate) content: String,
    pub(crate) tags: Option<String>,
    pub(crate) created_at_unix_ms: i64,
}

pub(crate) fn store_memory(content: &str, tags: Option<&str>) -> Result<MemoryRecord> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    conn.execute(
        "INSERT INTO memories (content, tags, created_at_unix_ms) VALUES (?1, ?2, ?3)",
        params![content, tags, now],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO memory_chunks (memory_id, chunk_index, content) VALUES (?1, 0, ?2)",
        params![id, content],
    )?;
    Ok(MemoryRecord {
        id,
        content: content.to_string(),
        tags: tags.map(ToOwned::to_owned),
        created_at_unix_ms: now,
    })
}

pub(crate) fn recall_memories(query: &str, limit: usize) -> Result<Vec<MemoryRecord>> {
    let conn = open_memory_db()?;
    let pattern = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT id, content, tags, created_at_unix_ms
         FROM memories
         WHERE content LIKE ?1 OR IFNULL(tags, '') LIKE ?1
         ORDER BY created_at_unix_ms DESC
         LIMIT ?2",
    )?;
    read_memory_rows(&mut stmt, params![pattern, limit.max(1) as i64])
}

pub(crate) fn list_memories(limit: usize) -> Result<Vec<MemoryRecord>> {
    let conn = open_memory_db()?;
    let mut stmt = conn.prepare(
        "SELECT id, content, tags, created_at_unix_ms
         FROM memories
         ORDER BY created_at_unix_ms DESC
         LIMIT ?1",
    )?;
    read_memory_rows(&mut stmt, params![limit.max(1) as i64])
}

fn read_memory_rows<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    params: P,
) -> Result<Vec<MemoryRecord>> {
    let rows = stmt.query_map(params, |row| {
        Ok(MemoryRecord {
            id: row.get(0)?,
            content: row.get(1)?,
            tags: row.get(2)?,
            created_at_unix_ms: row.get(3)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn open_memory_db() -> Result<Connection> {
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
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS memory_chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL,
            chunk_index INTEGER NOT NULL,
            content TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS concepts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            description TEXT,
            created_at_unix_ms INTEGER NOT NULL
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
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS agent_sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent TEXT,
            started_at_unix_ms INTEGER NOT NULL,
            ended_at_unix_ms INTEGER
        );
        CREATE TABLE IF NOT EXISTS mcp_calls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tool_name TEXT NOT NULL,
            arguments_json TEXT NOT NULL DEFAULT '{}',
            created_at_unix_ms INTEGER NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn packet28_db_path() -> PathBuf {
    dirs_home().join(".packet28").join("packet28.db")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn timestamp_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
