use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::memory_db::{
    fts_match_query, normalize_non_empty, open_memory_db, table_count, timestamp_unix_ms,
};
use crate::memory_graph_render::{render_graph_ascii, render_graph_dot};
use crate::memory_project_scan::{
    collect_project_configs, collect_project_dependencies, collect_project_entrypoints,
    collect_project_modules, project_identity,
};
use crate::memory_store::{list_memories_filtered, split_csv_field};
use crate::memory_store_types::*;

const DEFAULT_MEMOIR_NAME: &str = "default";

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
