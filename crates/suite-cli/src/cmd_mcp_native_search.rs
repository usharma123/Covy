use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::{Packet28SearchResponseMode, Packet28SearchStrategy};

#[derive(Debug, Clone)]
pub(super) struct Packet28SearchExecution {
    pub(super) strategy: Packet28SearchStrategy,
    pub(super) primary_backend: String,
    pub(super) secondary_backend: Option<String>,
    pub(super) shadowed: bool,
    pub(super) added_displayed_matches: usize,
    pub(super) added_paths: usize,
    pub(super) notes: Vec<String>,
}

const SLIM_PATH_LIMIT: usize = 6;
const SLIM_REGION_LIMIT: usize = 8;
const SLIM_SYMBOL_LIMIT: usize = 4;
const SLIM_DIAGNOSTIC_LIMIT: usize = 4;

fn merge_string_lists<'a>(lists: impl IntoIterator<Item = &'a [String]>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut merged = Vec::new();
    for list in lists {
        for item in list {
            if seen.insert(item.clone()) {
                merged.push(item.clone());
            }
        }
    }
    merged
}

fn query_uses_regex_features(query: &str) -> bool {
    let mut escaped = false;
    for ch in query.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' => {
                return true;
            }
            _ => {}
        }
    }
    false
}

pub(super) fn search_backend_name(result: &packet28_reducer_core::SearchResult) -> String {
    result
        .engine
        .as_ref()
        .map(|engine| engine.engine.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

pub(super) fn should_shadow_with_native(
    request: &packet28_reducer_core::SearchRequest,
    result: &packet28_reducer_core::SearchResult,
    strategy: Packet28SearchStrategy,
) -> bool {
    if search_backend_name(result) != "indexed_regex" {
        return false;
    }
    if matches!(strategy, Packet28SearchStrategy::Recall) {
        return true;
    }
    if result.match_count == 0 || request.context_lines.unwrap_or(0) > 0 {
        return true;
    }
    if request.fixed_string {
        return false;
    }
    request.whole_word
        || matches!(request.case_sensitive, Some(false))
        || query_uses_regex_features(&request.query)
}

fn run_search_preview(result: &packet28_reducer_core::SearchResult) -> String {
    if result.match_count == 0 {
        return "Search found 0 matches.".to_string();
    }
    let mut lines = vec![format!(
        "Search found {} matches in {} files.",
        result.match_count,
        result.groups.len()
    )];
    for group in result.groups.iter().take(12) {
        lines.push(format!("- {} ({})", group.path, group.match_count));
    }
    if result.groups.len() > 12 {
        lines.push(format!("+{} more files", result.groups.len() - 12));
    }
    lines.join("\n")
}

pub(super) fn merge_search_results(
    request: &packet28_reducer_core::SearchRequest,
    mut primary: packet28_reducer_core::SearchResult,
    secondary: &packet28_reducer_core::SearchResult,
) -> (packet28_reducer_core::SearchResult, usize, usize) {
    let mut group_matches =
        BTreeMap::<String, BTreeMap<(usize, String), packet28_reducer_core::SearchMatch>>::new();
    let mut group_counts = BTreeMap::<String, usize>::new();
    let mut primary_displayed = BTreeSet::<(String, usize, String)>::new();

    for group in &primary.groups {
        let entry = group_matches.entry(group.path.clone()).or_default();
        for item in &group.matches {
            primary_displayed.insert((item.path.clone(), item.line, item.text.clone()));
            entry
                .entry((item.line, item.text.clone()))
                .or_insert_with(|| item.clone());
        }
        group_counts.insert(
            group.path.clone(),
            group.match_count.max(group.displayed_match_count),
        );
    }

    let primary_paths = primary.paths.iter().cloned().collect::<BTreeSet<_>>();
    let mut added_displayed_matches = 0usize;
    let mut added_paths = 0usize;
    for group in &secondary.groups {
        if !primary_paths.contains(&group.path) {
            added_paths = added_paths.saturating_add(1);
        }
        let entry = group_matches.entry(group.path.clone()).or_default();
        for item in &group.matches {
            if primary_displayed.insert((item.path.clone(), item.line, item.text.clone())) {
                added_displayed_matches = added_displayed_matches.saturating_add(1);
            }
            entry
                .entry((item.line, item.text.clone()))
                .or_insert_with(|| item.clone());
        }
        let displayed_unique = entry.len();
        let merged_count = group
            .match_count
            .max(group.displayed_match_count)
            .max(displayed_unique);
        group_counts
            .entry(group.path.clone())
            .and_modify(|count| *count = (*count).max(merged_count))
            .or_insert(merged_count);
    }

    let mut groups = Vec::new();
    let mut total_match_count = 0usize;
    let mut returned_matches = Vec::new();
    let max_total_matches = request.max_total_matches.unwrap_or(50).clamp(1, 200);

    for (path, matches) in group_matches {
        let mut displayed = matches.into_values().collect::<Vec<_>>();
        displayed.sort_by(|left, right| {
            left.line
                .cmp(&right.line)
                .then_with(|| left.text.cmp(&right.text))
        });
        if displayed.len() > 12 {
            displayed.truncate(12);
        }
        let match_count = group_counts
            .get(&path)
            .copied()
            .unwrap_or(displayed.len())
            .max(displayed.len());
        total_match_count = total_match_count.saturating_add(match_count);
        for item in displayed.iter().cloned() {
            if returned_matches.len() >= max_total_matches {
                break;
            }
            returned_matches.push(item);
        }
        groups.push(packet28_reducer_core::SearchGroup {
            path,
            match_count,
            displayed_match_count: displayed.len(),
            truncated: match_count > displayed.len(),
            matches: displayed,
        });
    }

    groups.sort_by(|left, right| left.path.cmp(&right.path));
    primary.requested_paths = merge_string_lists([
        primary.requested_paths.as_slice(),
        secondary.requested_paths.as_slice(),
    ]);
    primary.resolved_paths = merge_string_lists([
        primary.resolved_paths.as_slice(),
        secondary.resolved_paths.as_slice(),
    ]);
    primary.paths = groups.iter().map(|group| group.path.clone()).collect();
    primary.regions = groups
        .iter()
        .flat_map(|group| {
            group
                .matches
                .iter()
                .map(|item| packet28_reducer_core::format_region(&item.path, item.line, item.line))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    primary.symbols =
        merge_string_lists([primary.symbols.as_slice(), secondary.symbols.as_slice()]);
    primary.groups = groups;
    primary.match_count = total_match_count
        .max(primary.match_count)
        .max(secondary.match_count);
    primary.returned_match_count = returned_matches.len();
    primary.truncated = primary.match_count > primary.returned_match_count;
    primary.compact_preview = run_search_preview(&primary);
    primary.diagnostics = merge_string_lists([
        primary.diagnostics.as_slice(),
        secondary.diagnostics.as_slice(),
    ]);

    (primary, added_displayed_matches, added_paths)
}

fn build_search_execution_value(execution: &Packet28SearchExecution) -> Value {
    json!({
        "strategy": execution.strategy.as_str(),
        "primary_backend": execution.primary_backend,
        "secondary_backend": execution.secondary_backend,
        "shadowed": execution.shadowed,
        "added_displayed_matches": execution.added_displayed_matches,
        "added_paths": execution.added_paths,
        "notes": execution.notes,
    })
}

fn build_search_slim_engine_value(
    engine: Option<&packet28_reducer_core::SearchEngineStats>,
) -> Option<Value> {
    let engine = engine?;
    let mut value = serde_json::Map::new();
    value.insert("engine".to_string(), json!(engine.engine));
    if let Some(plan_kind) = &engine.plan_kind {
        value.insert("plan_kind".to_string(), json!(plan_kind));
    }
    if let Some(planner_fallback) = &engine.planner_fallback {
        value.insert("planner_fallback".to_string(), json!(planner_fallback));
    }
    if let Some(stale_reason) = &engine.stale_reason {
        value.insert("stale_reason".to_string(), json!(stale_reason));
    }
    if let Some(fallback_reason) = &engine.fallback_reason {
        value.insert("fallback_reason".to_string(), json!(fallback_reason));
    }
    Some(Value::Object(value))
}

fn build_search_slim_execution_value(execution: &Packet28SearchExecution) -> Value {
    let mut value = serde_json::Map::new();
    value.insert(
        "primary_backend".to_string(),
        json!(execution.primary_backend),
    );
    if let Some(secondary_backend) = &execution.secondary_backend {
        value.insert("secondary_backend".to_string(), json!(secondary_backend));
    }
    if execution.shadowed {
        value.insert("shadowed".to_string(), json!(true));
    }
    if execution.added_displayed_matches > 0 {
        value.insert(
            "added_displayed_matches".to_string(),
            json!(execution.added_displayed_matches),
        );
    }
    if execution.added_paths > 0 {
        value.insert("added_paths".to_string(), json!(execution.added_paths));
    }
    let include_notes = execution.shadowed
        || execution.added_displayed_matches > 0
        || execution.added_paths > 0
        || execution.notes.iter().any(|note| note.contains("failed"));
    if include_notes && !execution.notes.is_empty() {
        value.insert("notes".to_string(), json!(execution.notes));
    }
    Value::Object(value)
}

fn build_search_slim_preview(search_result: &packet28_reducer_core::SearchResult) -> String {
    search_result
        .compact_preview
        .lines()
        .next()
        .unwrap_or("Search completed")
        .to_string()
}

pub(super) fn build_search_slim_payload(
    search_result: &packet28_reducer_core::SearchResult,
    artifact_id: Option<String>,
    execution: &Packet28SearchExecution,
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("match_count".to_string(), json!(search_result.match_count));
    if search_result.returned_match_count != search_result.match_count {
        payload.insert(
            "returned_match_count".to_string(),
            json!(search_result.returned_match_count),
        );
    }
    if search_result.truncated {
        payload.insert("truncated".to_string(), json!(true));
    }
    let paths = search_result
        .paths
        .iter()
        .take(SLIM_PATH_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    if !paths.is_empty() {
        payload.insert("paths".to_string(), json!(paths));
    }
    let regions = search_result
        .regions
        .iter()
        .take(SLIM_REGION_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    if !regions.is_empty() {
        payload.insert("regions".to_string(), json!(regions));
    }
    let symbols = search_result
        .symbols
        .iter()
        .take(SLIM_SYMBOL_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    if !symbols.is_empty() {
        payload.insert("symbols".to_string(), json!(symbols));
    }
    let diagnostics = search_result
        .diagnostics
        .iter()
        .take(SLIM_DIAGNOSTIC_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    if !diagnostics.is_empty() {
        payload.insert("diagnostics".to_string(), json!(diagnostics));
    }
    payload.insert(
        "compact_preview".to_string(),
        json!(build_search_slim_preview(search_result)),
    );
    if let Some(engine) = build_search_slim_engine_value(search_result.engine.as_ref()) {
        payload.insert("engine".to_string(), engine);
    }
    payload.insert(
        "search_strategy".to_string(),
        json!(execution.strategy.as_str()),
    );
    payload.insert(
        "hybrid".to_string(),
        build_search_slim_execution_value(execution),
    );
    if let Some(artifact_id) = artifact_id {
        payload.insert("artifact_id".to_string(), json!(artifact_id));
    }
    payload.insert("response_mode".to_string(), json!("slim"));
    Value::Object(payload)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_search_request(
    query: &str,
    paths: Vec<String>,
    fixed_string: bool,
    case_sensitive: Option<bool>,
    whole_word: bool,
    context_lines: Option<usize>,
    max_matches_per_file: Option<usize>,
    max_total_matches: Option<usize>,
) -> packet28_reducer_core::SearchRequest {
    packet28_reducer_core::SearchRequest {
        query: query.to_string(),
        requested_paths: paths,
        fixed_string,
        case_sensitive,
        whole_word,
        context_lines,
        max_matches_per_file,
        max_total_matches,
    }
}

pub(super) fn build_search_full_payload(
    search_result: &packet28_reducer_core::SearchResult,
    execution: &Packet28SearchExecution,
) -> Value {
    let groups = search_result
        .groups
        .iter()
        .map(|group| {
            json!({
                "path": group.path,
                "match_count": group.match_count,
                "displayed_match_count": group.displayed_match_count,
                "truncated": group.truncated,
                "matches": group.matches,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "query": search_result.query,
        "match_count": search_result.match_count,
        "returned_match_count": search_result.returned_match_count,
        "truncated": search_result.truncated,
        "requested_paths": search_result.requested_paths,
        "resolved_paths": search_result.resolved_paths,
        "paths": search_result.paths,
        "regions": search_result.regions,
        "symbols": search_result.symbols,
        "groups": groups,
        "compact_preview": search_result.compact_preview,
        "diagnostics": search_result.diagnostics,
        "engine": search_result.engine,
        "search_strategy": execution.strategy.as_str(),
        "hybrid": build_search_execution_value(execution),
        "response_mode": "full",
    })
}

pub(super) fn build_search_response_payload(
    search_result: &packet28_reducer_core::SearchResult,
    execution: &Packet28SearchExecution,
    response_mode: &Packet28SearchResponseMode,
    artifact_id: Option<String>,
) -> Value {
    let full_payload = build_search_full_payload(search_result, execution);
    match response_mode {
        Packet28SearchResponseMode::Full => {
            let mut payload = full_payload;
            if let Some(artifact_id) = artifact_id {
                payload["artifact_id"] = json!(artifact_id);
            }
            payload
        }
        Packet28SearchResponseMode::Slim => {
            build_search_slim_payload(search_result, artifact_id, execution)
        }
    }
}
