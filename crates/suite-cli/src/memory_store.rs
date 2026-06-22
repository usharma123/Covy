use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub(crate) use crate::memory_lint::lint_memory_records;
pub(crate) use crate::memory_store_types::*;

const DEFAULT_MEMOIR_NAME: &str = "default";
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

pub(crate) fn create_graph_memoir(
    name: Option<&str>,
    description: Option<&str>,
) -> Result<GraphMemoir> {
    let conn = open_memory_db()?;
    let name = normalize_non_empty(name, DEFAULT_MEMOIR_NAME);
    upsert_memoir(&conn, &name, description)?;
    show_graph_memoir_summary(&conn, &name)
}

pub(crate) fn list_graph_memoirs() -> Result<Vec<GraphMemoir>> {
    let conn = open_memory_db()?;
    let mut stmt = conn.prepare(
        "SELECT m.name
         FROM memoirs m
         ORDER BY m.updated_at_unix_ms DESC, m.name ASC",
    )?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    names
        .into_iter()
        .map(|name| show_graph_memoir_summary(&conn, &name))
        .collect()
}

pub(crate) fn show_graph_memoir(name: Option<&str>, limit: usize) -> Result<GraphMemoirShow> {
    let conn = open_memory_db()?;
    let name = normalize_non_empty(name, DEFAULT_MEMOIR_NAME);
    upsert_memoir(&conn, &name, None)?;
    let memoir = show_graph_memoir_summary(&conn, &name)?;
    let concepts = read_concepts_for_memoir(&conn, &name, limit.max(1))?;
    let relations = read_relations_for_memoir(&conn, &name, limit.max(1))?;
    Ok(GraphMemoirShow {
        memoir,
        concepts,
        relations,
    })
}

pub(crate) fn add_concept(name: &str, description: Option<&str>) -> Result<GraphConcept> {
    add_concept_with_metadata(name, description, None, &[], None, &[])
}

pub(crate) fn add_concept_with_metadata(
    name: &str,
    description: Option<&str>,
    memoir: Option<&str>,
    labels: &[String],
    confidence: Option<f64>,
    source_ids: &[String],
) -> Result<GraphConcept> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    let memoir_name = normalize_non_empty(memoir, DEFAULT_MEMOIR_NAME);
    upsert_memoir(&conn, &memoir_name, None)?;
    let labels_json = serde_json::to_string(labels)?;
    let source_ids_json = serde_json::to_string(source_ids)?;
    let has_confidence = confidence.is_some();
    let confidence = confidence.unwrap_or(0.5).clamp(0.0, 1.0);
    conn.execute(
        "INSERT INTO concepts (
            name, description, created_at_unix_ms, memoir_name, labels, confidence,
            revision, source_ids, updated_at_unix_ms
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?3)
         ON CONFLICT(name) DO UPDATE SET
            description=COALESCE(excluded.description, concepts.description),
            memoir_name=CASE WHEN ?8 THEN excluded.memoir_name ELSE concepts.memoir_name END,
            labels=CASE WHEN ?9 THEN excluded.labels ELSE concepts.labels END,
            confidence=CASE WHEN ?10 THEN excluded.confidence ELSE concepts.confidence END,
            source_ids=CASE WHEN ?11 THEN excluded.source_ids ELSE concepts.source_ids END,
            updated_at_unix_ms=excluded.updated_at_unix_ms",
        params![
            name,
            description,
            now,
            memoir_name,
            labels_json,
            confidence,
            source_ids_json,
            memoir.is_some(),
            !labels.is_empty(),
            has_confidence,
            !source_ids.is_empty(),
        ],
    )?;
    read_concept_by_name(&conn, name)
}

pub(crate) fn refine_concept(name: &str, description: &str) -> Result<GraphConcept> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    conn.execute(
        "INSERT INTO concepts (
            name, description, created_at_unix_ms, memoir_name, labels, confidence,
            revision, source_ids, updated_at_unix_ms
         )
         VALUES (?1, ?2, ?3, ?4, '[]', 0.5, 1, '[]', ?3)
         ON CONFLICT(name) DO UPDATE SET
            description=excluded.description,
            revision=concepts.revision + 1,
            updated_at_unix_ms=excluded.updated_at_unix_ms",
        params![name, description, now, DEFAULT_MEMOIR_NAME],
    )?;
    upsert_memoir(&conn, DEFAULT_MEMOIR_NAME, None)?;
    read_concept_by_name(&conn, name)
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

pub(crate) fn search_concepts_filtered(
    query: &str,
    memoir: Option<&str>,
    label: Option<&str>,
    limit: usize,
) -> Result<Vec<GraphConcept>> {
    let conn = open_memory_db()?;
    if let Some(match_query) = fts_match_query(query) {
        let mut stmt = conn.prepare(
            "SELECT c.id, c.name, c.description, c.memoir_name, c.labels, c.confidence,
                    c.revision, c.source_ids, c.created_at_unix_ms, c.updated_at_unix_ms
             FROM concepts_fts f
             JOIN concepts c ON c.rowid = f.rowid
             WHERE concepts_fts MATCH ?1
             ORDER BY bm25(concepts_fts), c.name ASC
             LIMIT ?2",
        )?;
        let concepts = read_concept_rows(&mut stmt, params![match_query, limit.max(1) as i64])?;
        let concepts = filter_graph_concepts(concepts, memoir, label);
        if !concepts.is_empty() {
            return Ok(concepts);
        }
    }
    let pattern = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT id, name, description, memoir_name, labels, confidence, revision,
                source_ids, created_at_unix_ms, updated_at_unix_ms
         FROM concepts
         WHERE name LIKE ?1 OR IFNULL(description, '') LIKE ?1 OR labels LIKE ?1
         ORDER BY name ASC
         LIMIT ?2",
    )?;
    let concepts = read_concept_rows(&mut stmt, params![pattern, limit.max(1) as i64])?;
    Ok(filter_graph_concepts(concepts, memoir, label))
}

pub(crate) fn link_concepts(source: &str, target: &str, relation: &str) -> Result<GraphRelation> {
    let source = add_concept(source, None)?;
    let target = add_concept(target, None)?;
    link_existing_concepts(&source, &target, relation)
}

fn link_existing_concepts(
    source: &GraphConcept,
    target: &GraphConcept,
    relation: &str,
) -> Result<GraphRelation> {
    let conn = open_memory_db()?;
    let now = timestamp_unix_ms();
    conn.execute(
        "INSERT INTO relations (source_concept_id, target_concept_id, relation, created_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![source.id, target.id, relation, now],
    )?;
    Ok(GraphRelation {
        id: conn.last_insert_rowid(),
        source: source.name.clone(),
        target: target.name.clone(),
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
    let concepts = read_concepts_for_memoir(&conn, "", limit.max(1))?;
    let relations = read_relations_for_memoir(&conn, "", limit.max(1))?;
    Ok(GraphInspect {
        concepts,
        relations,
    })
}

pub(crate) fn inspect_graph_concept(
    name: &str,
    memoir: Option<&str>,
    depth: usize,
) -> Result<GraphConceptInspect> {
    let conn = open_memory_db()?;
    let concept = read_concept_by_name(&conn, name)?;
    if let Some(memoir) = memoir.map(str::trim).filter(|value| !value.is_empty()) {
        if concept.memoir_name != memoir {
            anyhow::bail!("concept '{name}' is not in memoir '{memoir}'");
        }
    }
    let depth = depth.max(1);
    let all_relations = read_relations_for_memoir(&conn, memoir.unwrap_or(""), 10_000)?;
    let all_concepts = read_concepts_for_memoir(&conn, memoir.unwrap_or(""), 10_000)?;
    let mut seen_names = BTreeSet::from([concept.name.clone()]);
    let mut frontier = BTreeSet::from([concept.name.clone()]);
    let mut relations = Vec::new();
    for _ in 0..depth {
        let mut next_frontier = BTreeSet::new();
        for relation in &all_relations {
            let touches_source = frontier.contains(&relation.source);
            let touches_target = frontier.contains(&relation.target);
            if !touches_source && !touches_target {
                continue;
            }
            if !relations
                .iter()
                .any(|existing: &GraphRelation| existing.id == relation.id)
            {
                relations.push(relation.clone());
            }
            for name in [&relation.source, &relation.target] {
                if seen_names.insert(name.clone()) {
                    next_frontier.insert(name.clone());
                }
            }
        }
        if next_frontier.is_empty() {
            break;
        }
        frontier = next_frontier;
    }
    let neighbors = all_concepts
        .into_iter()
        .filter(|candidate| candidate.name != concept.name && seen_names.contains(&candidate.name))
        .collect();
    Ok(GraphConceptInspect {
        concept,
        depth,
        neighbors,
        relations,
    })
}

pub(crate) fn distill_memories_to_graph(
    topic: &str,
    memoir: Option<&str>,
    limit: usize,
) -> Result<GraphDistillReport> {
    let topic = normalize_non_empty(Some(topic), "general");
    let memoir = normalize_non_empty(memoir, DEFAULT_MEMOIR_NAME);
    create_graph_memoir(
        Some(&memoir),
        Some(&format!("Distilled memories for {topic}")),
    )?;
    let memories = list_memories_filtered(MemoryListQuery {
        limit: limit.max(1),
        topic: Some(&topic),
        project: None,
        all: true,
        sort: "recent",
    })?;
    if memories.is_empty() {
        anyhow::bail!("no memories found in topic: {topic}");
    }
    let mut created_count = 0usize;
    let mut refined_count = 0usize;
    let mut concepts = Vec::new();
    for memory in &memories {
        let keywords = split_csv_field(memory.keywords.as_deref());
        let concept_name = keywords
            .first()
            .cloned()
            .unwrap_or_else(|| format!("{}-{}", topic, memory.id));
        let existing = read_concept_by_name_optional(&open_memory_db()?, &concept_name)?;
        let source_id = format!("memory:{}", memory.id);
        let mut labels = vec![format!("topic:{topic}")];
        labels.extend(keywords.iter().map(|keyword| format!("tag:{keyword}")));
        labels.sort();
        labels.dedup();
        let description = match existing
            .as_ref()
            .and_then(|concept| concept.description.as_ref())
        {
            Some(existing_description) if !existing_description.contains(&memory.content) => {
                format!("{existing_description}\n---\n{}", memory.content)
            }
            Some(existing_description) => existing_description.clone(),
            None => memory.content.clone(),
        };
        let mut concept = add_concept_with_metadata(
            &concept_name,
            Some(&description),
            Some(&memoir),
            &labels,
            Some(memory.weight.clamp(0.0, 1.0)),
            &[source_id],
        )?;
        if existing.is_some() {
            concept = refine_concept(&concept_name, &description)?;
            refined_count += 1;
        } else {
            created_count += 1;
        }
        concepts.push(concept.clone());
        for related_name in keywords.iter().skip(1) {
            if related_name == &concept_name {
                continue;
            }
            let existing_related = read_concept_by_name_optional(&open_memory_db()?, related_name)?;
            let related_description = format!(
                "Related distilled keyword from memory {} in topic {topic}.",
                memory.id
            );
            let mut related_labels = vec![
                format!("topic:{topic}"),
                "distilled-keyword".to_string(),
                format!("tag:{related_name}"),
            ];
            related_labels.sort();
            related_labels.dedup();
            let related = add_concept_with_metadata(
                related_name,
                Some(&related_description),
                Some(&memoir),
                &related_labels,
                Some((memory.weight * 0.8).clamp(0.0, 1.0)),
                &[format!("memory:{}", memory.id)],
            )?;
            if existing_related.is_some() {
                refined_count += 1;
            } else {
                created_count += 1;
            }
            let _ = link_existing_concepts(&concept, &related, "mentions")?;
            concepts.push(related);
        }
    }
    Ok(GraphDistillReport {
        topic,
        memoir,
        source_memory_count: memories.len(),
        created_count,
        refined_count,
        concepts,
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
    memoir: Option<&str>,
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
    let memoir_name = normalize_non_empty(memoir, DEFAULT_MEMOIR_NAME);
    let project_description = project_identity(root, &project_name);
    create_graph_memoir(
        Some(&memoir_name),
        Some(&format!("Learned project graph for {project_name}")),
    )?;
    let project = add_concept_with_metadata(
        &project_name,
        Some(&project_description),
        Some(&memoir_name),
        &[],
        None,
        &[],
    )?;
    let mut concepts = vec![project.clone()];
    let mut relations = Vec::new();

    for (name, description) in collect_project_dependencies(root).into_iter().take(limit) {
        let concept = add_concept_with_metadata(
            &name,
            Some(&description),
            Some(&memoir_name),
            &[],
            None,
            &[],
        )?;
        relations.push(link_concepts(&project.name, &concept.name, "depends_on")?);
        concepts.push(concept);
    }
    for (name, description) in collect_project_modules(root).into_iter().take(limit) {
        let concept = add_concept_with_metadata(
            &name,
            Some(&description),
            Some(&memoir_name),
            &[],
            None,
            &[],
        )?;
        relations.push(link_concepts(&concept.name, &project.name, "part_of")?);
        concepts.push(concept);
    }
    for (name, description) in collect_project_entrypoints(root).into_iter().take(limit) {
        let concept = add_concept_with_metadata(
            &name,
            Some(&description),
            Some(&memoir_name),
            &[],
            None,
            &[],
        )?;
        relations.push(link_concepts(&concept.name, &project.name, "part_of")?);
        concepts.push(concept);
    }
    for (name, description) in collect_project_configs(root).into_iter().take(limit) {
        let concept = add_concept_with_metadata(
            &name,
            Some(&description),
            Some(&memoir_name),
            &[],
            None,
            &[],
        )?;
        relations.push(link_concepts(&concept.name, &project.name, "related_to")?);
        concepts.push(concept);
    }

    Ok(ProjectLearnReport {
        project_name,
        project_root: root.display().to_string(),
        memoir_name,
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
            memoir_name: row.get(3)?,
            labels: parse_json_string_array(row.get::<_, Option<String>>(4)?.as_deref()),
            confidence: row.get(5)?,
            revision: row.get(6)?,
            source_ids: parse_json_string_array(row.get::<_, Option<String>>(7)?.as_deref()),
            created_at_unix_ms: row.get(8)?,
            updated_at_unix_ms: row.get(9)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn read_concept_by_name(conn: &Connection, name: &str) -> Result<GraphConcept> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, memoir_name, labels, confidence, revision,
                source_ids, created_at_unix_ms, updated_at_unix_ms
         FROM concepts
         WHERE name = ?1",
    )?;
    let mut concepts = read_concept_rows(&mut stmt, params![name])?;
    concepts
        .pop()
        .ok_or_else(|| anyhow::anyhow!("concept not found after insert: {name}"))
}

fn read_concept_by_name_optional(conn: &Connection, name: &str) -> Result<Option<GraphConcept>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, memoir_name, labels, confidence, revision,
                source_ids, created_at_unix_ms, updated_at_unix_ms
         FROM concepts
         WHERE name = ?1",
    )?;
    let mut concepts = read_concept_rows(&mut stmt, params![name])?;
    Ok(concepts.pop())
}

fn filter_graph_concepts(
    concepts: Vec<GraphConcept>,
    memoir: Option<&str>,
    label: Option<&str>,
) -> Vec<GraphConcept> {
    let memoir = memoir.map(str::trim).filter(|value| !value.is_empty());
    let label = label.map(str::trim).filter(|value| !value.is_empty());
    concepts
        .into_iter()
        .filter(|concept| {
            memoir
                .map(|memoir| concept.memoir_name == memoir)
                .unwrap_or(true)
        })
        .filter(|concept| {
            label
                .map(|label| concept.labels.iter().any(|value| value == label))
                .unwrap_or(true)
        })
        .collect()
}

fn read_concepts_for_memoir(
    conn: &Connection,
    memoir_name: &str,
    limit: usize,
) -> Result<Vec<GraphConcept>> {
    let mut sql = String::from(
        "SELECT id, name, description, memoir_name, labels, confidence, revision,
                source_ids, created_at_unix_ms, updated_at_unix_ms
         FROM concepts",
    );
    if !memoir_name.is_empty() {
        sql.push_str(" WHERE memoir_name = ?1");
    }
    sql.push_str(" ORDER BY name ASC LIMIT ?");
    let mut stmt = conn.prepare(&sql)?;
    if memoir_name.is_empty() {
        read_concept_rows(&mut stmt, params![limit as i64])
    } else {
        read_concept_rows(&mut stmt, params![memoir_name, limit as i64])
    }
}

fn read_relations_for_memoir(
    conn: &Connection,
    memoir_name: &str,
    limit: usize,
) -> Result<Vec<GraphRelation>> {
    let mut sql = String::from(
        "SELECT r.id, s.name, t.name, r.relation
         FROM relations r
         JOIN concepts s ON s.id = r.source_concept_id
         JOIN concepts t ON t.id = r.target_concept_id",
    );
    if !memoir_name.is_empty() {
        sql.push_str(" WHERE s.memoir_name = ?1 OR t.memoir_name = ?1");
    }
    sql.push_str(" ORDER BY r.id DESC LIMIT ?");
    let mut stmt = conn.prepare(&sql)?;
    if memoir_name.is_empty() {
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(GraphRelation {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                relation: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    } else {
        let rows = stmt.query_map(params![memoir_name, limit as i64], |row| {
            Ok(GraphRelation {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                relation: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

fn upsert_memoir(conn: &Connection, name: &str, description: Option<&str>) -> Result<()> {
    let now = timestamp_unix_ms();
    conn.execute(
        "INSERT INTO memoirs (name, description, created_at_unix_ms, updated_at_unix_ms)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(name) DO UPDATE SET
            description=COALESCE(excluded.description, memoirs.description),
            updated_at_unix_ms=excluded.updated_at_unix_ms",
        params![name, description, now],
    )?;
    Ok(())
}

fn show_graph_memoir_summary(conn: &Connection, name: &str) -> Result<GraphMemoir> {
    let mut stmt = conn.prepare(
        "SELECT
            m.name,
            m.description,
            COUNT(c.id),
            COUNT(DISTINCT r.id),
            COALESCE(AVG(c.confidence), 0.0),
            m.created_at_unix_ms,
            COALESCE(MAX(c.updated_at_unix_ms), m.updated_at_unix_ms)
         FROM memoirs m
         LEFT JOIN concepts c ON c.memoir_name = m.name
         LEFT JOIN relations r ON r.source_concept_id = c.id OR r.target_concept_id = c.id
         WHERE m.name = ?1
         GROUP BY m.name, m.description, m.created_at_unix_ms, m.updated_at_unix_ms",
    )?;
    stmt.query_row(params![name], |row| {
        Ok(GraphMemoir {
            name: row.get(0)?,
            description: row.get(1)?,
            concept_count: row.get(2)?,
            relation_count: row.get(3)?,
            average_confidence: row.get(4)?,
            created_at_unix_ms: row.get(5)?,
            updated_at_unix_ms: row.get(6)?,
        })
    })
    .map_err(Into::into)
}

fn parse_json_string_array(value: Option<&str>) -> Vec<String> {
    value
        .and_then(|value| serde_json::from_str::<Vec<String>>(value).ok())
        .unwrap_or_default()
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

fn split_csv_field(field: Option<&str>) -> Vec<String> {
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

fn initial_memory_weight(importance: &str) -> f64 {
    match importance_rank(importance) {
        4 => 1.0,
        3 => 0.9,
        2 => 0.75,
        _ => 0.5,
    }
}

fn score_memory_recall(record: &MemoryRecord, base_score: f64, query: &str) -> f64 {
    let importance_multiplier = match importance_rank(&record.importance) {
        4 => 1.35,
        3 => 1.2,
        2 => 1.0,
        _ => 0.85,
    };
    let weight_multiplier = record.weight.clamp(0.5, 2.0);
    (base_score * importance_multiplier * weight_multiplier)
        + content_match_bonus(record, query).min(0.75)
        + metadata_match_bonus(record, query).min(0.5)
}

fn content_match_bonus(record: &MemoryRecord, query: &str) -> f64 {
    let terms = query_terms(query);
    if terms.is_empty() {
        return 0.0;
    }
    let query_phrase = terms.join(" ");
    let content = record.content.to_ascii_lowercase();
    let raw_excerpt = record
        .raw_excerpt
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let content_terms = terms
        .iter()
        .filter(|term| content.contains(term.as_str()))
        .count() as f64
        * 0.06;
    let raw_terms = terms
        .iter()
        .filter(|term| raw_excerpt.contains(term.as_str()))
        .count() as f64
        * 0.03;
    let phrase_bonus = if content.contains(&query_phrase) || raw_excerpt.contains(&query_phrase) {
        0.25
    } else {
        0.0
    };
    content_terms + raw_terms + phrase_bonus
}

fn metadata_match_bonus(record: &MemoryRecord, query: &str) -> f64 {
    let terms = query_terms(query);
    if terms.is_empty() {
        return 0.0;
    }
    let keyword_bonus = field_term_bonus(record.keywords.as_deref(), &terms, 0.18);
    let tag_bonus = field_term_bonus(record.tags.as_deref(), &terms, 0.12);
    let topic_bonus = field_term_bonus(Some(&record.topic), &terms, 0.08);
    let project_bonus = field_term_bonus(record.project.as_deref(), &terms, 0.04);
    keyword_bonus + tag_bonus + topic_bonus + project_bonus
}

fn field_term_bonus(field: Option<&str>, terms: &[String], per_match: f64) -> f64 {
    let Some(field) = field else {
        return 0.0;
    };
    let field = field.to_ascii_lowercase();
    let matches = terms
        .iter()
        .filter(|term| field.contains(term.as_str()))
        .count();
    matches as f64 * per_match
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .filter_map(|term| {
            let term = term.trim_matches('-').trim().to_ascii_lowercase();
            (term.len() >= 2).then_some(term)
        })
        .take(8)
        .collect()
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

fn memory_embedding_document(memory: &MemoryRecord) -> String {
    [
        Some(memory.content.as_str()),
        Some(memory.topic.as_str()),
        memory.tags.as_deref(),
        memory.keywords.as_deref(),
        memory.project.as_deref(),
        memory.source.as_deref(),
        memory.raw_excerpt.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n")
}

fn deterministic_embedding(content: &str, dimensions: usize) -> Vec<f64> {
    let dimensions = dimensions.clamp(8, 4096);
    let mut vector = vec![0.0_f64; dimensions];
    for token in content
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let normalized = token.to_ascii_lowercase();
        add_embedding_feature(&mut vector, &normalized, 2.0);
        for part in normalized.split(['_', '-']).filter(|part| part.len() >= 2) {
            add_embedding_feature(&mut vector, part, 1.5);
        }
        add_character_ngrams(&mut vector, &normalized);
    }
    let norm = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn add_character_ngrams(vector: &mut [f64], token: &str) {
    let chars = token.chars().collect::<Vec<_>>();
    if chars.len() < 3 {
        return;
    }
    for n in 3..=5 {
        if chars.len() < n {
            continue;
        }
        for window in chars.windows(n) {
            let gram = window.iter().collect::<String>();
            add_embedding_feature(vector, &format!("char:{gram}"), 0.75);
        }
    }
}

fn add_embedding_feature(vector: &mut [f64], feature: &str, weight: f64) {
    let mut hash = 14_695_981_039_346_656_037_u64;
    for byte in feature.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    let index = (hash as usize) % vector.len();
    vector[index] += weight;
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

fn rebuild_fts_table(conn: &Connection, table: &str) -> Result<()> {
    conn.execute(
        &format!("INSERT INTO {table}({table}) VALUES('rebuild')"),
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
