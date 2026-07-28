use anyhow::Result;
use rusqlite::params;

use crate::memory_db::{
    normalize_non_empty, table_count, timestamp_unix_ms, total_memory_connections_opened,
    LocalMemoryStore,
};
use crate::memory_store::store_memory_on;
use crate::memory_store_types::*;

pub(crate) fn local_store_stats() -> Result<LocalStoreStats> {
    let conn = LocalMemoryStore::open_default()?;
    let metrics = conn.metrics();
    Ok(LocalStoreStats {
        schema_version: conn.schema_version()?,
        process_connection_open_count: total_memory_connections_opened(),
        connection_open_count: metrics.connections_opened,
        migrations_applied_on_open: metrics.migrations_applied,
        memory_count: table_count(&conn, "memories")?,
        memory_embedding_count: table_count(&conn, "memory_embeddings")?,
        feedback_count: table_count(&conn, "feedback")?,
        concept_count: table_count(&conn, "concepts")?,
        relation_count: table_count(&conn, "relations")?,
        transcript_session_count: table_count(&conn, "transcript_sessions")?,
        transcript_message_count: table_count(&conn, "transcript_messages")?,
        mcp_call_count: table_count(&conn, "mcp_calls")?,
        hook_event_count: table_count(&conn, "hook_events")?,
        pending_extraction_count: table_count(&conn, "pending_extractions")?,
    })
}

pub(crate) fn enqueue_pending_extraction(
    input: PendingExtractionInput<'_>,
) -> Result<PendingExtractionRecord> {
    let raw_output = input.raw_output.trim();
    if raw_output.is_empty() {
        anyhow::bail!("pending extraction raw output cannot be empty");
    }
    let conn = LocalMemoryStore::open_default()?;
    let now = timestamp_unix_ms();
    let project = normalize_non_empty(input.project, "project");
    let tool_name = normalize_non_empty(input.tool_name, "unknown");
    conn.execute(
        "INSERT INTO pending_extractions
         (project, tool_name, raw_output, captured_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![project, tool_name, raw_output, now],
    )?;
    let id = conn.last_insert_rowid();
    Ok(PendingExtractionRecord {
        id,
        project,
        tool_name,
        raw_output: raw_output.to_string(),
        captured_at_unix_ms: now,
    })
}

pub(crate) fn list_pending_extractions(limit: usize) -> Result<Vec<PendingExtractionRecord>> {
    let conn = LocalMemoryStore::open_default()?;
    list_pending_extractions_on(&conn, limit)
}

fn list_pending_extractions_on(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<PendingExtractionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, project, tool_name, raw_output, captured_at_unix_ms
         FROM pending_extractions
         ORDER BY captured_at_unix_ms ASC, id ASC
         LIMIT ?1",
    )?;
    read_pending_extraction_rows(&mut stmt, params![limit.max(1) as i64])
}

pub(crate) fn delete_pending_extractions(ids: &[i64]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let conn = LocalMemoryStore::open_default()?;
    delete_pending_extractions_on(&conn, ids)
}

fn delete_pending_extractions_on(conn: &rusqlite::Connection, ids: &[i64]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("DELETE FROM pending_extractions WHERE id IN ({placeholders})");
    let params: Vec<&dyn rusqlite::ToSql> =
        ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    conn.execute(&sql, params.as_slice()).map_err(Into::into)
}

pub(crate) fn process_pending_extractions(
    limit: usize,
    dry_run: bool,
) -> Result<PendingExtractionProcessReport> {
    let mut store = LocalMemoryStore::open_default()?;
    process_pending_extractions_with_store(&mut store, limit, dry_run)
}

fn process_pending_extractions_with_store(
    store: &mut LocalMemoryStore,
    limit: usize,
    dry_run: bool,
) -> Result<PendingExtractionProcessReport> {
    let pending = list_pending_extractions_on(store, limit)?;
    let facts = pending
        .iter()
        .flat_map(|record| extract_durable_facts(&record.raw_output))
        .collect::<Vec<_>>();
    if dry_run {
        return Ok(PendingExtractionProcessReport {
            pending_count: pending.len(),
            extracted_count: facts.len(),
            deleted_count: 0,
            dry_run,
            facts,
        });
    }
    let ids = pending.iter().map(|record| record.id).collect::<Vec<_>>();
    let deleted_count = store.transaction(|tx| {
        for record in &pending {
            let topic = format!("context-{}", record.project);
            let source = format!("pending-extraction:{}", record.tool_name);
            for fact in extract_durable_facts(&record.raw_output) {
                store_memory_on(
                    tx,
                    MemoryStoreInput {
                        content: &fact,
                        tags: Some("packet28,extracted"),
                        topic: Some(&topic),
                        importance: Some("medium"),
                        keywords: None,
                        project: Some(&record.project),
                        source: Some(&source),
                        raw_excerpt: Some(&record.raw_output),
                    },
                )?;
            }
        }
        delete_pending_extractions_on(tx, &ids)
    })?;
    Ok(PendingExtractionProcessReport {
        pending_count: pending.len(),
        extracted_count: facts.len(),
        deleted_count,
        dry_run,
        facts,
    })
}

pub(crate) fn record_hook_event(input: HookEventInput<'_>) -> Result<HookEventRecord> {
    let conn = LocalMemoryStore::open_default()?;
    let now = timestamp_unix_ms();
    conn.execute(
        "INSERT INTO hook_events
         (runtime, event_kind, session_id, task_id, matcher, payload_json, created_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            input.runtime,
            input.event_kind,
            input.session_id,
            input.task_id,
            input.matcher,
            input.payload_json,
            now
        ],
    )?;
    let id = conn.last_insert_rowid();
    Ok(HookEventRecord {
        id,
        runtime: input.runtime.to_string(),
        event_kind: input.event_kind.to_string(),
        session_id: input.session_id.map(ToOwned::to_owned),
        task_id: input.task_id.map(ToOwned::to_owned),
        matcher: input.matcher.map(ToOwned::to_owned),
        payload_json: input.payload_json.to_string(),
        created_at_unix_ms: now,
    })
}

pub(crate) fn list_hook_events(limit: usize) -> Result<Vec<HookEventRecord>> {
    let conn = LocalMemoryStore::open_default()?;
    list_hook_events_on(&conn, limit)
}

pub(super) fn list_hook_events_on(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<HookEventRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, runtime, event_kind, session_id, task_id, matcher, payload_json, created_at_unix_ms
         FROM hook_events
         ORDER BY created_at_unix_ms DESC, id DESC
         LIMIT ?1",
    )?;
    read_hook_event_rows(&mut stmt, params![limit.max(1) as i64])
}

pub(crate) fn hook_event_stats() -> Result<Vec<HookEventStats>> {
    let conn = LocalMemoryStore::open_default()?;
    let mut stmt = conn.prepare(
        "SELECT runtime, event_kind, COUNT(*) AS event_count
         FROM hook_events
         GROUP BY runtime, event_kind
         ORDER BY runtime, event_kind",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(HookEventStats {
            runtime: row.get(0)?,
            event_kind: row.get(1)?,
            event_count: row.get(2)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn read_hook_event_rows<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    params: P,
) -> Result<Vec<HookEventRecord>> {
    let rows = stmt.query_map(params, |row| {
        Ok(HookEventRecord {
            id: row.get(0)?,
            runtime: row.get(1)?,
            event_kind: row.get(2)?,
            session_id: row.get(3)?,
            task_id: row.get(4)?,
            matcher: row.get(5)?,
            payload_json: row.get(6)?,
            created_at_unix_ms: row.get(7)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn read_pending_extraction_rows<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    params: P,
) -> Result<Vec<PendingExtractionRecord>> {
    let rows = stmt.query_map(params, |row| {
        Ok(PendingExtractionRecord {
            id: row.get(0)?,
            project: row.get(1)?,
            tool_name: row.get(2)?,
            raw_output: row.get(3)?,
            captured_at_unix_ms: row.get(4)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn extract_durable_facts(raw_output: &str) -> Vec<String> {
    raw_output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let fact = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
                .or_else(|| trimmed.strip_prefix("FACT:"))
                .or_else(|| trimmed.strip_prefix("Fact:"))
                .unwrap_or(trimmed)
                .trim();
            if fact.is_empty()
                || fact.eq_ignore_ascii_case("none")
                || fact == "(none)"
                || fact.len() < 8
            {
                None
            } else {
                Some(fact.to_string())
            }
        })
        .take(20)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_extraction_batch_rolls_back_memories_when_delete_fails() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = LocalMemoryStore::open_path(temp.path().join("memory.db")).unwrap();
        store
            .execute(
                "INSERT INTO pending_extractions
                 (project, tool_name, raw_output, captured_at_unix_ms)
                 VALUES ('packet28', 'test', '- Durable fact from a tool result', 1)",
                [],
            )
            .unwrap();
        store
            .execute_batch(
                "
                CREATE TRIGGER fail_pending_delete
                BEFORE DELETE ON pending_extractions
                BEGIN
                    SELECT RAISE(ABORT, 'forced pending delete failure');
                END;
                ",
            )
            .unwrap();

        let result = process_pending_extractions_with_store(&mut store, 10, false);

        assert!(result.is_err());
        assert_eq!(table_count(&store, "pending_extractions").unwrap(), 1);
        assert_eq!(table_count(&store, "memories").unwrap(), 0);
        assert_eq!(store.metrics().transactions_rolled_back, 1);
        assert_eq!(store.metrics().connections_opened, 1);
    }
}
