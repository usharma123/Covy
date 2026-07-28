use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection};

pub(crate) use crate::memory_db::{
    expanded_filter_limit, fts_match_query, normalize_non_empty, open_memory_db, timestamp_unix_ms,
};
pub(crate) use crate::memory_feedback_transcript::{
    append_transcript_message, apply_feedback, delete_feedback, feedback_stats, list_feedback,
    list_transcript_sessions, record_feedback_with_metadata, search_feedback,
    search_feedback_filtered, search_transcripts, search_transcripts_filtered,
    show_transcript_session, transcript_stats,
};
pub(crate) use crate::memory_graph_store::{
    add_concept_with_metadata, create_graph_memoir, delete_concept, distill_memories_to_graph,
    export_graph, graph_stats, inspect_graph, inspect_graph_concept, learn_project_graph,
    link_concepts, list_graph_memoirs, refine_concept, search_concepts_filtered, show_graph_memoir,
};
pub(crate) use crate::memory_lint::lint_memory_records;
pub(crate) use crate::memory_local_store::{
    delete_pending_extractions, enqueue_pending_extraction, hook_event_stats, list_hook_events,
    list_pending_extractions, local_store_stats, process_pending_extractions, record_hook_event,
};
use crate::memory_scoring::{
    cosine_similarity, deterministic_embedding, importance_rank, initial_memory_weight,
    memory_embedding_document, normalize_importance, score_memory_recall,
};
pub(crate) use crate::memory_store_types::*;

const LOCAL_EMBEDDING_MODEL: &str = "packet28-local-lexical-v2";
const LEGACY_EMBEDDING_MODEL: &str = "packet28-local-hash-v1";

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
    let weight = initial_memory_weight(&importance);
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
            weight,
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
    let now = timestamp_unix_ms();
    let expanded_limit = expanded_filter_limit(input.limit, input.has_filters());
    let mut fts_records = Vec::new();
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
        fts_records = read_memory_rows(&mut stmt, params![match_query, expanded_limit as i64])?;
        fts_records = filter_memory_records(fts_records, input);
    }
    let vector_records = recall_memories_vector(&conn, input, expanded_limit)?;
    let vector_records = filter_memory_records(vector_records, input);
    let hybrid_records =
        merge_hybrid_memory_records(input.query, fts_records, vector_records, input.limit);
    if !hybrid_records.is_empty() {
        mark_memories_accessed(&conn, &hybrid_records, now)?;
        return Ok(hybrid_records);
    }
    let mut records = filter_memory_records(
        recall_memories_like(&conn, input.query, expanded_limit)?,
        input,
    )
    .into_iter()
    .map(|mut record| {
        record.recall_score = Some(score_memory_recall(&record, 0.5, input.query));
        record
    })
    .collect::<Vec<_>>();
    records.sort_by(|a, b| {
        b.recall_score
            .partial_cmp(&a.recall_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.updated_at_unix_ms.cmp(&a.updated_at_unix_ms))
            .then_with(|| a.id.cmp(&b.id))
    });
    let records = limit_memory_records(records, input.limit);
    mark_memories_accessed(&conn, &records, now)?;
    Ok(records)
}

fn mark_memories_accessed(conn: &Connection, records: &[MemoryRecord], now: i64) -> Result<()> {
    for record in records {
        conn.execute(
            "UPDATE memories
             SET access_count = access_count + 1,
                 last_accessed_unix_ms = ?1
             WHERE id = ?2",
            params![now, record.id],
        )?;
    }
    Ok(())
}

fn merge_hybrid_memory_records(
    query: &str,
    fts_records: Vec<MemoryRecord>,
    vector_records: Vec<MemoryRecord>,
    limit: usize,
) -> Vec<MemoryRecord> {
    let mut by_id = HashMap::<i64, (MemoryRecord, f64)>::new();
    for (rank, mut record) in fts_records.into_iter().enumerate() {
        let score = score_memory_recall(&record, 1.0 / (rank as f64 + 1.0), query);
        record.recall_score = Some(score);
        by_id.insert(record.id, (record, score));
    }
    for record in vector_records {
        let score =
            score_memory_recall(&record, record.recall_score.unwrap_or(0.0).max(0.0), query);
        by_id
            .entry(record.id)
            .and_modify(|(existing, existing_score)| {
                *existing_score += score;
                existing.recall_score = Some(*existing_score);
            })
            .or_insert((record, score));
    }
    let mut records = by_id
        .into_values()
        .map(|(mut record, score)| {
            record.recall_score = Some(score);
            record
        })
        .collect::<Vec<_>>();
    records.sort_by(|a, b| {
        b.recall_score
            .partial_cmp(&a.recall_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.updated_at_unix_ms.cmp(&a.updated_at_unix_ms))
            .then_with(|| a.id.cmp(&b.id))
    });
    limit_memory_records(records, limit)
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
         WHERE e.model IN (?1, ?2)",
    )?;
    let rows = stmt.query_map(
        params![LOCAL_EMBEDDING_MODEL, LEGACY_EMBEDDING_MODEL],
        |row| {
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
        },
    )?;
    let mut by_id = HashMap::<i64, MemoryRecord>::new();
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
            by_id
                .entry(record.id)
                .and_modify(|existing| {
                    if score > existing.recall_score.unwrap_or(0.0) {
                        *existing = record.clone();
                    }
                })
                .or_insert(record);
        }
    }
    let mut records = by_id.into_values().collect::<Vec<_>>();
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
    let normalized_importance = normalize_importance(Some(importance))?;
    let min_weight = initial_memory_weight(&normalized_importance);
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
             weight = MAX(weight, ?9),
             updated_at_unix_ms = ?10
         WHERE id = ?11",
        params![
            content,
            tags,
            normalize_non_empty(Some(topic), "general"),
            normalized_importance,
            keywords,
            project,
            source,
            raw_excerpt,
            min_weight,
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
                AVG(weight),
                AVG(CAST(access_count AS REAL)),
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
                AVG(weight),
                AVG(CAST(access_count AS REAL)),
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
         SET weight = weight * MAX(
             0.0,
             1.0 - (
                 ((1.0 - ?1) *
                     CASE LOWER(importance)
                         WHEN 'high' THEN 0.5
                         WHEN 'low' THEN 2.0
                         ELSE 1.0
                     END
                 ) / (1.0 + (MIN(access_count, 5) * 0.1))
             )
         ),
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
         WHERE weight < ?1 AND LOWER(importance) NOT IN ('critical', 'high')
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
    let skipped_protected_count = conn
        .query_row(
            "SELECT COUNT(*) FROM memories
             WHERE weight < ?1 AND LOWER(importance) IN ('critical', 'high')",
            params![threshold],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or_default() as usize;
    Ok(MemoryPruneReport {
        threshold,
        dry_run,
        candidate_count,
        deleted_count,
        skipped_protected_count,
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
    let embedding = deterministic_embedding(&memory_embedding_document(&memory), dimensions);
    let embedding_json = serde_json::to_string(&embedding)?;
    let now = timestamp_unix_ms();
    let model = LOCAL_EMBEDDING_MODEL;
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
        model: LOCAL_EMBEDDING_MODEL.to_string(),
        dimensions,
        embedded_count: embeddings.len(),
        embeddings,
    })
}

pub(crate) fn extract_memory_patterns(
    topic: &str,
    memoir: Option<&str>,
    min_cluster_size: usize,
) -> Result<MemoryPatternReport> {
    let topic = normalize_non_empty(Some(topic), "general");
    let min_cluster_size = min_cluster_size.max(2);
    let memories = list_memories_filtered(MemoryListQuery {
        limit: 10_000,
        topic: Some(&topic),
        project: None,
        all: true,
        sort: "recent",
    })?;
    let mut groups: BTreeMap<String, Vec<MemoryRecord>> = BTreeMap::new();
    for memory in &memories {
        for token in pattern_tokens(memory) {
            groups.entry(token).or_default().push(memory.clone());
        }
    }
    let mut patterns = groups
        .into_iter()
        .filter_map(|(key, mut records)| {
            records.sort_by(|a, b| b.updated_at_unix_ms.cmp(&a.updated_at_unix_ms));
            records.dedup_by_key(|record| record.id);
            if records.len() < min_cluster_size {
                return None;
            }
            let mut related = BTreeSet::new();
            for record in &records {
                for keyword in split_csv_field(record.keywords.as_deref()) {
                    if !keyword.eq_ignore_ascii_case(&key) {
                        related.insert(keyword);
                    }
                }
                for tag in split_csv_field(record.tags.as_deref()) {
                    if !tag.eq_ignore_ascii_case(&key) {
                        related.insert(tag);
                    }
                }
            }
            Some(MemoryPattern {
                key,
                memory_count: records.len(),
                memory_ids: records.iter().map(|record| record.id).collect(),
                keywords: related.into_iter().take(12).collect(),
                sample_contents: records
                    .iter()
                    .take(3)
                    .map(|record| record.content.clone())
                    .collect(),
            })
        })
        .collect::<Vec<_>>();
    patterns.sort_by(|a, b| {
        b.memory_count
            .cmp(&a.memory_count)
            .then_with(|| a.key.cmp(&b.key))
    });

    let mut created_concepts = Vec::new();
    let memoir_name = memoir
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if let Some(memoir_name) = &memoir_name {
        create_graph_memoir(
            Some(memoir_name),
            Some(&format!("Extracted memory patterns for {topic}")),
        )?;
        for pattern in &patterns {
            let description = format!(
                "Recurring memory pattern '{}' found in {} memories for topic '{}'.\n{}",
                pattern.key,
                pattern.memory_count,
                topic,
                pattern.sample_contents.join("\n")
            );
            let mut labels = vec![
                format!("topic:{topic}"),
                "memory-pattern".to_string(),
                format!("pattern:{}", pattern.key),
            ];
            labels.extend(
                pattern
                    .keywords
                    .iter()
                    .take(4)
                    .map(|keyword| format!("tag:{keyword}")),
            );
            labels.sort();
            labels.dedup();
            let source_ids = pattern
                .memory_ids
                .iter()
                .map(|id| format!("memory:{id}"))
                .collect::<Vec<_>>();
            created_concepts.push(add_concept_with_metadata(
                &pattern.key,
                Some(&description),
                Some(memoir_name),
                &labels,
                Some(0.7),
                &source_ids,
            )?);
        }
    }

    Ok(MemoryPatternReport {
        topic,
        min_cluster_size,
        memoir: memoir_name,
        source_memory_count: memories.len(),
        pattern_count: patterns.len(),
        patterns,
        created_concepts,
    })
}

pub(crate) fn lint_memories(root: &Path, limit: usize) -> Result<MemoryLintReport> {
    let memories = list_memories_filtered(MemoryListQuery {
        limit,
        topic: None,
        project: None,
        all: false,
        sort: "recent",
    })?;
    let hook_events = list_hook_events(500)?;
    Ok(lint_memory_records(root, &memories, &hook_events))
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
            input.project.is_none_or(|project| {
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

pub(crate) fn split_csv_field(field: Option<&str>) -> Vec<String> {
    field
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn pattern_tokens(memory: &MemoryRecord) -> BTreeSet<String> {
    let mut tokens = BTreeSet::new();
    for value in split_csv_field(memory.keywords.as_deref())
        .into_iter()
        .chain(split_csv_field(memory.tags.as_deref()))
    {
        let normalized = normalize_pattern_token(&value);
        if is_pattern_token(&normalized) {
            tokens.insert(normalized);
        }
    }
    for raw in memory
        .content
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
    {
        let normalized = normalize_pattern_token(raw);
        if is_pattern_token(&normalized) {
            tokens.insert(normalized);
        }
    }
    tokens
}

fn normalize_pattern_token(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .to_ascii_lowercase()
}

fn is_pattern_token(value: &str) -> bool {
    const STOP_WORDS: &[&str] = &[
        "about", "after", "again", "also", "because", "before", "from", "have", "into", "memory",
        "more", "that", "the", "their", "then", "this", "with", "without",
    ];
    value.len() >= 4
        && value.chars().any(|ch| ch.is_ascii_alphabetic())
        && !STOP_WORDS.contains(&value)
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
        let oldest_updated: i64 = row.get(5)?;
        let newest_updated: i64 = row.get(6)?;
        Ok(MemoryHealthTopic {
            topic: row.get(0)?,
            memory_count,
            avg_weight: row.get::<_, Option<f64>>(2)?.unwrap_or_default(),
            avg_access_count: row.get::<_, Option<f64>>(3)?.unwrap_or_default(),
            stale_count: row.get::<_, Option<i64>>(4)?.unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn memory(id: i64, content: &str) -> MemoryRecord {
        MemoryRecord {
            id,
            content: content.to_string(),
            tags: None,
            topic: "general".to_string(),
            importance: "medium".to_string(),
            keywords: None,
            project: None,
            source: None,
            raw_excerpt: None,
            weight: 1.0,
            recall_score: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    #[test]
    fn memory_lint_flags_stale_runtime_specific_memory_and_preserves_generic_memory() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        std::fs::write(root.path().join("docs/current.md"), "ok").unwrap();
        let memories = vec![
            memory(
                1,
                "Windsurf must use transparent rewrite hooks documented in docs/missing.md",
            ),
            memory(
                2,
                "Project reducers preserve raw artifacts; see docs/current.md for evidence.",
            ),
        ];
        let report = lint_memory_records(root.path(), &memories, &[]);

        assert_eq!(report.memory_count, 2);
        assert!(report.issue_count >= 3);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.memory_id == 1 && issue.kind == "runtime_specific_memory"));
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.memory_id == 1 && issue.kind == "unsupported_runtime_assumption"));
        assert!(report.issues.iter().any(|issue| {
            issue.memory_id == 1 && issue.kind == "stale_path" && issue.detail == "docs/missing.md"
        }));
        assert!(!report.issues.iter().any(|issue| issue.memory_id == 2));
        assert!(serde_json::to_string(&report).unwrap().len() < 768);
    }
}
