use anyhow::Result;

use crate::memory_store_types::MemoryRecord;

pub(crate) fn normalize_importance(value: Option<&str>) -> Result<String> {
    let normalized = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("medium")
        .to_ascii_lowercase();
    match normalized.as_str() {
        "low" | "medium" | "high" | "critical" => Ok(normalized),
        other => anyhow::bail!(
            "unsupported memory importance '{other}' (expected low, medium, high, or critical)"
        ),
    }
}

pub(crate) fn importance_rank(importance: &str) -> i64 {
    match importance.trim().to_ascii_lowercase().as_str() {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 2,
    }
}

pub(crate) fn initial_memory_weight(importance: &str) -> f64 {
    match importance_rank(importance) {
        4 => 1.0,
        3 => 0.9,
        2 => 0.75,
        _ => 0.5,
    }
}

pub(crate) fn score_memory_recall(record: &MemoryRecord, base_score: f64, query: &str) -> f64 {
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

pub(crate) fn memory_embedding_document(memory: &MemoryRecord) -> String {
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

pub(crate) fn deterministic_embedding(content: &str, dimensions: usize) -> Vec<f64> {
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

pub(crate) fn cosine_similarity(left: &[f64], right: &[f64]) -> f64 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| a * b)
        .sum::<f64>()
}
