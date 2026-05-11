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
    pub(crate) topic: String,
    pub(crate) importance: String,
    pub(crate) keywords: Option<String>,
    pub(crate) raw_excerpt: Option<String>,
    pub(crate) created_at_unix_ms: i64,
    pub(crate) updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FeedbackRecord {
    pub(crate) id: i64,
    pub(crate) subject: String,
    pub(crate) correction: String,
    pub(crate) created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphConcept {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphRelation {
    pub(crate) id: i64,
    pub(crate) source: String,
    pub(crate) target: String,
    pub(crate) relation: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphInspect {
    pub(crate) concepts: Vec<GraphConcept>,
    pub(crate) relations: Vec<GraphRelation>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LocalStoreStats {
    pub(crate) memory_count: i64,
    pub(crate) feedback_count: i64,
    pub(crate) concept_count: i64,
    pub(crate) relation_count: i64,
    pub(crate) mcp_call_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryTopicStats {
    pub(crate) topic: String,
    pub(crate) memory_count: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryStoreInput<'a> {
    pub(crate) content: &'a str,
    pub(crate) tags: Option<&'a str>,
    pub(crate) topic: Option<&'a str>,
    pub(crate) importance: Option<&'a str>,
    pub(crate) keywords: Option<&'a str>,
    pub(crate) raw_excerpt: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryUpdateInput<'a> {
    pub(crate) id: i64,
    pub(crate) content: Option<&'a str>,
    pub(crate) tags: Option<&'a str>,
    pub(crate) topic: Option<&'a str>,
    pub(crate) importance: Option<&'a str>,
    pub(crate) keywords: Option<&'a str>,
    pub(crate) raw_excerpt: Option<&'a str>,
}

pub(crate) fn store_memory_with_metadata(input: MemoryStoreInput<'_>) -> Result<MemoryRecord> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    let topic = normalize_non_empty(input.topic, "general");
    let importance = normalize_non_empty(input.importance, "medium");
    conn.execute(
        "INSERT INTO memories
         (content, tags, topic, importance, keywords, raw_excerpt, created_at_unix_ms, updated_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![
            input.content,
            input.tags,
            topic,
            importance,
            input.keywords,
            input.raw_excerpt,
            now
        ],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO memory_chunks (memory_id, chunk_index, content) VALUES (?1, 0, ?2)",
        params![id, input.content],
    )?;
    get_memory(&conn, id)
}

pub(crate) fn recall_memories(query: &str, limit: usize) -> Result<Vec<MemoryRecord>> {
    let conn = open_memory_db()?;
    if let Some(match_query) = fts_match_query(query) {
        let mut stmt = conn.prepare(
            "SELECT
                m.id, m.content, m.tags, m.topic, m.importance, m.keywords, m.raw_excerpt,
                m.created_at_unix_ms, m.updated_at_unix_ms
             FROM memories_fts f
             JOIN memories m ON m.rowid = f.rowid
             WHERE memories_fts MATCH ?1
             ORDER BY bm25(memories_fts), m.created_at_unix_ms DESC
             LIMIT ?2",
        )?;
        let records = read_memory_rows(&mut stmt, params![match_query, limit.max(1) as i64])?;
        if !records.is_empty() {
            return Ok(records);
        }
    }
    recall_memories_like(&conn, query, limit)
}

fn recall_memories_like(conn: &Connection, query: &str, limit: usize) -> Result<Vec<MemoryRecord>> {
    let pattern = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT
            id, content, tags, topic, importance, keywords, raw_excerpt,
            created_at_unix_ms, updated_at_unix_ms
         FROM memories
         WHERE content LIKE ?1
            OR IFNULL(tags, '') LIKE ?1
            OR IFNULL(topic, '') LIKE ?1
            OR IFNULL(keywords, '') LIKE ?1
            OR IFNULL(raw_excerpt, '') LIKE ?1
         ORDER BY created_at_unix_ms DESC
         LIMIT ?2",
    )?;
    read_memory_rows(&mut stmt, params![pattern, limit.max(1) as i64])
}

pub(crate) fn list_memories(limit: usize) -> Result<Vec<MemoryRecord>> {
    let conn = open_memory_db()?;
    let mut stmt = conn.prepare(
        "SELECT
            id, content, tags, topic, importance, keywords, raw_excerpt,
            created_at_unix_ms, updated_at_unix_ms
         FROM memories
         ORDER BY created_at_unix_ms DESC
         LIMIT ?1",
    )?;
    read_memory_rows(&mut stmt, params![limit.max(1) as i64])
}

pub(crate) fn update_memory(input: MemoryUpdateInput<'_>) -> Result<MemoryRecord> {
    let conn = open_memory_db()?;
    let current = get_memory(&conn, input.id)?;
    let now = timestamp_unix_ms();
    let content = input.content.unwrap_or(&current.content);
    let tags = input.tags.or(current.tags.as_deref());
    let topic = input.topic.unwrap_or(&current.topic);
    let importance = input.importance.unwrap_or(&current.importance);
    let keywords = input.keywords.or(current.keywords.as_deref());
    let raw_excerpt = input.raw_excerpt.or(current.raw_excerpt.as_deref());
    conn.execute(
        "UPDATE memories
         SET content = ?1,
             tags = ?2,
             topic = ?3,
             importance = ?4,
             keywords = ?5,
             raw_excerpt = ?6,
             updated_at_unix_ms = ?7
         WHERE id = ?8",
        params![
            content,
            tags,
            normalize_non_empty(Some(topic), "general"),
            normalize_non_empty(Some(importance), "medium"),
            keywords,
            raw_excerpt,
            now,
            input.id
        ],
    )?;
    conn.execute(
        "UPDATE memory_chunks SET content = ?1 WHERE memory_id = ?2 AND chunk_index = 0",
        params![content, input.id],
    )?;
    get_memory(&conn, input.id)
}

pub(crate) fn forget_memory(id: i64) -> Result<usize> {
    let conn = open_memory_db()?;
    conn.execute(
        "DELETE FROM memory_chunks WHERE memory_id = ?1",
        params![id],
    )?;
    conn.execute("DELETE FROM memories WHERE id = ?1", params![id])
        .map_err(Into::into)
}

pub(crate) fn forget_memories_by_topic(topic: &str) -> Result<usize> {
    let conn = open_memory_db()?;
    let topic = normalize_non_empty(Some(topic), "general");
    conn.execute(
        "DELETE FROM memory_chunks WHERE memory_id IN (SELECT id FROM memories WHERE topic = ?1)",
        params![topic],
    )?;
    conn.execute("DELETE FROM memories WHERE topic = ?1", params![topic])
        .map_err(Into::into)
}

pub(crate) fn memory_topics() -> Result<Vec<MemoryTopicStats>> {
    let conn = open_memory_db()?;
    let mut stmt = conn.prepare(
        "SELECT topic, COUNT(*)
         FROM memories
         GROUP BY topic
         ORDER BY COUNT(*) DESC, topic ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(MemoryTopicStats {
            topic: row.get(0)?,
            memory_count: row.get(1)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub(crate) fn record_feedback(subject: &str, correction: &str) -> Result<FeedbackRecord> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    conn.execute(
        "INSERT INTO feedback (subject, correction, created_at_unix_ms) VALUES (?1, ?2, ?3)",
        params![subject, correction, now],
    )?;
    Ok(FeedbackRecord {
        id: conn.last_insert_rowid(),
        subject: subject.to_string(),
        correction: correction.to_string(),
        created_at_unix_ms: now,
    })
}

pub(crate) fn search_feedback(query: &str, limit: usize) -> Result<Vec<FeedbackRecord>> {
    let conn = open_memory_db()?;
    if let Some(match_query) = fts_match_query(query) {
        let mut stmt = conn.prepare(
            "SELECT fb.id, fb.subject, fb.correction, fb.created_at_unix_ms
             FROM feedback_fts f
             JOIN feedback fb ON fb.rowid = f.rowid
             WHERE feedback_fts MATCH ?1
             ORDER BY bm25(feedback_fts), fb.created_at_unix_ms DESC
             LIMIT ?2",
        )?;
        let records = read_feedback_rows(&mut stmt, params![match_query, limit.max(1) as i64])?;
        if !records.is_empty() {
            return Ok(records);
        }
    }
    search_feedback_like(&conn, query, limit)
}

fn search_feedback_like(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<FeedbackRecord>> {
    let pattern = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT id, subject, correction, created_at_unix_ms
         FROM feedback
         WHERE subject LIKE ?1 OR correction LIKE ?1
         ORDER BY created_at_unix_ms DESC
         LIMIT ?2",
    )?;
    read_feedback_rows(&mut stmt, params![pattern, limit.max(1) as i64])
}

fn read_feedback_rows<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    params: P,
) -> Result<Vec<FeedbackRecord>> {
    let rows = stmt.query_map(params, |row| {
        Ok(FeedbackRecord {
            id: row.get(0)?,
            subject: row.get(1)?,
            correction: row.get(2)?,
            created_at_unix_ms: row.get(3)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub(crate) fn add_concept(name: &str, description: Option<&str>) -> Result<GraphConcept> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    conn.execute(
        "INSERT INTO concepts (name, description, created_at_unix_ms)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET description=COALESCE(excluded.description, concepts.description)",
        params![name, description, now],
    )?;
    let mut stmt = conn.prepare("SELECT id, name, description FROM concepts WHERE name = ?1")?;
    let concept = stmt.query_row(params![name], |row| {
        Ok(GraphConcept {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
        })
    })?;
    Ok(concept)
}

pub(crate) fn link_concepts(source: &str, target: &str, relation: &str) -> Result<GraphRelation> {
    let source = add_concept(source, None)?;
    let target = add_concept(target, None)?;
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    conn.execute(
        "INSERT INTO relations (source_concept_id, target_concept_id, relation, created_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![source.id, target.id, relation, now],
    )?;
    Ok(GraphRelation {
        id: conn.last_insert_rowid(),
        source: source.name,
        target: target.name,
        relation: relation.to_string(),
    })
}

pub(crate) fn inspect_graph(limit: usize) -> Result<GraphInspect> {
    let conn = open_memory_db()?;
    let mut concepts_stmt =
        conn.prepare("SELECT id, name, description FROM concepts ORDER BY name ASC LIMIT ?1")?;
    let concepts = concepts_stmt
        .query_map(params![limit.max(1) as i64], |row| {
            Ok(GraphConcept {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut relations_stmt = conn.prepare(
        "SELECT r.id, s.name, t.name, r.relation
         FROM relations r
         JOIN concepts s ON s.id = r.source_concept_id
         JOIN concepts t ON t.id = r.target_concept_id
         ORDER BY r.id DESC
         LIMIT ?1",
    )?;
    let relations = relations_stmt
        .query_map(params![limit.max(1) as i64], |row| {
            Ok(GraphRelation {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                relation: row.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(GraphInspect {
        concepts,
        relations,
    })
}

pub(crate) fn local_store_stats() -> Result<LocalStoreStats> {
    let conn = open_memory_db()?;
    Ok(LocalStoreStats {
        memory_count: table_count(&conn, "memories")?,
        feedback_count: table_count(&conn, "feedback")?,
        concept_count: table_count(&conn, "concepts")?,
        relation_count: table_count(&conn, "relations")?,
        mcp_call_count: table_count(&conn, "mcp_calls")?,
    })
}

fn table_count(conn: &Connection, table: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    conn.query_row(&sql, [], |row| row.get(0))
        .map_err(Into::into)
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
            topic: row.get(3)?,
            importance: row.get(4)?,
            keywords: row.get(5)?,
            raw_excerpt: row.get(6)?,
            created_at_unix_ms: row.get(7)?,
            updated_at_unix_ms: row.get(8)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn get_memory(conn: &Connection, id: i64) -> Result<MemoryRecord> {
    conn.query_row(
        "SELECT
            id, content, tags, topic, importance, keywords, raw_excerpt,
            created_at_unix_ms, updated_at_unix_ms
         FROM memories
         WHERE id = ?1",
        params![id],
        |row| {
            Ok(MemoryRecord {
                id: row.get(0)?,
                content: row.get(1)?,
                tags: row.get(2)?,
                topic: row.get(3)?,
                importance: row.get(4)?,
                keywords: row.get(5)?,
                raw_excerpt: row.get(6)?,
                created_at_unix_ms: row.get(7)?,
                updated_at_unix_ms: row.get(8)?,
            })
        },
    )
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
            topic TEXT NOT NULL DEFAULT 'general',
            importance TEXT NOT NULL DEFAULT 'medium',
            keywords TEXT,
            raw_excerpt TEXT,
            created_at_unix_ms INTEGER NOT NULL,
            updated_at_unix_ms INTEGER NOT NULL DEFAULT 0
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
    add_column_if_missing(conn, "memories", "raw_excerpt", "TEXT")?;
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
        "INSERT INTO memories_fts(rowid, content, tags)
         SELECT id, content, tags FROM memories
         WHERE id NOT IN (SELECT rowid FROM memories_fts)",
        [],
    )?;
    conn.execute(
        "INSERT INTO feedback_fts(rowid, subject, correction)
         SELECT id, subject, correction FROM feedback
         WHERE id NOT IN (SELECT rowid FROM feedback_fts)",
        [],
    )?;
    Ok(())
}

fn fts_match_query(query: &str) -> Option<String> {
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

fn normalize_non_empty(value: Option<&str>, default: &str) -> String {
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

fn timestamp_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
