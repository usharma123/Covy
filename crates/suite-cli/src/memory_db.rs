use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, Transaction, TransactionBehavior};

pub(crate) const CURRENT_MEMORY_SCHEMA_VERSION: u32 = 3;

static CONNECTIONS_OPENED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LocalMemoryStoreMetrics {
    pub(crate) connections_opened: u64,
    pub(crate) migrations_applied: u32,
    pub(crate) transactions_committed: u64,
    pub(crate) transactions_rolled_back: u64,
}

/// Owns one initialized SQLite connection for a complete local-memory workflow.
pub(crate) struct LocalMemoryStore {
    conn: Connection,
    metrics: LocalMemoryStoreMetrics,
}

impl LocalMemoryStore {
    pub(crate) fn open_default() -> Result<Self> {
        Self::open_path(packet28_db_path())
    }

    pub(crate) fn open_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create '{}'", parent.display()))?;
            }
        }
        let mut conn = Connection::open(path)
            .with_context(|| format!("failed to open '{}'", path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let migrations_applied = migrate_schema(&mut conn)?;
        CONNECTIONS_OPENED.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            conn,
            metrics: LocalMemoryStoreMetrics {
                connections_opened: 1,
                migrations_applied,
                ..LocalMemoryStoreMetrics::default()
            },
        })
    }

    pub(crate) fn metrics(&self) -> LocalMemoryStoreMetrics {
        self.metrics
    }

    pub(crate) fn schema_version(&self) -> Result<u32> {
        schema_version(&self.conn)
    }

    pub(crate) fn transaction<T>(
        &mut self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        match operation(&tx) {
            Ok(value) => match tx.commit() {
                Ok(()) => {
                    self.metrics.transactions_committed =
                        self.metrics.transactions_committed.saturating_add(1);
                    Ok(value)
                }
                Err(error) => {
                    self.metrics.transactions_rolled_back =
                        self.metrics.transactions_rolled_back.saturating_add(1);
                    Err(error.into())
                }
            },
            Err(error) => {
                self.metrics.transactions_rolled_back =
                    self.metrics.transactions_rolled_back.saturating_add(1);
                tx.rollback().with_context(|| {
                    format!("failed to roll back local memory workflow after: {error:#}")
                })?;
                Err(error)
            }
        }
    }
}

impl Deref for LocalMemoryStore {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

pub(crate) fn total_memory_connections_opened() -> u64 {
    CONNECTIONS_OPENED.load(Ordering::Relaxed)
}

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

fn schema_version(conn: &Connection) -> Result<u32> {
    let version = conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
    u32::try_from(version).context("SQLite user_version is outside the supported range")
}

fn migrate_schema(conn: &mut Connection) -> Result<u32> {
    let from_version = schema_version(conn)?;
    if from_version > CURRENT_MEMORY_SCHEMA_VERSION {
        anyhow::bail!(
            "local memory schema version {from_version} is newer than supported version {CURRENT_MEMORY_SCHEMA_VERSION}"
        );
    }
    if from_version == CURRENT_MEMORY_SCHEMA_VERSION {
        return Ok(0);
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for version in (from_version + 1)..=CURRENT_MEMORY_SCHEMA_VERSION {
        apply_migration(&tx, version)?;
        tx.pragma_update(None, "user_version", version)?;
    }
    tx.commit()?;
    Ok(CURRENT_MEMORY_SCHEMA_VERSION - from_version)
}

fn apply_migration(conn: &Connection, version: u32) -> Result<()> {
    match version {
        1 => initialize_legacy_schema(conn),
        2 => add_query_indexes(conn),
        3 => enforce_memory_dependent_lifecycle(conn),
        _ => anyhow::bail!("missing local memory migration for schema version {version}"),
    }
}

fn initialize_legacy_schema(conn: &Connection) -> Result<()> {
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

fn add_query_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_memory_chunks_memory_id
            ON memory_chunks(memory_id, chunk_index);
        CREATE INDEX IF NOT EXISTS idx_memory_embeddings_memory_id
            ON memory_embeddings(memory_id, model);
        CREATE INDEX IF NOT EXISTS idx_memories_topic_project_updated
            ON memories(topic, project, updated_at_unix_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_memories_weight_updated
            ON memories(weight, updated_at_unix_ms);
        CREATE INDEX IF NOT EXISTS idx_concepts_memoir_updated
            ON concepts(memoir_name, updated_at_unix_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_relations_source
            ON relations(source_concept_id);
        CREATE INDEX IF NOT EXISTS idx_relations_target
            ON relations(target_concept_id);
        CREATE INDEX IF NOT EXISTS idx_feedback_topic_project_created
            ON feedback(topic, project, created_at_unix_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_transcript_messages_session_created
            ON transcript_messages(session_id, created_at_unix_ms, id);
        CREATE INDEX IF NOT EXISTS idx_hook_events_runtime_kind_created
            ON hook_events(runtime, event_kind, created_at_unix_ms DESC);
        ",
    )?;
    Ok(())
}

fn enforce_memory_dependent_lifecycle(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        DELETE FROM memory_chunks
        WHERE NOT EXISTS (
            SELECT 1 FROM memories WHERE memories.id = memory_chunks.memory_id
        );
        DELETE FROM memory_embeddings
        WHERE NOT EXISTS (
            SELECT 1 FROM memories WHERE memories.id = memory_embeddings.memory_id
        );

        CREATE TRIGGER IF NOT EXISTS memories_dependents_ad
        AFTER DELETE ON memories
        BEGIN
            DELETE FROM memory_chunks WHERE memory_id = old.id;
            DELETE FROM memory_embeddings WHERE memory_id = old.id;
        END;

        CREATE TRIGGER IF NOT EXISTS memories_embedding_inputs_au
        AFTER UPDATE OF content, tags, topic, keywords, project, source, raw_excerpt ON memories
        BEGIN
            DELETE FROM memory_embeddings WHERE memory_id = old.id;
        END;
        ",
    )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn create_database_at_version(path: &Path, version: u32) {
        let conn = Connection::open(path).unwrap();
        for next in 1..=version {
            apply_migration(&conn, next).unwrap();
            conn.pragma_update(None, "user_version", next).unwrap();
        }
    }

    fn assert_current_schema(store: &LocalMemoryStore) {
        assert_eq!(
            store.schema_version().unwrap(),
            CURRENT_MEMORY_SCHEMA_VERSION
        );
        for table in [
            "memories",
            "memory_embeddings",
            "concepts",
            "relations",
            "feedback",
            "transcript_sessions",
            "transcript_messages",
            "hook_events",
            "pending_extractions",
        ] {
            let exists = store
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM sqlite_master
                        WHERE type IN ('table', 'view') AND name = ?1
                     )",
                    params![table],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap();
            assert!(exists, "missing migrated table {table}");
        }
    }

    #[test]
    fn migrates_from_every_supported_schema_version() {
        for from_version in 0..=CURRENT_MEMORY_SCHEMA_VERSION {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join(format!("memory-v{from_version}.db"));
            create_database_at_version(&path, from_version);

            let store = LocalMemoryStore::open_path(&path).unwrap();

            assert_current_schema(&store);
            assert_eq!(
                store.metrics().migrations_applied,
                CURRENT_MEMORY_SCHEMA_VERSION - from_version
            );
        }
    }

    #[test]
    fn reopening_current_schema_is_idempotent_and_preserves_fts_rows() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("memory.db");
        let first = LocalMemoryStore::open_path(&path).unwrap();
        assert_eq!(
            first.metrics().migrations_applied,
            CURRENT_MEMORY_SCHEMA_VERSION
        );
        first
            .execute(
                "INSERT INTO memories
                 (content, topic, importance, weight, created_at_unix_ms, updated_at_unix_ms)
                 VALUES ('durable fact', 'general', 'medium', 1.0, 1, 1)",
                [],
            )
            .unwrap();
        drop(first);

        let second = LocalMemoryStore::open_path(&path).unwrap();

        assert_eq!(second.metrics().migrations_applied, 0);
        assert_eq!(
            second
                .query_row(
                    "SELECT COUNT(*) FROM memories_fts WHERE memories_fts MATCH 'durable'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            second
                .query_row(
                    "SELECT COUNT(*) FROM memoirs WHERE name = 'default'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn memory_dependents_are_schema_owned_and_updates_invalidate_embeddings() {
        let temp = tempfile::tempdir().unwrap();
        let store = LocalMemoryStore::open_path(temp.path().join("memory.db")).unwrap();
        store
            .execute(
                "INSERT INTO memories
                 (id, content, topic, importance, weight, created_at_unix_ms, updated_at_unix_ms)
                 VALUES (7, 'before', 'general', 'medium', 1.0, 1, 1)",
                [],
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO memory_chunks (memory_id, chunk_index, content)
                 VALUES (7, 0, 'before')",
                [],
            )
            .unwrap();
        let insert_embedding = || {
            store
                .execute(
                    "INSERT INTO memory_embeddings
                     (memory_id, model, dimensions, embedding_json, created_at_unix_ms)
                     VALUES (7, 'test', 8, '[]', 1)",
                    [],
                )
                .unwrap();
        };
        insert_embedding();

        store
            .execute(
                "UPDATE memories SET content = 'after', updated_at_unix_ms = 2 WHERE id = 7",
                [],
            )
            .unwrap();
        assert_eq!(table_count(&store, "memory_embeddings").unwrap(), 0);
        assert_eq!(table_count(&store, "memory_chunks").unwrap(), 1);

        insert_embedding();
        store
            .execute("DELETE FROM memories WHERE id = 7", [])
            .unwrap();
        assert_eq!(table_count(&store, "memory_chunks").unwrap(), 0);
        assert_eq!(table_count(&store, "memory_embeddings").unwrap(), 0);
    }

    #[test]
    fn dependent_lifecycle_migration_removes_existing_orphans() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("memory-v2.db");
        create_database_at_version(&path, 2);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO memory_chunks (memory_id, chunk_index, content)
             VALUES (99, 0, 'orphan')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memory_embeddings
             (memory_id, model, dimensions, embedding_json, created_at_unix_ms)
             VALUES (99, 'test', 8, '[]', 1)",
            [],
        )
        .unwrap();
        drop(conn);

        let store = LocalMemoryStore::open_path(&path).unwrap();

        assert_eq!(store.metrics().migrations_applied, 1);
        assert_eq!(table_count(&store, "memory_chunks").unwrap(), 0);
        assert_eq!(table_count(&store, "memory_embeddings").unwrap(), 0);
    }

    #[test]
    fn failed_migration_rolls_back_schema_and_user_version() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("broken.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE VIEW memories AS SELECT 1 AS id;")
            .unwrap();
        drop(conn);

        let error = LocalMemoryStore::open_path(&path)
            .err()
            .expect("incompatible legacy schema must fail closed");
        assert!(
            error.to_string().contains("memories")
                || error.to_string().contains("column")
                || error.to_string().contains("table")
        );

        let conn = Connection::open(&path).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), 0);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'events'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn workflow_transaction_reports_commit_and_rollback_without_partial_rows() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("memory.db");
        let mut store = LocalMemoryStore::open_path(&path).unwrap();

        let failed: Result<()> = store.transaction(|tx| {
            tx.execute(
                "INSERT INTO events (kind, payload_json, created_at_unix_ms)
                 VALUES ('rollback', '{}', 1)",
                [],
            )?;
            anyhow::bail!("forced rollback")
        });
        assert!(failed.is_err());
        assert_eq!(table_count(&store, "events").unwrap(), 0);
        assert_eq!(store.metrics().transactions_rolled_back, 1);

        store
            .transaction(|tx| {
                tx.execute(
                    "INSERT INTO events (kind, payload_json, created_at_unix_ms)
                     VALUES ('commit', '{}', 2)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(table_count(&store, "events").unwrap(), 1);
        assert_eq!(store.metrics().transactions_committed, 1);
    }

    #[test]
    fn newer_schema_version_fails_without_downgrading() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("future.db");
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(
            None,
            "user_version",
            CURRENT_MEMORY_SCHEMA_VERSION.saturating_add(1),
        )
        .unwrap();
        drop(conn);

        let error = LocalMemoryStore::open_path(&path)
            .err()
            .expect("future schema must fail closed");
        assert!(error.to_string().contains("newer than supported"));
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            schema_version(&conn).unwrap(),
            CURRENT_MEMORY_SCHEMA_VERSION + 1
        );
    }

    #[test]
    #[ignore = "controlled local SQLite ownership benchmark"]
    fn benchmark_owned_connection_against_rebuild_per_operation() {
        const ROWS: usize = 250;
        const ITERATIONS: usize = 12;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("benchmark.db");
        let mut store = LocalMemoryStore::open_path(&path).unwrap();
        store
            .transaction(|tx| {
                let mut insert = tx.prepare_cached(
                    "INSERT INTO memories
                     (content, topic, importance, weight, created_at_unix_ms, updated_at_unix_ms)
                     VALUES (?1, 'benchmark', 'medium', 1.0, ?2, ?2)",
                )?;
                for index in 0..ROWS {
                    insert.execute(params![format!("benchmark memory {index}"), index as i64])?;
                }
                Ok(())
            })
            .unwrap();
        drop(store);

        let rebuild_started = Instant::now();
        for _ in 0..ITERATIONS {
            let conn = Connection::open(&path).unwrap();
            for table in [
                "memories_fts",
                "feedback_fts",
                "feedback_fts_all",
                "concepts_fts",
                "transcript_messages_fts",
            ] {
                rebuild_fts_table(&conn, table).unwrap();
            }
            std::hint::black_box(table_count(&conn, "memories").unwrap());
        }
        let rebuild_elapsed = rebuild_started.elapsed();

        let owned_started = Instant::now();
        let owned = LocalMemoryStore::open_path(&path).unwrap();
        for _ in 0..ITERATIONS {
            std::hint::black_box(table_count(&owned, "memories").unwrap());
        }
        let owned_elapsed = owned_started.elapsed();

        println!(
            "{{\"rows\":{ROWS},\"iterations\":{ITERATIONS},\"rebuild_connections\":{ITERATIONS},\"owned_connections\":{},\"rebuild_micros\":{},\"owned_micros\":{}}}",
            owned.metrics().connections_opened,
            rebuild_elapsed.as_micros(),
            owned_elapsed.as_micros()
        );
        assert_eq!(owned.metrics().connections_opened, 1);
        assert_eq!(owned.metrics().migrations_applied, 0);
    }
}
