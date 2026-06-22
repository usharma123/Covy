use anyhow::Result;
use rusqlite::{params, Connection};

use crate::memory_db::{
    expanded_filter_limit, fts_match_query, normalize_non_empty, open_memory_db, table_count,
    timestamp_unix_ms,
};
use crate::memory_store_types::*;

pub(crate) fn record_feedback_with_metadata(input: FeedbackInput<'_>) -> Result<FeedbackRecord> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    let topic = normalize_non_empty(input.topic, "general");
    conn.execute(
        "INSERT INTO feedback
         (subject, correction, topic, context, predicted, reason, source, project, applied_count, created_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
        params![
            input.subject,
            input.correction,
            topic,
            input.context,
            input.predicted,
            input.reason,
            input.source,
            input.project,
            now
        ],
    )?;
    get_feedback(&conn, conn.last_insert_rowid())
}

pub(crate) fn append_transcript_message(
    input: TranscriptAppendInput<'_>,
) -> Result<TranscriptMessage> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    let session_key = normalize_non_empty(input.session, &format!("transcript-{now}"));
    let role = normalize_non_empty(input.role, "assistant");
    let session_id = ensure_transcript_session(&conn, &session_key, input.agent, now)?;
    conn.execute(
        "INSERT INTO transcript_messages
         (session_id, role, content, source, project, created_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            session_id,
            role,
            input.content,
            input.source,
            input.project,
            now
        ],
    )?;
    conn.execute(
        "UPDATE transcript_sessions
         SET agent = COALESCE(?1, agent),
             updated_at_unix_ms = ?2
         WHERE id = ?3",
        params![input.agent, now, session_id],
    )?;
    get_transcript_message(&conn, conn.last_insert_rowid())
}

pub(crate) fn list_transcript_sessions(limit: usize) -> Result<Vec<TranscriptSession>> {
    let conn = open_memory_db()?;
    let mut stmt = conn.prepare(
        "SELECT
            s.id,
            s.session_key,
            s.agent,
            COUNT(m.id) AS message_count,
            s.started_at_unix_ms,
            s.updated_at_unix_ms
         FROM transcript_sessions s
         LEFT JOIN transcript_messages m ON m.session_id = s.id
         GROUP BY s.id
         ORDER BY s.updated_at_unix_ms DESC, s.id DESC
         LIMIT ?1",
    )?;
    read_transcript_session_rows(&mut stmt, params![limit.max(1) as i64])
}

pub(crate) fn show_transcript_session(
    session_key: &str,
    limit: usize,
) -> Result<Vec<TranscriptMessage>> {
    let conn = open_memory_db()?;
    let mut stmt = conn.prepare(
        "SELECT
            m.id, m.session_id, s.session_key, s.agent, m.role, m.content, m.source, m.project,
            m.created_at_unix_ms
         FROM transcript_messages m
         JOIN transcript_sessions s ON s.id = m.session_id
         WHERE s.session_key = ?1
         ORDER BY m.created_at_unix_ms ASC, m.id ASC
         LIMIT ?2",
    )?;
    read_transcript_message_rows(&mut stmt, params![session_key, limit.max(1) as i64])
}

pub(crate) fn search_transcripts(query: &str, limit: usize) -> Result<Vec<TranscriptMessage>> {
    search_transcripts_filtered(query, None, limit)
}

pub(crate) fn search_transcripts_filtered(
    query: &str,
    project: Option<&str>,
    limit: usize,
) -> Result<Vec<TranscriptMessage>> {
    let conn = open_memory_db()?;
    let expanded_limit = expanded_filter_limit(limit, project.is_some());
    if let Some(match_query) = fts_match_query(query) {
        let mut stmt = conn.prepare(
            "SELECT
                m.id, m.session_id, s.session_key, s.agent, m.role, m.content, m.source, m.project,
                m.created_at_unix_ms
             FROM transcript_messages_fts f
             JOIN transcript_messages m ON m.rowid = f.rowid
             JOIN transcript_sessions s ON s.id = m.session_id
             WHERE transcript_messages_fts MATCH ?1
             ORDER BY bm25(transcript_messages_fts), m.created_at_unix_ms DESC
             LIMIT ?2",
        )?;
        let records =
            read_transcript_message_rows(&mut stmt, params![match_query, expanded_limit as i64])?;
        let records = filter_transcript_records(records, project, limit);
        if !records.is_empty() {
            return Ok(records);
        }
    }
    let records = search_transcripts_like(&conn, query, expanded_limit)?;
    Ok(filter_transcript_records(records, project, limit))
}

pub(crate) fn transcript_stats() -> Result<TranscriptStats> {
    let conn = open_memory_db()?;
    Ok(TranscriptStats {
        session_count: table_count(&conn, "transcript_sessions")?,
        message_count: table_count(&conn, "transcript_messages")?,
        agent_count: conn.query_row(
            "SELECT COUNT(DISTINCT agent)
             FROM transcript_sessions
             WHERE agent IS NOT NULL AND TRIM(agent) != ''",
            [],
            |row| row.get(0),
        )?,
    })
}

pub(crate) fn search_feedback(query: &str, limit: usize) -> Result<Vec<FeedbackRecord>> {
    search_feedback_filtered(query, None, limit)
}

pub(crate) fn search_feedback_filtered(
    query: &str,
    project: Option<&str>,
    limit: usize,
) -> Result<Vec<FeedbackRecord>> {
    let conn = open_memory_db()?;
    let expanded_limit = expanded_filter_limit(limit, project.is_some());
    if let Some(match_query) = fts_match_query(query) {
        let mut stmt = conn.prepare(
            "SELECT
                fb.id, fb.subject, fb.correction, fb.topic, fb.context, fb.predicted,
                fb.reason, fb.source, fb.project, fb.applied_count, fb.created_at_unix_ms
             FROM feedback_fts_all f
             JOIN feedback fb ON fb.rowid = f.rowid
             WHERE feedback_fts_all MATCH ?1
             ORDER BY bm25(feedback_fts_all), fb.created_at_unix_ms DESC
             LIMIT ?2",
        )?;
        let records = read_feedback_rows(&mut stmt, params![match_query, expanded_limit as i64])?;
        let records = filter_feedback_records(records, project, limit);
        if !records.is_empty() {
            return Ok(records);
        }
    }
    let records = search_feedback_like(&conn, query, expanded_limit)?;
    Ok(filter_feedback_records(records, project, limit))
}

fn search_feedback_like(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<FeedbackRecord>> {
    let pattern = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT
            id, subject, correction, topic, context, predicted, reason, source, project,
            applied_count, created_at_unix_ms
         FROM feedback
         WHERE subject LIKE ?1
            OR correction LIKE ?1
            OR IFNULL(topic, '') LIKE ?1
            OR IFNULL(context, '') LIKE ?1
            OR IFNULL(predicted, '') LIKE ?1
            OR IFNULL(reason, '') LIKE ?1
            OR IFNULL(source, '') LIKE ?1
            OR IFNULL(project, '') LIKE ?1
         ORDER BY created_at_unix_ms DESC
         LIMIT ?2",
    )?;
    read_feedback_rows(&mut stmt, params![pattern, limit.max(1) as i64])
}

pub(crate) fn list_feedback(topic: Option<&str>, limit: usize) -> Result<Vec<FeedbackRecord>> {
    let conn = open_memory_db()?;
    if let Some(topic) = topic {
        let topic = normalize_non_empty(Some(topic), "general");
        let mut stmt = conn.prepare(
            "SELECT
                id, subject, correction, topic, context, predicted, reason, source, project,
                applied_count, created_at_unix_ms
             FROM feedback
             WHERE topic = ?1
             ORDER BY created_at_unix_ms DESC
             LIMIT ?2",
        )?;
        read_feedback_rows(&mut stmt, params![topic, limit.max(1) as i64])
    } else {
        let mut stmt = conn.prepare(
            "SELECT
                id, subject, correction, topic, context, predicted, reason, source, project,
                applied_count, created_at_unix_ms
             FROM feedback
             ORDER BY created_at_unix_ms DESC
             LIMIT ?1",
        )?;
        read_feedback_rows(&mut stmt, params![limit.max(1) as i64])
    }
}

pub(crate) fn delete_feedback(id: i64) -> Result<usize> {
    let conn = open_memory_db()?;
    conn.execute("DELETE FROM feedback WHERE id = ?1", params![id])
        .map_err(Into::into)
}

pub(crate) fn apply_feedback(id: i64) -> Result<FeedbackRecord> {
    let conn = open_memory_db()?;
    conn.execute(
        "UPDATE feedback SET applied_count = applied_count + 1 WHERE id = ?1",
        params![id],
    )?;
    get_feedback(&conn, id)
}

pub(crate) fn feedback_stats() -> Result<FeedbackStats> {
    let conn = open_memory_db()?;
    Ok(FeedbackStats {
        feedback_count: table_count(&conn, "feedback")?,
        applied_count: conn.query_row(
            "SELECT COALESCE(SUM(applied_count), 0) FROM feedback",
            [],
            |row| row.get(0),
        )?,
        topic_count: conn.query_row("SELECT COUNT(DISTINCT topic) FROM feedback", [], |row| {
            row.get(0)
        })?,
    })
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
            topic: row.get(3)?,
            context: row.get(4)?,
            predicted: row.get(5)?,
            reason: row.get(6)?,
            source: row.get(7)?,
            project: row.get(8)?,
            applied_count: row.get(9)?,
            created_at_unix_ms: row.get(10)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn filter_feedback_records(
    records: Vec<FeedbackRecord>,
    project: Option<&str>,
    limit: usize,
) -> Vec<FeedbackRecord> {
    let project = project.map(|project| normalize_non_empty(Some(project), "default"));
    records
        .into_iter()
        .filter(|record| {
            project
                .as_deref()
                .map_or(true, |wanted| record.project.as_deref() == Some(wanted))
        })
        .take(limit.max(1))
        .collect()
}

fn search_transcripts_like(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<TranscriptMessage>> {
    let pattern = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT
            m.id, m.session_id, s.session_key, s.agent, m.role, m.content, m.source, m.project,
            m.created_at_unix_ms
         FROM transcript_messages m
         JOIN transcript_sessions s ON s.id = m.session_id
         WHERE m.content LIKE ?1
            OR m.role LIKE ?1
            OR IFNULL(m.source, '') LIKE ?1
            OR IFNULL(m.project, '') LIKE ?1
            OR s.session_key LIKE ?1
            OR IFNULL(s.agent, '') LIKE ?1
         ORDER BY m.created_at_unix_ms DESC, m.id DESC
         LIMIT ?2",
    )?;
    read_transcript_message_rows(&mut stmt, params![pattern, limit.max(1) as i64])
}

fn filter_transcript_records(
    records: Vec<TranscriptMessage>,
    project: Option<&str>,
    limit: usize,
) -> Vec<TranscriptMessage> {
    let project = project.map(|project| normalize_non_empty(Some(project), "default"));
    records
        .into_iter()
        .filter(|record| {
            project
                .as_deref()
                .map_or(true, |wanted| record.project.as_deref() == Some(wanted))
        })
        .take(limit.max(1))
        .collect()
}

fn read_transcript_session_rows<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    params: P,
) -> Result<Vec<TranscriptSession>> {
    let rows = stmt.query_map(params, |row| {
        Ok(TranscriptSession {
            id: row.get(0)?,
            session_key: row.get(1)?,
            agent: row.get(2)?,
            message_count: row.get(3)?,
            started_at_unix_ms: row.get(4)?,
            updated_at_unix_ms: row.get(5)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn read_transcript_message_rows<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    params: P,
) -> Result<Vec<TranscriptMessage>> {
    let rows = stmt.query_map(params, |row| {
        Ok(TranscriptMessage {
            id: row.get(0)?,
            session_id: row.get(1)?,
            session_key: row.get(2)?,
            agent: row.get(3)?,
            role: row.get(4)?,
            content: row.get(5)?,
            source: row.get(6)?,
            project: row.get(7)?,
            created_at_unix_ms: row.get(8)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn get_feedback(conn: &Connection, id: i64) -> Result<FeedbackRecord> {
    conn.query_row(
        "SELECT
            id, subject, correction, topic, context, predicted, reason, source, project,
            applied_count, created_at_unix_ms
         FROM feedback
         WHERE id = ?1",
        params![id],
        |row| {
            Ok(FeedbackRecord {
                id: row.get(0)?,
                subject: row.get(1)?,
                correction: row.get(2)?,
                topic: row.get(3)?,
                context: row.get(4)?,
                predicted: row.get(5)?,
                reason: row.get(6)?,
                source: row.get(7)?,
                project: row.get(8)?,
                applied_count: row.get(9)?,
                created_at_unix_ms: row.get(10)?,
            })
        },
    )
    .map_err(Into::into)
}

fn ensure_transcript_session(
    conn: &Connection,
    session_key: &str,
    agent: Option<&str>,
    now: i64,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO transcript_sessions
         (session_key, agent, started_at_unix_ms, updated_at_unix_ms)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(session_key) DO UPDATE SET
             agent = COALESCE(excluded.agent, transcript_sessions.agent),
             updated_at_unix_ms = excluded.updated_at_unix_ms",
        params![session_key, agent, now],
    )?;
    conn.query_row(
        "SELECT id FROM transcript_sessions WHERE session_key = ?1",
        params![session_key],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn get_transcript_message(conn: &Connection, id: i64) -> Result<TranscriptMessage> {
    conn.query_row(
        "SELECT
            m.id, m.session_id, s.session_key, s.agent, m.role, m.content, m.source, m.project,
            m.created_at_unix_ms
         FROM transcript_messages m
         JOIN transcript_sessions s ON s.id = m.session_id
         WHERE m.id = ?1",
        params![id],
        |row| {
            Ok(TranscriptMessage {
                id: row.get(0)?,
                session_id: row.get(1)?,
                session_key: row.get(2)?,
                agent: row.get(3)?,
                role: row.get(4)?,
                content: row.get(5)?,
                source: row.get(6)?,
                project: row.get(7)?,
                created_at_unix_ms: row.get(8)?,
            })
        },
    )
    .map_err(Into::into)
}
