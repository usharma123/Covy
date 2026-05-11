use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryRecord {
    pub(crate) id: i64,
    pub(crate) content: String,
    pub(crate) tags: Option<String>,
    pub(crate) topic: String,
    pub(crate) importance: String,
    pub(crate) keywords: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) raw_excerpt: Option<String>,
    pub(crate) weight: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) recall_score: Option<f64>,
    pub(crate) created_at_unix_ms: i64,
    pub(crate) updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FeedbackRecord {
    pub(crate) id: i64,
    pub(crate) subject: String,
    pub(crate) correction: String,
    pub(crate) topic: String,
    pub(crate) context: Option<String>,
    pub(crate) predicted: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) applied_count: i64,
    pub(crate) created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TranscriptSession {
    pub(crate) id: i64,
    pub(crate) session_key: String,
    pub(crate) agent: Option<String>,
    pub(crate) message_count: i64,
    pub(crate) started_at_unix_ms: i64,
    pub(crate) updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TranscriptMessage {
    pub(crate) id: i64,
    pub(crate) session_id: i64,
    pub(crate) session_key: String,
    pub(crate) agent: Option<String>,
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) source: Option<String>,
    pub(crate) created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TranscriptStats {
    pub(crate) session_count: i64,
    pub(crate) message_count: i64,
    pub(crate) agent_count: i64,
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
pub(crate) struct GraphDeleteReport {
    pub(crate) deleted_concepts: usize,
    pub(crate) deleted_relations: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphExport {
    pub(crate) format: String,
    pub(crate) content: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphRelationTypeStats {
    pub(crate) relation: String,
    pub(crate) relation_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphStats {
    pub(crate) concept_count: i64,
    pub(crate) relation_count: i64,
    pub(crate) relation_type_count: i64,
    pub(crate) isolated_concept_count: i64,
    pub(crate) relation_types: Vec<GraphRelationTypeStats>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProjectLearnReport {
    pub(crate) project_name: String,
    pub(crate) project_root: String,
    pub(crate) total_concepts: usize,
    pub(crate) link_count: usize,
    pub(crate) concepts: Vec<GraphConcept>,
    pub(crate) relations: Vec<GraphRelation>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LocalStoreStats {
    pub(crate) memory_count: i64,
    pub(crate) memory_embedding_count: i64,
    pub(crate) feedback_count: i64,
    pub(crate) concept_count: i64,
    pub(crate) relation_count: i64,
    pub(crate) transcript_session_count: i64,
    pub(crate) transcript_message_count: i64,
    pub(crate) mcp_call_count: i64,
    pub(crate) hook_event_count: i64,
    pub(crate) pending_extraction_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PendingExtractionRecord {
    pub(crate) id: i64,
    pub(crate) project: String,
    pub(crate) tool_name: String,
    pub(crate) raw_output: String,
    pub(crate) captured_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PendingExtractionProcessReport {
    pub(crate) pending_count: usize,
    pub(crate) extracted_count: usize,
    pub(crate) deleted_count: usize,
    pub(crate) dry_run: bool,
    pub(crate) facts: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HookEventRecord {
    pub(crate) id: i64,
    pub(crate) runtime: String,
    pub(crate) event_kind: String,
    pub(crate) session_id: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) matcher: Option<String>,
    pub(crate) payload_json: String,
    pub(crate) created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HookEventStats {
    pub(crate) runtime: String,
    pub(crate) event_kind: String,
    pub(crate) event_count: i64,
}

pub(crate) struct HookEventInput<'a> {
    pub(crate) runtime: &'a str,
    pub(crate) event_kind: &'a str,
    pub(crate) session_id: Option<&'a str>,
    pub(crate) task_id: Option<&'a str>,
    pub(crate) matcher: Option<&'a str>,
    pub(crate) payload_json: &'a str,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingExtractionInput<'a> {
    pub(crate) project: Option<&'a str>,
    pub(crate) tool_name: Option<&'a str>,
    pub(crate) raw_output: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryTopicStats {
    pub(crate) topic: String,
    pub(crate) memory_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryHealthTopic {
    pub(crate) topic: String,
    pub(crate) memory_count: i64,
    pub(crate) stale_count: i64,
    pub(crate) oldest_age_days: i64,
    pub(crate) newest_age_days: i64,
    pub(crate) consolidation_needed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryHealthReport {
    pub(crate) topic_filter: Option<String>,
    pub(crate) stale_after_days: i64,
    pub(crate) consolidation_threshold: i64,
    pub(crate) total_topics: usize,
    pub(crate) total_memories: i64,
    pub(crate) stale_memories: i64,
    pub(crate) topics_needing_consolidation: i64,
    pub(crate) topics: Vec<MemoryHealthTopic>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryConsolidationReport {
    pub(crate) topic: String,
    pub(crate) source_count: usize,
    pub(crate) status: String,
    pub(crate) keep_originals: bool,
    pub(crate) consolidated_memory: Option<MemoryRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryDecayReport {
    pub(crate) factor: f64,
    pub(crate) decayed_count: usize,
    pub(crate) skipped_critical_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryPruneReport {
    pub(crate) threshold: f64,
    pub(crate) dry_run: bool,
    pub(crate) candidate_count: usize,
    pub(crate) deleted_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryEmbeddingRecord {
    pub(crate) memory_id: i64,
    pub(crate) model: String,
    pub(crate) dimensions: usize,
    pub(crate) embedding_json: String,
    pub(crate) created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryEmbedReport {
    pub(crate) model: String,
    pub(crate) dimensions: usize,
    pub(crate) embedded_count: usize,
    pub(crate) embeddings: Vec<MemoryEmbeddingRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FeedbackStats {
    pub(crate) feedback_count: i64,
    pub(crate) applied_count: i64,
    pub(crate) topic_count: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct TranscriptAppendInput<'a> {
    pub(crate) session: Option<&'a str>,
    pub(crate) agent: Option<&'a str>,
    pub(crate) role: Option<&'a str>,
    pub(crate) content: &'a str,
    pub(crate) source: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(crate) struct FeedbackInput<'a> {
    pub(crate) subject: &'a str,
    pub(crate) correction: &'a str,
    pub(crate) topic: Option<&'a str>,
    pub(crate) context: Option<&'a str>,
    pub(crate) predicted: Option<&'a str>,
    pub(crate) reason: Option<&'a str>,
    pub(crate) source: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryStoreInput<'a> {
    pub(crate) content: &'a str,
    pub(crate) tags: Option<&'a str>,
    pub(crate) topic: Option<&'a str>,
    pub(crate) importance: Option<&'a str>,
    pub(crate) keywords: Option<&'a str>,
    pub(crate) project: Option<&'a str>,
    pub(crate) source: Option<&'a str>,
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
    pub(crate) project: Option<&'a str>,
    pub(crate) source: Option<&'a str>,
    pub(crate) raw_excerpt: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MemoryRecallQuery<'a> {
    pub(crate) query: &'a str,
    pub(crate) limit: usize,
    pub(crate) topic: Option<&'a str>,
    pub(crate) project: Option<&'a str>,
    pub(crate) tag: Option<&'a str>,
    pub(crate) keyword: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MemoryListQuery<'a> {
    pub(crate) limit: usize,
    pub(crate) topic: Option<&'a str>,
    pub(crate) project: Option<&'a str>,
    pub(crate) all: bool,
    pub(crate) sort: &'a str,
}

impl MemoryRecallQuery<'_> {
    fn has_filters(&self) -> bool {
        self.topic
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
            || self
                .project
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
            || self
                .tag
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
            || self
                .keyword
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
    }
}

pub(crate) fn store_memory_with_metadata(input: MemoryStoreInput<'_>) -> Result<MemoryRecord> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    let topic = normalize_non_empty(input.topic, "general");
    let importance = normalize_importance(input.importance)?;
    conn.execute(
        "INSERT INTO memories
         (content, tags, topic, importance, keywords, project, source, raw_excerpt, weight, created_at_unix_ms, updated_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        params![
            input.content,
            input.tags,
            topic,
            importance,
            input.keywords,
            input.project,
            input.source,
            input.raw_excerpt,
            1.0_f64,
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

pub(crate) fn recall_memories_filtered(input: MemoryRecallQuery<'_>) -> Result<Vec<MemoryRecord>> {
    let conn = open_memory_db()?;
    let expanded_limit = expanded_filter_limit(input.limit, input.has_filters());
    if let Some(match_query) = fts_match_query(input.query) {
        let mut stmt = conn.prepare(
            "SELECT
                m.id, m.content, m.tags, m.topic, m.importance, m.keywords, m.project, m.source, m.raw_excerpt, m.weight,
                m.created_at_unix_ms, m.updated_at_unix_ms, bm25(memories_fts) AS recall_score
             FROM memories_fts f
             JOIN memories m ON m.rowid = f.rowid
             WHERE memories_fts MATCH ?1
             ORDER BY recall_score, m.created_at_unix_ms DESC
             LIMIT ?2",
        )?;
        let records = read_memory_rows(&mut stmt, params![match_query, expanded_limit as i64])?;
        let records = filter_memory_records(records, input);
        if !records.is_empty() {
            return Ok(limit_memory_records(records, input.limit));
        }
    }
    let vector_records = recall_memories_vector(&conn, input, expanded_limit)?;
    let vector_records = filter_memory_records(vector_records, input);
    if !vector_records.is_empty() {
        return Ok(limit_memory_records(vector_records, input.limit));
    }
    let records = recall_memories_like(&conn, input.query, expanded_limit)?;
    Ok(limit_memory_records(
        filter_memory_records(records, input),
        input.limit,
    ))
}

fn recall_memories_like(conn: &Connection, query: &str, limit: usize) -> Result<Vec<MemoryRecord>> {
    let pattern = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT
            id, content, tags, topic, importance, keywords, project, source, raw_excerpt, weight,
            created_at_unix_ms, updated_at_unix_ms
         FROM memories
         WHERE content LIKE ?1
            OR IFNULL(tags, '') LIKE ?1
            OR IFNULL(topic, '') LIKE ?1
            OR IFNULL(keywords, '') LIKE ?1
            OR IFNULL(project, '') LIKE ?1
            OR IFNULL(source, '') LIKE ?1
            OR IFNULL(raw_excerpt, '') LIKE ?1
         ORDER BY created_at_unix_ms DESC
         LIMIT ?2",
    )?;
    read_memory_rows(&mut stmt, params![pattern, limit.max(1) as i64])
}

fn recall_memories_vector(
    conn: &Connection,
    input: MemoryRecallQuery<'_>,
    limit: usize,
) -> Result<Vec<MemoryRecord>> {
    let mut stmt = conn.prepare(
        "SELECT
            m.id, m.content, m.tags, m.topic, m.importance, m.keywords, m.project, m.source, m.raw_excerpt, m.weight,
            m.created_at_unix_ms, m.updated_at_unix_ms,
            e.dimensions, e.embedding_json
         FROM memory_embeddings e
         JOIN memories m ON m.id = e.memory_id
         WHERE e.model = 'packet28-local-hash-v1'",
    )?;
    let rows = stmt.query_map([], |row| {
        let dimensions: i64 = row.get(12)?;
        Ok((
            MemoryRecord {
                id: row.get(0)?,
                content: row.get(1)?,
                tags: row.get(2)?,
                topic: row.get(3)?,
                importance: row.get(4)?,
                keywords: row.get(5)?,
                project: row.get(6)?,
                source: row.get(7)?,
                raw_excerpt: row.get(8)?,
                weight: row.get(9)?,
                recall_score: None,
                created_at_unix_ms: row.get(10)?,
                updated_at_unix_ms: row.get(11)?,
            },
            dimensions.max(0) as usize,
            row.get::<_, String>(13)?,
        ))
    })?;
    let mut records = Vec::new();
    for row in rows {
        let (mut record, dimensions, embedding_json) = row?;
        let Ok(embedding) = serde_json::from_str::<Vec<f64>>(&embedding_json) else {
            continue;
        };
        if dimensions == 0 || embedding.is_empty() {
            continue;
        }
        let query_embedding = deterministic_embedding(input.query, dimensions);
        let score = cosine_similarity(&query_embedding, &embedding);
        if score > 0.0 {
            record.recall_score = Some(score);
            records.push(record);
        }
    }
    records.sort_by(|a, b| {
        b.recall_score
            .partial_cmp(&a.recall_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.updated_at_unix_ms.cmp(&a.updated_at_unix_ms))
    });
    records.truncate(limit.max(1));
    Ok(records)
}

pub(crate) fn list_memories(limit: usize) -> Result<Vec<MemoryRecord>> {
    list_memories_filtered(MemoryListQuery {
        limit,
        topic: None,
        project: None,
        all: false,
        sort: "recent",
    })
}

pub(crate) fn list_memories_filtered(input: MemoryListQuery<'_>) -> Result<Vec<MemoryRecord>> {
    let conn = open_memory_db()?;
    let limit = if input.all {
        10_000
    } else {
        input.limit.max(1)
    };
    let order_by = match input.sort.trim().to_ascii_lowercase().as_str() {
        "oldest" => "created_at_unix_ms ASC, id ASC",
        "importance" => {
            "CASE LOWER(importance) WHEN 'critical' THEN 4 WHEN 'high' THEN 3 WHEN 'medium' THEN 2 WHEN 'low' THEN 1 ELSE 2 END DESC, updated_at_unix_ms DESC"
        }
        "weight" => "weight DESC, updated_at_unix_ms DESC",
        "recent" | "newest" | "" => "created_at_unix_ms DESC, id DESC",
        other => anyhow::bail!("unsupported memory list sort '{other}'"),
    };
    let topic = input
        .topic
        .map(|topic| normalize_non_empty(Some(topic), "general"));
    let sql = if topic.is_some() {
        format!(
            "SELECT
                id, content, tags, topic, importance, keywords, project, source, raw_excerpt, weight,
                created_at_unix_ms, updated_at_unix_ms
             FROM memories
             WHERE topic = ?1{project_filter}
             ORDER BY {order_by}
             LIMIT ?2",
            project_filter = if input.project.is_some() { " AND project = ?3" } else { "" }
        )
    } else {
        format!(
            "SELECT
                id, content, tags, topic, importance, keywords, project, source, raw_excerpt, weight,
                created_at_unix_ms, updated_at_unix_ms
             FROM memories
             {project_where}
             ORDER BY {order_by}
             LIMIT ?1",
            project_where = if input.project.is_some() { "WHERE project = ?2" } else { "" }
        )
    };
    let mut stmt = conn.prepare(&sql)?;
    let project = input
        .project
        .map(|project| normalize_non_empty(Some(project), "default"));
    if let Some(topic) = topic {
        if let Some(project) = project {
            read_memory_rows(&mut stmt, params![topic, limit as i64, project])
        } else {
            read_memory_rows(&mut stmt, params![topic, limit as i64])
        }
    } else if let Some(project) = project {
        read_memory_rows(&mut stmt, params![limit as i64, project])
    } else {
        read_memory_rows(&mut stmt, params![limit as i64])
    }
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
    let project = input.project.or(current.project.as_deref());
    let source = input.source.or(current.source.as_deref());
    let raw_excerpt = input.raw_excerpt.or(current.raw_excerpt.as_deref());
    conn.execute(
        "UPDATE memories
         SET content = ?1,
             tags = ?2,
             topic = ?3,
             importance = ?4,
             keywords = ?5,
             project = ?6,
             source = ?7,
             raw_excerpt = ?8,
             updated_at_unix_ms = ?9
         WHERE id = ?10",
        params![
            content,
            tags,
            normalize_non_empty(Some(topic), "general"),
            normalize_importance(Some(importance))?,
            keywords,
            project,
            source,
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

pub(crate) fn memory_health(
    topic_filter: Option<&str>,
    stale_after_days: i64,
    consolidation_threshold: i64,
) -> Result<MemoryHealthReport> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    let day_ms = 86_400_000_i64;
    let stale_after_days = stale_after_days.max(0);
    let consolidation_threshold = consolidation_threshold.max(1);
    let stale_cutoff = now.saturating_sub(stale_after_days.saturating_mul(day_ms));
    let mut topics = if let Some(topic) = topic_filter {
        let topic = normalize_non_empty(Some(topic), "general");
        let mut stmt = conn.prepare(
            "SELECT
                topic,
                COUNT(*),
                SUM(CASE WHEN updated_at_unix_ms <= ?1 THEN 1 ELSE 0 END),
                MIN(updated_at_unix_ms),
                MAX(updated_at_unix_ms)
             FROM memories
             WHERE topic = ?2
             GROUP BY topic
             ORDER BY topic ASC",
        )?;
        read_health_rows(
            &mut stmt,
            params![stale_cutoff, topic],
            now,
            day_ms,
            consolidation_threshold,
        )?
    } else {
        let mut stmt = conn.prepare(
            "SELECT
                topic,
                COUNT(*),
                SUM(CASE WHEN updated_at_unix_ms <= ?1 THEN 1 ELSE 0 END),
                MIN(updated_at_unix_ms),
                MAX(updated_at_unix_ms)
             FROM memories
             GROUP BY topic
             ORDER BY COUNT(*) DESC, topic ASC",
        )?;
        read_health_rows(
            &mut stmt,
            params![stale_cutoff],
            now,
            day_ms,
            consolidation_threshold,
        )?
    };
    topics.sort_by(|a, b| {
        b.consolidation_needed
            .cmp(&a.consolidation_needed)
            .then_with(|| b.memory_count.cmp(&a.memory_count))
            .then_with(|| a.topic.cmp(&b.topic))
    });
    let total_memories = topics.iter().map(|topic| topic.memory_count).sum();
    let stale_memories = topics.iter().map(|topic| topic.stale_count).sum();
    let topics_needing_consolidation = topics
        .iter()
        .filter(|topic| topic.consolidation_needed)
        .count() as i64;
    Ok(MemoryHealthReport {
        topic_filter: topic_filter.map(|topic| normalize_non_empty(Some(topic), "general")),
        stale_after_days,
        consolidation_threshold,
        total_topics: topics.len(),
        total_memories,
        stale_memories,
        topics_needing_consolidation,
        topics,
    })
}

pub(crate) fn decay_memories(factor: f64) -> Result<MemoryDecayReport> {
    let factor = factor.clamp(0.0, 1.0);
    let conn = open_memory_db()?;
    let decayed_count = conn.execute(
        "UPDATE memories
         SET weight = weight * ?1,
             updated_at_unix_ms = ?2
         WHERE LOWER(importance) != 'critical'",
        params![factor, timestamp_unix_ms()],
    )?;
    let skipped_critical_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE LOWER(importance) = 'critical'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or_default() as usize;
    Ok(MemoryDecayReport {
        factor,
        decayed_count,
        skipped_critical_count,
    })
}

pub(crate) fn prune_memories(threshold: f64, dry_run: bool) -> Result<MemoryPruneReport> {
    let threshold = threshold.clamp(0.0, 1.0);
    let conn = open_memory_db()?;
    let mut stmt = conn.prepare(
        "SELECT id FROM memories
         WHERE weight < ?1 AND LOWER(importance) != 'critical'
         ORDER BY weight ASC, updated_at_unix_ms ASC",
    )?;
    let candidate_ids = stmt
        .query_map(params![threshold], |row| row.get::<_, i64>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let candidate_count = candidate_ids.len();
    drop(stmt);
    let mut deleted_count = 0;
    if !dry_run {
        for id in &candidate_ids {
            conn.execute(
                "DELETE FROM memory_chunks WHERE memory_id = ?1",
                params![id],
            )?;
            deleted_count += conn.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        }
    }
    Ok(MemoryPruneReport {
        threshold,
        dry_run,
        candidate_count,
        deleted_count,
    })
}

pub(crate) fn consolidate_memories(
    topic: Option<&str>,
    keep_originals: bool,
) -> Result<MemoryConsolidationReport> {
    let topic = normalize_non_empty(topic, "general");
    let conn = open_memory_db()?;
    let mut stmt = conn.prepare(
        "SELECT
            id, content, tags, topic, importance, keywords, project, source, raw_excerpt, weight,
            created_at_unix_ms, updated_at_unix_ms
         FROM memories
         WHERE topic = ?1
         ORDER BY updated_at_unix_ms DESC, id DESC
         LIMIT 100",
    )?;
    let memories = read_memory_rows(&mut stmt, params![topic.as_str()])?;
    if memories.is_empty() {
        return Ok(MemoryConsolidationReport {
            topic,
            source_count: 0,
            status: "no_memories".to_string(),
            keep_originals,
            consolidated_memory: None,
        });
    }
    if memories.len() == 1 {
        return Ok(MemoryConsolidationReport {
            topic,
            source_count: 1,
            status: "single_memory_noop".to_string(),
            keep_originals,
            consolidated_memory: Some(memories[0].clone()),
        });
    }
    drop(stmt);
    drop(conn);

    let content = render_consolidated_memory(&topic, &memories);
    let tags = merge_csv_field(memories.iter().filter_map(|memory| memory.tags.as_deref()));
    let keywords = merge_csv_field(
        memories
            .iter()
            .filter_map(|memory| memory.keywords.as_deref()),
    );
    let source = merge_csv_field(
        memories
            .iter()
            .filter_map(|memory| memory.source.as_deref()),
    );
    let project = merge_csv_field(
        memories
            .iter()
            .filter_map(|memory| memory.project.as_deref()),
    );
    let raw_excerpt = render_consolidated_raw_excerpt(&memories);
    let importance = consolidated_importance(&memories);
    let source_ids: Vec<i64> = memories.iter().map(|memory| memory.id).collect();

    let conn = open_memory_db()?;
    if !keep_originals {
        let tx = conn.unchecked_transaction()?;
        for id in &source_ids {
            tx.execute(
                "DELETE FROM memory_chunks WHERE memory_id = ?1",
                params![id],
            )?;
            tx.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
    }
    drop(conn);

    let consolidated = store_memory_with_metadata(MemoryStoreInput {
        content: &content,
        tags: tags.as_deref(),
        topic: Some(&topic),
        importance: Some(&importance),
        keywords: keywords.as_deref(),
        project: project.as_deref(),
        source: source.as_deref(),
        raw_excerpt: raw_excerpt.as_deref(),
    })?;
    Ok(MemoryConsolidationReport {
        topic,
        source_count: source_ids.len(),
        status: "consolidated".to_string(),
        keep_originals,
        consolidated_memory: Some(consolidated),
    })
}

pub(crate) fn embed_memory(id: i64, dimensions: usize) -> Result<MemoryEmbeddingRecord> {
    let conn = open_memory_db()?;
    let memory = get_memory(&conn, id)?;
    let embedding = deterministic_embedding(&memory.content, dimensions);
    let embedding_json = serde_json::to_string(&embedding)?;
    let now = timestamp_unix_ms();
    let model = "packet28-local-hash-v1";
    conn.execute(
        "INSERT INTO memory_embeddings
         (memory_id, model, dimensions, embedding_json, created_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(memory_id, model) DO UPDATE SET
             dimensions = excluded.dimensions,
             embedding_json = excluded.embedding_json,
             created_at_unix_ms = excluded.created_at_unix_ms",
        params![id, model, dimensions.max(1) as i64, embedding_json, now],
    )?;
    get_memory_embedding(&conn, id, model)
}

pub(crate) fn embed_memories(
    id: Option<i64>,
    all: bool,
    dimensions: usize,
) -> Result<MemoryEmbedReport> {
    let dimensions = dimensions.clamp(8, 4096);
    let embeddings = if let Some(id) = id {
        vec![embed_memory(id, dimensions)?]
    } else if all {
        let memories = list_memories(10_000)?;
        let mut records = Vec::with_capacity(memories.len());
        for memory in memories {
            records.push(embed_memory(memory.id, dimensions)?);
        }
        records
    } else {
        anyhow::bail!("pass a memory id or --all");
    };
    Ok(MemoryEmbedReport {
        model: "packet28-local-hash-v1".to_string(),
        dimensions,
        embedded_count: embeddings.len(),
        embeddings,
    })
}

pub(crate) fn record_feedback_with_metadata(input: FeedbackInput<'_>) -> Result<FeedbackRecord> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    let topic = normalize_non_empty(input.topic, "general");
    conn.execute(
        "INSERT INTO feedback
         (subject, correction, topic, context, predicted, reason, source, applied_count, created_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)",
        params![
            input.subject,
            input.correction,
            topic,
            input.context,
            input.predicted,
            input.reason,
            input.source,
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
         (session_id, role, content, source, created_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![session_id, role, input.content, input.source, now],
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
            m.id, m.session_id, s.session_key, s.agent, m.role, m.content, m.source,
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
    let conn = open_memory_db()?;
    if let Some(match_query) = fts_match_query(query) {
        let mut stmt = conn.prepare(
            "SELECT
                m.id, m.session_id, s.session_key, s.agent, m.role, m.content, m.source,
                m.created_at_unix_ms
             FROM transcript_messages_fts f
             JOIN transcript_messages m ON m.rowid = f.rowid
             JOIN transcript_sessions s ON s.id = m.session_id
             WHERE transcript_messages_fts MATCH ?1
             ORDER BY bm25(transcript_messages_fts), m.created_at_unix_ms DESC
             LIMIT ?2",
        )?;
        let records =
            read_transcript_message_rows(&mut stmt, params![match_query, limit.max(1) as i64])?;
        if !records.is_empty() {
            return Ok(records);
        }
    }
    search_transcripts_like(&conn, query, limit)
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
    let conn = open_memory_db()?;
    if let Some(match_query) = fts_match_query(query) {
        let mut stmt = conn.prepare(
            "SELECT
                fb.id, fb.subject, fb.correction, fb.topic, fb.context, fb.predicted,
                fb.reason, fb.source, fb.applied_count, fb.created_at_unix_ms
             FROM feedback_fts_all f
             JOIN feedback fb ON fb.rowid = f.rowid
             WHERE feedback_fts_all MATCH ?1
             ORDER BY bm25(feedback_fts_all), fb.created_at_unix_ms DESC
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
        "SELECT
            id, subject, correction, topic, context, predicted, reason, source,
            applied_count, created_at_unix_ms
         FROM feedback
         WHERE subject LIKE ?1
            OR correction LIKE ?1
            OR IFNULL(topic, '') LIKE ?1
            OR IFNULL(context, '') LIKE ?1
            OR IFNULL(predicted, '') LIKE ?1
            OR IFNULL(reason, '') LIKE ?1
            OR IFNULL(source, '') LIKE ?1
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
                id, subject, correction, topic, context, predicted, reason, source,
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
                id, subject, correction, topic, context, predicted, reason, source,
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
            applied_count: row.get(8)?,
            created_at_unix_ms: row.get(9)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn search_transcripts_like(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<TranscriptMessage>> {
    let pattern = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT
            m.id, m.session_id, s.session_key, s.agent, m.role, m.content, m.source,
            m.created_at_unix_ms
         FROM transcript_messages m
         JOIN transcript_sessions s ON s.id = m.session_id
         WHERE m.content LIKE ?1
            OR m.role LIKE ?1
            OR IFNULL(m.source, '') LIKE ?1
            OR s.session_key LIKE ?1
            OR IFNULL(s.agent, '') LIKE ?1
         ORDER BY m.created_at_unix_ms DESC, m.id DESC
         LIMIT ?2",
    )?;
    read_transcript_message_rows(&mut stmt, params![pattern, limit.max(1) as i64])
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
            created_at_unix_ms: row.get(7)?,
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

pub(crate) fn refine_concept(name: &str, description: &str) -> Result<GraphConcept> {
    add_concept(name, Some(description))
}

pub(crate) fn delete_concept(name: &str) -> Result<GraphDeleteReport> {
    let conn = open_memory_db()?;
    let concept_id = conn
        .query_row(
            "SELECT id FROM concepts WHERE name = ?1",
            params![name],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let Some(concept_id) = concept_id else {
        return Ok(GraphDeleteReport {
            deleted_concepts: 0,
            deleted_relations: 0,
        });
    };
    let deleted_relations = conn.execute(
        "DELETE FROM relations WHERE source_concept_id = ?1 OR target_concept_id = ?1",
        params![concept_id],
    )?;
    let deleted_concepts =
        conn.execute("DELETE FROM concepts WHERE id = ?1", params![concept_id])?;
    Ok(GraphDeleteReport {
        deleted_concepts,
        deleted_relations,
    })
}

pub(crate) fn search_concepts(query: &str, limit: usize) -> Result<Vec<GraphConcept>> {
    let conn = open_memory_db()?;
    if let Some(match_query) = fts_match_query(query) {
        let mut stmt = conn.prepare(
            "SELECT c.id, c.name, c.description
             FROM concepts_fts f
             JOIN concepts c ON c.rowid = f.rowid
             WHERE concepts_fts MATCH ?1
             ORDER BY bm25(concepts_fts), c.name ASC
             LIMIT ?2",
        )?;
        let concepts = read_concept_rows(&mut stmt, params![match_query, limit.max(1) as i64])?;
        if !concepts.is_empty() {
            return Ok(concepts);
        }
    }
    let pattern = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT id, name, description
         FROM concepts
         WHERE name LIKE ?1 OR IFNULL(description, '') LIKE ?1
         ORDER BY name ASC
         LIMIT ?2",
    )?;
    read_concept_rows(&mut stmt, params![pattern, limit.max(1) as i64])
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

pub(crate) fn export_graph(format: &str, limit: usize) -> Result<GraphExport> {
    let graph = inspect_graph(limit)?;
    let format = format.trim().to_ascii_lowercase();
    let content = match format.as_str() {
        "dot" => render_graph_dot(&graph),
        "ascii" => render_graph_ascii(&graph),
        "json" | "" => serde_json::to_string_pretty(&graph)?,
        other => anyhow::bail!("unsupported graph export format '{other}'"),
    };
    Ok(GraphExport {
        format: if format.is_empty() {
            "json".to_string()
        } else {
            format
        },
        content,
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

pub(crate) fn graph_stats() -> Result<GraphStats> {
    let conn = open_memory_db()?;
    let mut stmt = conn.prepare(
        "SELECT relation, COUNT(*)
         FROM relations
         GROUP BY relation
         ORDER BY COUNT(*) DESC, relation ASC",
    )?;
    let relation_types = stmt
        .query_map([], |row| {
            Ok(GraphRelationTypeStats {
                relation: row.get(0)?,
                relation_count: row.get(1)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let isolated_concept_count = conn.query_row(
        "SELECT COUNT(*)
         FROM concepts c
         WHERE NOT EXISTS (
             SELECT 1
             FROM relations r
             WHERE r.source_concept_id = c.id OR r.target_concept_id = c.id
         )",
        [],
        |row| row.get(0),
    )?;
    Ok(GraphStats {
        concept_count: table_count(&conn, "concepts")?,
        relation_count: table_count(&conn, "relations")?,
        relation_type_count: relation_types.len() as i64,
        isolated_concept_count,
        relation_types,
    })
}

pub(crate) fn learn_project_graph(
    root: &Path,
    name: Option<&str>,
    limit: usize,
) -> Result<ProjectLearnReport> {
    if !root.is_dir() {
        anyhow::bail!("project root not found: {}", root.display());
    }
    let limit = limit.max(1);
    let project_name = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            root.file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "project".to_string());
    let project_description = project_identity(root, &project_name);
    let project = add_concept(&project_name, Some(&project_description))?;
    let mut concepts = vec![project.clone()];
    let mut relations = Vec::new();

    for (name, description) in collect_project_dependencies(root).into_iter().take(limit) {
        let concept = add_concept(&name, Some(&description))?;
        relations.push(link_concepts(&project.name, &concept.name, "depends_on")?);
        concepts.push(concept);
    }
    for (name, description) in collect_project_modules(root).into_iter().take(limit) {
        let concept = add_concept(&name, Some(&description))?;
        relations.push(link_concepts(&concept.name, &project.name, "part_of")?);
        concepts.push(concept);
    }
    for (name, description) in collect_project_entrypoints(root).into_iter().take(limit) {
        let concept = add_concept(&name, Some(&description))?;
        relations.push(link_concepts(&concept.name, &project.name, "part_of")?);
        concepts.push(concept);
    }
    for (name, description) in collect_project_configs(root).into_iter().take(limit) {
        let concept = add_concept(&name, Some(&description))?;
        relations.push(link_concepts(&concept.name, &project.name, "related_to")?);
        concepts.push(concept);
    }

    Ok(ProjectLearnReport {
        project_name,
        project_root: root.display().to_string(),
        total_concepts: concepts.len(),
        link_count: relations.len(),
        concepts,
        relations,
    })
}

fn read_concept_rows<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    params: P,
) -> Result<Vec<GraphConcept>> {
    let rows = stmt.query_map(params, |row| {
        Ok(GraphConcept {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn render_graph_dot(graph: &GraphInspect) -> String {
    let mut out = String::from("digraph packet28_graph {\n");
    for concept in &graph.concepts {
        out.push_str(&format!("  {:?};\n", concept.name));
    }
    for relation in &graph.relations {
        out.push_str(&format!(
            "  {:?} -> {:?} [label={:?}];\n",
            relation.source, relation.target, relation.relation
        ));
    }
    out.push_str("}\n");
    out
}

fn render_graph_ascii(graph: &GraphInspect) -> String {
    let mut out = String::new();
    for concept in &graph.concepts {
        out.push_str(&format!("* {}\n", concept.name));
        if let Some(description) = &concept.description {
            out.push_str(&format!("  {}\n", description));
        }
    }
    for relation in &graph.relations {
        out.push_str(&format!(
            "{} -{}-> {}\n",
            relation.source, relation.relation, relation.target
        ));
    }
    out
}

pub(crate) fn local_store_stats() -> Result<LocalStoreStats> {
    let conn = open_memory_db()?;
    Ok(LocalStoreStats {
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
    let conn = open_memory_db()?;
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
    let conn = open_memory_db()?;
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
    let conn = open_memory_db()?;
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
    let pending = list_pending_extractions(limit)?;
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
    for record in &pending {
        let topic = format!("context-{}", record.project);
        for fact in extract_durable_facts(&record.raw_output) {
            store_memory_with_metadata(MemoryStoreInput {
                content: &fact,
                tags: Some("packet28,extracted"),
                topic: Some(&topic),
                importance: Some("medium"),
                keywords: None,
                project: Some(&record.project),
                source: Some(&format!("pending-extraction:{}", record.tool_name)),
                raw_excerpt: Some(&record.raw_output),
            })?;
        }
    }
    let ids = pending.iter().map(|record| record.id).collect::<Vec<_>>();
    let deleted_count = delete_pending_extractions(&ids)?;
    Ok(PendingExtractionProcessReport {
        pending_count: pending.len(),
        extracted_count: facts.len(),
        deleted_count,
        dry_run,
        facts,
    })
}

pub(crate) fn record_hook_event(input: HookEventInput<'_>) -> Result<HookEventRecord> {
    let conn = open_memory_db()?;
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
    let conn = open_memory_db()?;
    let mut stmt = conn.prepare(
        "SELECT id, runtime, event_kind, session_id, task_id, matcher, payload_json, created_at_unix_ms
         FROM hook_events
         ORDER BY created_at_unix_ms DESC, id DESC
         LIMIT ?1",
    )?;
    read_hook_event_rows(&mut stmt, params![limit.max(1) as i64])
}

pub(crate) fn hook_event_stats() -> Result<Vec<HookEventStats>> {
    let conn = open_memory_db()?;
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
            project: row.get(6)?,
            source: row.get(7)?,
            raw_excerpt: row.get(8)?,
            weight: row.get(9)?,
            recall_score: if row.as_ref().column_count() > 12 {
                row.get(12)?
            } else {
                None
            },
            created_at_unix_ms: row.get(10)?,
            updated_at_unix_ms: row.get(11)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn expanded_filter_limit(limit: usize, has_filters: bool) -> usize {
    if has_filters {
        limit.max(1).saturating_mul(20).min(10_000)
    } else {
        limit.max(1)
    }
}

fn limit_memory_records(mut records: Vec<MemoryRecord>, limit: usize) -> Vec<MemoryRecord> {
    records.truncate(limit.max(1));
    records
}

fn filter_memory_records(
    records: Vec<MemoryRecord>,
    input: MemoryRecallQuery<'_>,
) -> Vec<MemoryRecord> {
    records
        .into_iter()
        .filter(|record| {
            input
                .topic
                .map(|topic| record.topic == normalize_non_empty(Some(topic), "general"))
                .unwrap_or(true)
        })
        .filter(|record| {
            input.project.map_or(true, |project| {
                let wanted = normalize_non_empty(Some(project), "default");
                record.project.as_deref() == Some(wanted.as_str())
            })
        })
        .filter(|record| {
            input
                .tag
                .map(|tag| csv_field_contains(record.tags.as_deref(), tag))
                .unwrap_or(true)
        })
        .filter(|record| {
            input
                .keyword
                .map(|keyword| csv_field_contains(record.keywords.as_deref(), keyword))
                .unwrap_or(true)
        })
        .collect()
}

fn csv_field_contains(field: Option<&str>, needle: &str) -> bool {
    let needle = needle.trim();
    !needle.is_empty()
        && field
            .unwrap_or_default()
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case(needle))
}

fn project_identity(root: &Path, fallback_name: &str) -> String {
    for file in ["Cargo.toml", "package.json", "pyproject.toml", "go.mod"] {
        let path = root.join(file);
        if path.exists() {
            return format!("Project {fallback_name} identified by {file}");
        }
    }
    format!("Project: {fallback_name}")
}

fn collect_project_dependencies(root: &Path) -> Vec<(String, String)> {
    let mut deps = BTreeSet::new();
    collect_manifest_dependencies(&root.join("Cargo.toml"), &mut deps);
    collect_manifest_dependencies(&root.join("package.json"), &mut deps);
    collect_manifest_dependencies(&root.join("pyproject.toml"), &mut deps);
    collect_go_dependencies(&root.join("go.mod"), &mut deps);
    deps.into_iter()
        .map(|dep| (dep.clone(), format!("Dependency: {dep}")))
        .collect()
}

fn collect_manifest_dependencies(path: &Path, deps: &mut BTreeSet<String>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('"') || line.starts_with('\'') || line.starts_with('[') {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            let name = name.trim().trim_matches('"').trim_matches('\'');
            if is_dependency_name(name) {
                deps.insert(name.to_string());
            }
        }
        if line.contains("\":") {
            let name = line
                .split_once(':')
                .map(|(name, _)| name.trim().trim_matches('"'))
                .unwrap_or_default();
            if is_dependency_name(name) {
                deps.insert(name.to_string());
            }
        }
    }
}

fn collect_go_dependencies(path: &Path, deps: &mut BTreeSet<String>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("require ") {
            if let Some(name) = rest.split_whitespace().next() {
                if is_dependency_name(name) {
                    deps.insert(name.to_string());
                }
            }
        }
    }
}

fn is_dependency_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(
            name,
            "package"
                | "dependencies"
                | "devDependencies"
                | "scripts"
                | "workspace"
                | "features"
                | "lib"
                | "bin"
                | "name"
                | "version"
                | "edition"
        )
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '@'))
}

fn collect_project_modules(root: &Path) -> Vec<(String, String)> {
    let mut modules = BTreeSet::new();
    for rel in ["src", "crates", "packages", "apps"] {
        let dir = root.join(rel);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                    modules.insert(format!("{rel}/{name}"));
                }
            }
        }
    }
    modules
        .into_iter()
        .map(|module| (module.clone(), format!("Project module: {module}")))
        .collect()
}

fn collect_project_entrypoints(root: &Path) -> Vec<(String, String)> {
    let mut entrypoints = BTreeSet::new();
    for rel in [
        "src/main.rs",
        "src/lib.rs",
        "src/index.ts",
        "src/index.tsx",
        "src/main.ts",
        "src/main.tsx",
        "main.py",
        "app.py",
    ] {
        if root.join(rel).exists() {
            entrypoints.insert(rel.to_string());
        }
    }
    entrypoints
        .into_iter()
        .map(|entry| (entry.clone(), format!("Project entrypoint: {entry}")))
        .collect()
}

fn collect_project_configs(root: &Path) -> Vec<(String, String)> {
    let mut configs = BTreeSet::new();
    for rel in [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "tsconfig.json",
        ".github/workflows",
        ".mcp.json",
        ".mcp.proxy.json",
    ] {
        if root.join(rel).exists() {
            configs.insert(rel.to_string());
        }
    }
    configs
        .into_iter()
        .map(|config| (config.clone(), format!("Project config: {config}")))
        .collect()
}

fn read_health_rows<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    params: P,
    now: i64,
    day_ms: i64,
    consolidation_threshold: i64,
) -> Result<Vec<MemoryHealthTopic>> {
    let rows = stmt.query_map(params, |row| {
        let memory_count: i64 = row.get(1)?;
        let oldest_updated: i64 = row.get(3)?;
        let newest_updated: i64 = row.get(4)?;
        Ok(MemoryHealthTopic {
            topic: row.get(0)?,
            memory_count,
            stale_count: row.get::<_, Option<i64>>(2)?.unwrap_or_default(),
            oldest_age_days: age_days(now, oldest_updated, day_ms),
            newest_age_days: age_days(now, newest_updated, day_ms),
            consolidation_needed: memory_count >= consolidation_threshold,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn age_days(now: i64, timestamp: i64, day_ms: i64) -> i64 {
    now.saturating_sub(timestamp) / day_ms
}

fn render_consolidated_memory(topic: &str, memories: &[MemoryRecord]) -> String {
    let mut out = format!(
        "Consolidated memory for topic '{}'. Source memories: {}.",
        topic,
        memories.len()
    );
    for memory in memories.iter().rev() {
        out.push_str("\n- ");
        out.push_str(memory.content.trim());
    }
    out
}

fn render_consolidated_raw_excerpt(memories: &[MemoryRecord]) -> Option<String> {
    let excerpts: Vec<&str> = memories
        .iter()
        .filter_map(|memory| memory.raw_excerpt.as_deref())
        .filter(|raw| !raw.trim().is_empty())
        .take(10)
        .collect();
    (!excerpts.is_empty()).then(|| excerpts.join("\n---\n"))
}

fn consolidated_importance(memories: &[MemoryRecord]) -> String {
    let rank = memories
        .iter()
        .map(|memory| importance_rank(&memory.importance))
        .max()
        .unwrap_or(1);
    match rank {
        4 => "critical",
        3 => "high",
        2 => "medium",
        _ => "low",
    }
    .to_string()
}

fn normalize_importance(value: Option<&str>) -> Result<String> {
    let normalized = normalize_non_empty(value, "medium")
        .trim()
        .to_ascii_lowercase();
    match normalized.as_str() {
        "low" | "medium" | "high" | "critical" => Ok(normalized),
        other => anyhow::bail!(
            "unsupported memory importance '{other}' (expected low, medium, high, or critical)"
        ),
    }
}

fn importance_rank(importance: &str) -> i64 {
    match importance.trim().to_ascii_lowercase().as_str() {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 2,
    }
}

fn merge_csv_field<'a>(values: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut merged = Vec::<String>::new();
    for value in values {
        for part in value.split(',') {
            let part = part.trim();
            if !part.is_empty() && !merged.iter().any(|existing| existing == part) {
                merged.push(part.to_string());
            }
        }
    }
    (!merged.is_empty()).then(|| merged.join(","))
}

fn deterministic_embedding(content: &str, dimensions: usize) -> Vec<f64> {
    let dimensions = dimensions.clamp(8, 4096);
    let mut vector = vec![0.0_f64; dimensions];
    for token in content
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let mut hash = 14_695_981_039_346_656_037_u64;
        for byte in token.to_ascii_lowercase().bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(1_099_511_628_211);
        }
        let index = (hash as usize) % dimensions;
        vector[index] += 1.0;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn cosine_similarity(left: &[f64], right: &[f64]) -> f64 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| a * b)
        .sum::<f64>()
}

fn get_memory(conn: &Connection, id: i64) -> Result<MemoryRecord> {
    conn.query_row(
        "SELECT
            id, content, tags, topic, importance, keywords, project, source, raw_excerpt, weight,
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
                project: row.get(6)?,
                source: row.get(7)?,
                raw_excerpt: row.get(8)?,
                weight: row.get(9)?,
                recall_score: None,
                created_at_unix_ms: row.get(10)?,
                updated_at_unix_ms: row.get(11)?,
            })
        },
    )
    .map_err(Into::into)
}

fn get_memory_embedding(
    conn: &Connection,
    memory_id: i64,
    model: &str,
) -> Result<MemoryEmbeddingRecord> {
    conn.query_row(
        "SELECT memory_id, model, dimensions, embedding_json, created_at_unix_ms
         FROM memory_embeddings
         WHERE memory_id = ?1 AND model = ?2",
        params![memory_id, model],
        |row| {
            let dimensions: i64 = row.get(2)?;
            Ok(MemoryEmbeddingRecord {
                memory_id: row.get(0)?,
                model: row.get(1)?,
                dimensions: dimensions.max(0) as usize,
                embedding_json: row.get(3)?,
                created_at_unix_ms: row.get(4)?,
            })
        },
    )
    .map_err(Into::into)
}

fn get_feedback(conn: &Connection, id: i64) -> Result<FeedbackRecord> {
    conn.query_row(
        "SELECT
            id, subject, correction, topic, context, predicted, reason, source,
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
                applied_count: row.get(8)?,
                created_at_unix_ms: row.get(9)?,
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
            m.id, m.session_id, s.session_key, s.agent, m.role, m.content, m.source,
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
                created_at_unix_ms: row.get(7)?,
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
            project TEXT,
            source TEXT,
            raw_excerpt TEXT,
            weight REAL NOT NULL DEFAULT 1.0,
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
            topic TEXT NOT NULL DEFAULT 'general',
            context TEXT,
            predicted TEXT,
            reason TEXT,
            source TEXT,
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
        "updated_at_unix_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    conn.execute(
        "UPDATE memories SET updated_at_unix_ms = created_at_unix_ms WHERE updated_at_unix_ms = 0",
        [],
    )?;
    add_column_if_missing(conn, "feedback", "topic", "TEXT NOT NULL DEFAULT 'general'")?;
    add_column_if_missing(conn, "feedback", "context", "TEXT")?;
    add_column_if_missing(conn, "feedback", "predicted", "TEXT")?;
    add_column_if_missing(conn, "feedback", "reason", "TEXT")?;
    add_column_if_missing(conn, "feedback", "source", "TEXT")?;
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
    conn.execute(
        "INSERT INTO feedback_fts_all(rowid, subject, correction, topic, context, predicted, reason, source)
         SELECT id, subject, correction, topic, context, predicted, reason, source FROM feedback
         WHERE id NOT IN (SELECT rowid FROM feedback_fts_all)",
        [],
    )?;
    conn.execute(
        "INSERT INTO concepts_fts(rowid, name, description)
         SELECT id, name, description FROM concepts
         WHERE id NOT IN (SELECT rowid FROM concepts_fts)",
        [],
    )?;
    conn.execute(
        "INSERT INTO transcript_messages_fts(rowid, role, content, source)
         SELECT id, role, content, source FROM transcript_messages
         WHERE id NOT IN (SELECT rowid FROM transcript_messages_fts)",
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
