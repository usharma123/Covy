use super::*;
use crate::cmd_mcp::support::{
    load_raw_output_artifact, load_tool_result_artifact, next_task_invocation,
    packet28_search_via_session, packet28_search_via_session_with_force, store_result_artifact,
    write_auto_capture_state_batch_via_session,
};
use glob::Pattern;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Packet28SearchResponseMode {
    #[default]
    Slim,
    Full,
}

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Packet28SearchStrategy {
    Indexed,
    Native,
    Recall,
    #[default]
    Hybrid,
}

impl Packet28SearchStrategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Hybrid => "hybrid",
            Self::Recall => "recall",
            Self::Indexed => "indexed",
            Self::Native => "native",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28SearchArgs {
    pub(crate) task_id: String,
    pub(crate) query: String,
    pub(crate) paths: Vec<String>,
    pub(crate) fixed_string: bool,
    pub(crate) case_sensitive: Option<bool>,
    pub(crate) whole_word: bool,
    pub(crate) context_lines: Option<usize>,
    pub(crate) max_matches_per_file: Option<usize>,
    pub(crate) max_total_matches: Option<usize>,
    pub(crate) search_strategy: Packet28SearchStrategy,
    pub(crate) response_mode: Packet28SearchResponseMode,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28SearchFastArgs {
    pub(crate) query: String,
    pub(crate) paths: Vec<String>,
    pub(crate) fixed_string: bool,
    pub(crate) case_sensitive: Option<bool>,
    pub(crate) whole_word: bool,
    pub(crate) context_lines: Option<usize>,
    pub(crate) max_matches_per_file: Option<usize>,
    pub(crate) max_total_matches: Option<usize>,
    pub(crate) search_strategy: Packet28SearchStrategy,
    pub(crate) response_mode: Packet28SearchResponseMode,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28ReadRegionsArgs {
    pub(crate) task_id: String,
    pub(crate) path: String,
    pub(crate) regions: Vec<String>,
    pub(crate) line_start: Option<usize>,
    pub(crate) line_end: Option<usize>,
    pub(crate) response_mode: Packet28SearchResponseMode,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28GlobArgs {
    pub(crate) task_id: String,
    pub(crate) pattern: String,
    pub(crate) paths: Vec<String>,
    pub(crate) max_results: Option<usize>,
    pub(crate) response_mode: Packet28SearchResponseMode,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28FetchToolResultArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) invocation_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28FetchRawOutputArgs {
    pub(crate) task_id: String,
    pub(crate) handle: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28FetchContextArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
    pub(crate) response_mode: Option<BrokerResponseMode>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28PrepareHandoffArgs {
    pub(crate) task_id: String,
    pub(crate) query: Option<String>,
    pub(crate) response_mode: Option<BrokerResponseMode>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28WriteIntentionArgs {
    pub(crate) task_id: String,
    pub(crate) text: String,
    pub(crate) note: Option<String>,
    pub(crate) step_id: Option<String>,
    pub(crate) question_id: Option<String>,
    pub(crate) paths: Vec<String>,
    pub(crate) symbols: Vec<String>,
}

#[derive(Debug, Clone)]
struct Packet28SearchExecution {
    strategy: Packet28SearchStrategy,
    primary_backend: String,
    secondary_backend: Option<String>,
    shadowed: bool,
    added_displayed_matches: usize,
    added_paths: usize,
    notes: Vec<String>,
}

const SLIM_PATH_LIMIT: usize = 6;
const SLIM_REGION_LIMIT: usize = 8;
const SLIM_SYMBOL_LIMIT: usize = 4;
const SLIM_DIAGNOSTIC_LIMIT: usize = 4;

fn json_array_strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(ToOwned::to_owned))
        .collect()
}

fn search_request_summary(args: &Packet28SearchArgs) -> String {
    search_request_summary_parts(
        &args.query,
        &args.paths,
        &args.response_mode,
        args.search_strategy,
    )
}

fn search_request_summary_parts(
    query: &str,
    paths: &[String],
    response_mode: &Packet28SearchResponseMode,
    strategy: Packet28SearchStrategy,
) -> String {
    let scope = if paths.is_empty() {
        format!("search '{}' across repo ({:?})", query, response_mode)
    } else {
        format!(
            "search '{}' in {} path(s) ({:?})",
            query,
            paths.len(),
            response_mode
        )
    };
    if matches!(strategy, Packet28SearchStrategy::Hybrid) {
        scope
    } else {
        format!("{scope} via {}", strategy.as_str())
    }
}

fn read_regions_request_summary(args: &Packet28ReadRegionsArgs, path: &str) -> String {
    if !args.regions.is_empty() {
        format!(
            "read_regions {path} using {} region hint(s)",
            args.regions.len()
        )
    } else if args.line_start.is_some() || args.line_end.is_some() {
        format!(
            "read_regions {path} lines {}-{}",
            args.line_start.unwrap_or(1),
            args.line_end.unwrap_or(args.line_start.unwrap_or(1))
        )
    } else {
        format!("read_regions {path}")
    }
}

fn glob_request_summary(args: &Packet28GlobArgs) -> String {
    if args.paths.is_empty() {
        format!(
            "glob '{}' across repo ({:?})",
            args.pattern, args.response_mode
        )
    } else {
        format!(
            "glob '{}' in {} path(s) ({:?})",
            args.pattern,
            args.paths.len(),
            args.response_mode
        )
    }
}

fn estimate_tokens_for_value(value: &Value) -> u64 {
    let bytes = serde_json::to_vec(value).unwrap_or_default().len() as f64;
    (bytes / 4.0).ceil() as u64
}

fn append_unique(items: &mut Vec<String>, value: impl Into<String>) {
    let value = value.into();
    if !value.is_empty() && !items.iter().any(|existing| existing == &value) {
        items.push(value);
    }
}

fn merge_string_lists<'a>(lists: impl IntoIterator<Item = &'a [String]>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
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
                return true
            }
            _ => {}
        }
    }
    false
}

fn search_backend_name(result: &packet28_reducer_core::SearchResult) -> String {
    result
        .engine
        .as_ref()
        .map(|engine| engine.engine.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

fn should_shadow_with_native(
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

fn merge_search_results(
    request: &packet28_reducer_core::SearchRequest,
    mut primary: packet28_reducer_core::SearchResult,
    secondary: &packet28_reducer_core::SearchResult,
) -> (packet28_reducer_core::SearchResult, usize, usize) {
    let mut group_matches = std::collections::BTreeMap::<
        String,
        std::collections::BTreeMap<(usize, String), packet28_reducer_core::SearchMatch>,
    >::new();
    let mut group_counts = std::collections::BTreeMap::<String, usize>::new();
    let mut primary_displayed = std::collections::BTreeSet::<(String, usize, String)>::new();

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

    let primary_paths = primary
        .paths
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
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
        .collect::<std::collections::BTreeSet<_>>()
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

fn build_search_slim_payload(
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

fn build_search_request(
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

fn build_search_full_payload(
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

fn build_search_response_payload(
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

fn execute_search_primary(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    request: &packet28_reducer_core::SearchRequest,
) -> Result<packet28_reducer_core::SearchResult> {
    match packet28_search_via_session(root, session, request.clone()) {
        Ok(result) => Ok(result),
        Err(daemon_error) => {
            let mut fallback = packet28_reducer_core::search(root, request)?;
            if let Some(engine) = fallback.engine.as_mut() {
                engine.fallback_reason = Some(daemon_error.to_string());
            }
            Ok(fallback)
        }
    }
}

fn execute_search_with_strategy(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    request: &packet28_reducer_core::SearchRequest,
    strategy: Packet28SearchStrategy,
) -> Result<(packet28_reducer_core::SearchResult, Packet28SearchExecution)> {
    match strategy {
        Packet28SearchStrategy::Native => {
            let result = packet28_reducer_core::search(root, request)?;
            let execution = Packet28SearchExecution {
                strategy,
                primary_backend: search_backend_name(&result),
                secondary_backend: None,
                shadowed: false,
                added_displayed_matches: 0,
                added_paths: 0,
                notes: vec!["used native search backend directly".to_string()],
            };
            Ok((result, execution))
        }
        Packet28SearchStrategy::Indexed => {
            let result =
                packet28_search_via_session_with_force(root, session, request.clone(), true)?;
            let execution = Packet28SearchExecution {
                strategy,
                primary_backend: search_backend_name(&result),
                secondary_backend: None,
                shadowed: false,
                added_displayed_matches: 0,
                added_paths: 0,
                notes: vec!["forced indexed search backend".to_string()],
            };
            Ok((result, execution))
        }
        Packet28SearchStrategy::Hybrid | Packet28SearchStrategy::Recall => {
            let mut primary = execute_search_primary(root, session, request)?;
            let primary_backend = search_backend_name(&primary);
            let mut execution = Packet28SearchExecution {
                strategy,
                primary_backend: primary_backend.clone(),
                secondary_backend: None,
                shadowed: false,
                added_displayed_matches: 0,
                added_paths: 0,
                notes: Vec::new(),
            };
            if should_shadow_with_native(request, &primary, strategy) {
                match packet28_reducer_core::search(root, request) {
                    Ok(secondary) => {
                        execution.secondary_backend = Some(search_backend_name(&secondary));
                        execution.shadowed = true;
                        let (mut merged, added_displayed_matches, added_paths) =
                            merge_search_results(request, primary, &secondary);
                        execution.added_displayed_matches = added_displayed_matches;
                        execution.added_paths = added_paths;
                        if added_displayed_matches > 0 || added_paths > 0 {
                            append_unique(
                                &mut merged.diagnostics,
                                format!(
                                    "native recall verification added {added_displayed_matches} displayed matches across {added_paths} new paths"
                                ),
                            );
                            execution.notes.push(format!(
                                "native recall verification added {added_displayed_matches} displayed matches across {added_paths} new paths"
                            ));
                        } else {
                            append_unique(
                                &mut merged.diagnostics,
                                "native recall verification confirmed the indexed result",
                            );
                            execution.notes.push(
                                "native recall verification confirmed the indexed result"
                                    .to_string(),
                            );
                        }
                        primary = merged;
                    }
                    Err(error) => {
                        append_unique(
                            &mut primary.diagnostics,
                            format!("native recall verification failed: {error}"),
                        );
                        execution
                            .notes
                            .push(format!("native recall verification failed: {error}"));
                    }
                }
            } else if primary_backend == "indexed_regex" {
                execution.notes.push(
                    "indexed search was selective enough to skip native recall verification"
                        .to_string(),
                );
            } else {
                execution
                    .notes
                    .push(format!("primary backend already used {}", primary_backend));
            }
            Ok((primary, execution))
        }
    }
}

fn write_native_tool_result(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    record: NativeToolResultRecord<'_>,
) -> Result<()> {
    write_auto_capture_state_batch_via_session(
        root,
        session,
        vec![BrokerWriteStateRequest {
            task_id: record.task_id.to_string(),
            op: Some(BrokerWriteOp::ToolResult),
            invocation_id: Some(record.invocation_id.to_string()),
            tool_name: Some(record.tool_name.to_string()),
            operation_kind: Some(record.operation_kind),
            request_summary: Some(record.request_summary),
            result_summary: Some(record.result_summary),
            compact_path: Some(record.compact_path.to_string()),
            raw_est_tokens: record.raw_est_tokens,
            reduced_est_tokens: record.reduced_est_tokens,
            search_query: record.search_query,
            command: record.command,
            sequence: Some(record.sequence),
            duration_ms: Some(record.duration_ms),
            paths: record.paths,
            regions: record.regions,
            symbols: record.symbols,
            artifact_id: record.artifact_id,
            raw_artifact_handle: record.raw_artifact_handle.clone(),
            raw_artifact_available: Some(record.raw_artifact_handle.is_some()),
            refresh_context: Some(false),
            ..BrokerWriteStateRequest::default()
        }],
    )
}

struct NativeToolResultRecord<'a> {
    task_id: &'a str,
    invocation_id: &'a str,
    sequence: u64,
    tool_name: &'a str,
    operation_kind: suite_packet_core::ToolOperationKind,
    request_summary: String,
    result_summary: String,
    compact_path: &'a str,
    raw_est_tokens: Option<u64>,
    reduced_est_tokens: Option<u64>,
    search_query: Option<String>,
    command: Option<String>,
    paths: Vec<String>,
    regions: Vec<String>,
    symbols: Vec<String>,
    artifact_id: Option<String>,
    raw_artifact_handle: Option<String>,
    duration_ms: u64,
}

fn write_native_tool_failure(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    record: NativeToolFailureRecord<'_>,
) -> Result<()> {
    write_auto_capture_state_batch_via_session(
        root,
        session,
        vec![BrokerWriteStateRequest {
            task_id: record.task_id.to_string(),
            op: Some(BrokerWriteOp::ToolInvocationFailed),
            invocation_id: Some(record.invocation_id.to_string()),
            tool_name: Some(record.tool_name.to_string()),
            operation_kind: Some(record.operation_kind),
            request_summary: Some(record.request_summary),
            compact_path: Some(record.compact_path.to_string()),
            error_class: Some(classify_error_message(&record.error_message)),
            error_message: Some(record.error_message.clone()),
            raw_est_tokens: record.raw_est_tokens,
            reduced_est_tokens: record.reduced_est_tokens,
            retryable: Some(is_retryable_error(&record.error_message)),
            sequence: Some(record.sequence),
            duration_ms: Some(record.duration_ms),
            command: record.command,
            raw_artifact_handle: record.raw_artifact_handle.clone(),
            raw_artifact_available: Some(record.raw_artifact_handle.is_some()),
            refresh_context: Some(false),
            ..BrokerWriteStateRequest::default()
        }],
    )
}

struct NativeToolFailureRecord<'a> {
    task_id: &'a str,
    invocation_id: &'a str,
    sequence: u64,
    tool_name: &'a str,
    operation_kind: suite_packet_core::ToolOperationKind,
    request_summary: String,
    error_message: String,
    compact_path: &'a str,
    raw_est_tokens: Option<u64>,
    reduced_est_tokens: Option<u64>,
    command: Option<String>,
    raw_artifact_handle: Option<String>,
    duration_ms: u64,
}

pub(crate) fn handle_packet28_search(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    args: Packet28SearchArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.search requires task_id"));
    }
    let query = args.query.trim();
    if query.is_empty() {
        return Err(anyhow!("packet28.search requires query"));
    }
    let (sequence, invocation_id) = next_task_invocation(session, task_id)?;
    let request_summary = search_request_summary(&args);

    let request = build_search_request(
        query,
        args.paths.clone(),
        args.fixed_string,
        args.case_sensitive,
        args.whole_word,
        args.context_lines,
        args.max_matches_per_file,
        args.max_total_matches,
    );
    let started_at = Instant::now();
    let (search_result, execution) =
        match execute_search_with_strategy(root, session, &request, args.search_strategy) {
            Ok(result) => result,
            Err(error) => {
                let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                write_native_tool_failure(
                    root,
                    session,
                    NativeToolFailureRecord {
                        task_id,
                        invocation_id: &invocation_id,
                        sequence,
                        tool_name: "packet28.search",
                        operation_kind: suite_packet_core::ToolOperationKind::Search,
                        request_summary,
                        error_message: error.to_string(),
                        compact_path: "native_tool",
                        raw_est_tokens: None,
                        reduced_est_tokens: None,
                        command: None,
                        raw_artifact_handle: None,
                        duration_ms,
                    },
                )?;
                return Err(error);
            }
        };
    let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let result_summary = if search_result.match_count == 0 {
        if !args.paths.is_empty() && search_result.resolved_paths.is_empty() {
            format!(
                "No search paths resolved for '{}' ({} requested path(s) missing)",
                query,
                args.paths.len()
            )
        } else if search_result.resolved_paths.is_empty() {
            format!("No matches for '{}' across repo", query)
        } else {
            format!(
                "No matches for '{}' in {} path(s)",
                query,
                search_result.resolved_paths.len()
            )
        }
    } else {
        search_result
            .compact_preview
            .lines()
            .next()
            .unwrap_or("Search completed")
            .to_string()
    };
    let mut full_payload = build_search_full_payload(&search_result, &execution);
    full_payload["task_id"] = json!(task_id);
    full_payload["invocation_id"] = json!(invocation_id);
    full_payload["sequence"] = json!(sequence);
    let artifact_id = Some(store_result_artifact(
        root,
        task_id,
        full_payload["invocation_id"].as_str().unwrap_or_default(),
        &full_payload,
    )?);
    let payload = build_search_response_payload(
        &search_result,
        &execution,
        &args.response_mode,
        artifact_id.clone(),
    );
    let raw_est_tokens = Some(estimate_tokens_for_value(&full_payload));
    let reduced_est_tokens = Some(estimate_tokens_for_value(&payload));
    write_native_tool_result(
        root,
        session,
        NativeToolResultRecord {
            task_id,
            invocation_id: &invocation_id,
            sequence,
            tool_name: "packet28.search",
            operation_kind: suite_packet_core::ToolOperationKind::Search,
            request_summary,
            result_summary,
            compact_path: "native_tool",
            raw_est_tokens,
            reduced_est_tokens,
            search_query: Some(query.to_string()),
            command: None,
            paths: search_result.paths.clone(),
            regions: search_result.regions.clone(),
            symbols: search_result.symbols.clone(),
            artifact_id,
            raw_artifact_handle: None,
            duration_ms,
        },
    )?;
    Ok(payload)
}

pub(crate) fn handle_packet28_search_fast(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    args: Packet28SearchFastArgs,
) -> Result<Value> {
    let query = args.query.trim();
    if query.is_empty() {
        return Err(anyhow!("packet28.search_fast requires query"));
    }
    let request = build_search_request(
        query,
        args.paths.clone(),
        args.fixed_string,
        args.case_sensitive,
        args.whole_word,
        args.context_lines,
        args.max_matches_per_file,
        args.max_total_matches,
    );
    let (search_result, execution) =
        execute_search_with_strategy(root, session, &request, args.search_strategy)?;
    Ok(build_search_response_payload(
        &search_result,
        &execution,
        &args.response_mode,
        None,
    ))
}

pub(crate) fn handle_packet28_fetch_tool_result(
    root: &Path,
    args: Packet28FetchToolResultArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.fetch_tool_result requires task_id"));
    }
    let (artifact_id, mut payload) = load_tool_result_artifact(
        root,
        task_id,
        args.artifact_id.as_deref(),
        args.invocation_id.as_deref(),
    )?;
    if payload.get("artifact_id").is_none() {
        payload["artifact_id"] = json!(artifact_id.clone());
    }
    if payload.get("response_mode").is_none() {
        payload["response_mode"] = json!("full");
    }
    Ok(payload)
}

pub(crate) fn handle_packet28_fetch_raw_output(
    root: &Path,
    args: Packet28FetchRawOutputArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.fetch_raw_output requires task_id"));
    }
    let (path, content) = load_raw_output_artifact(root, task_id, &args.handle)?;
    Ok(json!({
        "task_id": task_id,
        "handle": args.handle,
        "path": path,
        "content": content,
        "line_count": content.lines().count(),
    }))
}

pub(crate) fn handle_packet28_fetch_context(
    root: &Path,
    args: Packet28FetchContextArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.fetch_context requires task_id"));
    }
    let artifact_id = args
        .artifact_id
        .or(args.context_version)
        .ok_or_else(|| anyhow!("packet28.fetch_context requires artifact_id or context_version"))?;
    let path = task_version_json_path(root, task_id, &artifact_id);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "failed to read stored broker context artifact '{}'",
            path.display()
        )
    })?;
    let mut payload: Value = serde_json::from_slice(&bytes)?;
    if payload.get("artifact_id").is_none() {
        payload["artifact_id"] = json!(artifact_id.clone());
    }
    // Honour response_mode: when slim is requested, strip heavy section
    // data and keep only the metadata the agent needs to decide next steps.
    if matches!(args.response_mode, Some(BrokerResponseMode::Slim)) {
        if let Some(obj) = payload.as_object_mut() {
            obj.remove("sections");
            obj.remove("delta");
            obj.remove("evidence_cache");
            obj.remove("search_evidence");
            obj.remove("code_evidence");
        }
        payload["response_mode"] = json!("slim");
    } else if payload.get("response_mode").is_none() {
        payload["response_mode"] = json!("full");
    }
    Ok(payload)
}

pub(crate) fn handle_packet28_prepare_handoff(
    root: &Path,
    args: Packet28PrepareHandoffArgs,
) -> Result<Value> {
    let response = crate::broker_client::prepare_handoff(
        root,
        BrokerPrepareHandoffRequest {
            task_id: args.task_id,
            query: args.query,
            response_mode: args.response_mode,
            include_debug_memory: false,
        },
    )?;
    Ok(serde_json::to_value(response)?)
}

pub(crate) fn handle_packet28_write_intention(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    args: Packet28WriteIntentionArgs,
) -> Result<Value> {
    let text = args.text.trim();
    if text.is_empty() {
        return Err(anyhow!("packet28.write_intention requires text"));
    }
    if args.task_id.trim().is_empty() {
        return Err(anyhow!("packet28.write_intention requires task_id"));
    }
    crate::cmd_mcp::support::track_task(session, root, &args.task_id)?;
    let response = crate::broker_client::write_intention(
        root,
        BrokerWriteStateRequest {
            task_id: args.task_id,
            text: Some(text.to_string()),
            note: args.note,
            step_id: args.step_id,
            question_id: args.question_id,
            paths: args.paths,
            symbols: args.symbols,
            refresh_context: Some(false),
            ..BrokerWriteStateRequest::default()
        },
    )?;
    Ok(serde_json::to_value(response)?)
}

pub(crate) fn handle_packet28_read_regions(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    args: Packet28ReadRegionsArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.read_regions requires task_id"));
    }
    let (sequence, invocation_id) = next_task_invocation(session, task_id)?;
    let request_summary = read_regions_request_summary(&args, &args.path);

    let started_at = Instant::now();
    let read_result = match packet28_reducer_core::read_regions(
        root,
        &packet28_reducer_core::ReadRegionsRequest {
            path: args.path.clone(),
            regions: args.regions.clone(),
            line_start: args.line_start,
            line_end: args.line_end,
        },
    ) {
        Ok(result) => result,
        Err(error) => {
            let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            write_native_tool_failure(
                root,
                session,
                NativeToolFailureRecord {
                    task_id,
                    invocation_id: &invocation_id,
                    sequence,
                    tool_name: "packet28.read_regions",
                    operation_kind: suite_packet_core::ToolOperationKind::Read,
                    request_summary,
                    error_message: error.to_string(),
                    compact_path: "native_tool",
                    raw_est_tokens: None,
                    reduced_est_tokens: None,
                    command: None,
                    raw_artifact_handle: None,
                    duration_ms,
                },
            )?;
            return Err(error);
        }
    };
    let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let result_summary = format!(
        "Read {} line(s) from {} across {} region(s)",
        read_result.lines.len(),
        read_result.path,
        read_result.regions.len()
    );
    let full_payload = json!({
        "task_id": task_id,
        "invocation_id": invocation_id,
        "sequence": sequence,
        "path": read_result.path,
        "regions": read_result.regions,
        "symbols": read_result.symbols,
        "lines": read_result.lines,
        "compact_preview": read_result.compact_preview,
        "response_mode": "full",
    });
    let artifact_id = Some(store_result_artifact(
        root,
        task_id,
        full_payload["invocation_id"].as_str().unwrap_or_default(),
        &full_payload,
    )?);
    let payload = match args.response_mode {
        Packet28SearchResponseMode::Full => {
            let mut payload = full_payload.clone();
            payload["artifact_id"] = json!(artifact_id.clone());
            payload
        }
        Packet28SearchResponseMode::Slim => json!({
            "path": read_result.path,
            "regions": read_result.regions,
            "symbols": read_result.symbols,
            "compact_preview": read_result.compact_preview,
            "artifact_id": artifact_id.clone(),
            "response_mode": "slim",
        }),
    };
    let raw_est_tokens = Some(estimate_tokens_for_value(&full_payload));
    let reduced_est_tokens = Some(estimate_tokens_for_value(&payload));
    write_native_tool_result(
        root,
        session,
        NativeToolResultRecord {
            task_id,
            invocation_id: payload["invocation_id"].as_str().unwrap_or_default(),
            sequence,
            tool_name: "packet28.read_regions",
            operation_kind: suite_packet_core::ToolOperationKind::Read,
            request_summary,
            result_summary,
            compact_path: "native_tool",
            raw_est_tokens,
            reduced_est_tokens,
            search_query: None,
            command: None,
            paths: vec![payload["path"].as_str().unwrap_or_default().to_string()],
            regions: json_array_strings(&full_payload, "regions"),
            symbols: json_array_strings(&full_payload, "symbols"),
            artifact_id,
            raw_artifact_handle: None,
            duration_ms,
        },
    )?;
    Ok(payload)
}

fn collect_glob_matches(
    root: &Path,
    pattern: &str,
    requested_paths: &[String],
) -> Result<(Vec<String>, Vec<String>)> {
    let compiled =
        Pattern::new(pattern).with_context(|| format!("invalid glob pattern '{pattern}'"))?;
    let mut stack = Vec::<std::path::PathBuf>::new();
    let mut resolved_paths = Vec::<String>::new();
    if requested_paths.is_empty() {
        stack.push(root.to_path_buf());
    } else {
        for requested in requested_paths {
            let normalized = packet28_reducer_core::normalize_capture_path(root, requested);
            if normalized.is_empty() {
                continue;
            }
            let candidate = root.join(&normalized);
            if candidate.exists() {
                resolved_paths.push(normalized);
                stack.push(candidate);
            }
        }
    }
    let mut matches = Vec::<String>::new();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            for entry in fs::read_dir(&path)
                .with_context(|| format!("failed to read directory '{}'", path.display()))?
            {
                let entry = entry?;
                let child = entry.path();
                let relative = packet28_reducer_core::normalize_capture_path(
                    root,
                    &child.display().to_string(),
                );
                if relative.starts_with(".git/") || relative.starts_with(".packet28/") {
                    continue;
                }
                if child.is_dir() {
                    stack.push(child);
                    continue;
                }
                if !relative.is_empty() && compiled.matches(&relative) {
                    matches.push(relative);
                }
            }
        } else {
            let relative =
                packet28_reducer_core::normalize_capture_path(root, &path.display().to_string());
            if !relative.is_empty() && compiled.matches(&relative) {
                matches.push(relative);
            }
        }
    }
    matches.sort();
    matches.dedup();
    resolved_paths.sort();
    resolved_paths.dedup();
    Ok((resolved_paths, matches))
}

fn render_glob_compact_preview(pattern: &str, matches: &[String]) -> String {
    let mut rendered = vec![format!(
        "Glob matched {} path(s) for '{}'.",
        matches.len(),
        pattern
    )];
    for path in matches.iter().take(12) {
        rendered.push(path.clone());
    }
    if matches.len() > 12 {
        rendered.push(format!("+{} more path(s)", matches.len() - 12));
    }
    rendered.join("\n")
}

pub(crate) fn handle_packet28_glob(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    args: Packet28GlobArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.glob requires task_id"));
    }
    let pattern = args.pattern.trim();
    if pattern.is_empty() {
        return Err(anyhow!("packet28.glob requires pattern"));
    }
    let (sequence, invocation_id) = next_task_invocation(session, task_id)?;
    let request_summary = glob_request_summary(&args);
    let started_at = Instant::now();
    let (resolved_paths, mut matches) = match collect_glob_matches(root, pattern, &args.paths) {
        Ok(result) => result,
        Err(error) => {
            let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            write_native_tool_failure(
                root,
                session,
                NativeToolFailureRecord {
                    task_id,
                    invocation_id: &invocation_id,
                    sequence,
                    tool_name: "packet28.glob",
                    operation_kind: suite_packet_core::ToolOperationKind::Search,
                    request_summary,
                    error_message: error.to_string(),
                    compact_path: "native_tool",
                    raw_est_tokens: None,
                    reduced_est_tokens: None,
                    command: None,
                    raw_artifact_handle: None,
                    duration_ms,
                },
            )?;
            return Err(error);
        }
    };
    let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let max_results = args.max_results.unwrap_or(200).clamp(1, 500);
    let truncated = matches.len() > max_results;
    if truncated {
        matches.truncate(max_results);
    }
    let compact_preview = render_glob_compact_preview(pattern, &matches);
    let result_summary = compact_preview
        .lines()
        .next()
        .unwrap_or("Glob completed")
        .to_string();
    let slim_preview = result_summary.clone();
    let matched_paths = matches.clone();
    let symbols = packet28_reducer_core::infer_symbols_from_pattern(pattern);
    let full_payload = json!({
        "task_id": task_id,
        "invocation_id": invocation_id,
        "sequence": sequence,
        "pattern": pattern,
        "requested_paths": args.paths,
        "resolved_paths": resolved_paths,
        "match_count": matches.len(),
        "truncated": truncated,
        "paths": matched_paths.clone(),
        "symbols": symbols.clone(),
        "compact_preview": compact_preview,
        "response_mode": "full",
    });
    let artifact_id = Some(store_result_artifact(
        root,
        task_id,
        full_payload["invocation_id"].as_str().unwrap_or_default(),
        &full_payload,
    )?);
    let payload = match args.response_mode {
        Packet28SearchResponseMode::Full => {
            let mut payload = full_payload.clone();
            payload["artifact_id"] = json!(artifact_id.clone());
            payload
        }
        Packet28SearchResponseMode::Slim => json!({
            "match_count": matches.len(),
            "compact_preview": slim_preview,
            "artifact_id": artifact_id.clone(),
            "response_mode": "slim",
        }),
    };
    let raw_est_tokens = Some(estimate_tokens_for_value(&full_payload));
    let reduced_est_tokens = Some(estimate_tokens_for_value(&payload));
    write_native_tool_result(
        root,
        session,
        NativeToolResultRecord {
            task_id,
            invocation_id: &invocation_id,
            sequence,
            tool_name: "packet28.glob",
            operation_kind: suite_packet_core::ToolOperationKind::Search,
            request_summary,
            result_summary,
            compact_path: "native_tool",
            raw_est_tokens,
            reduced_est_tokens,
            search_query: Some(pattern.to_string()),
            command: None,
            paths: matched_paths,
            regions: Vec::new(),
            symbols,
            artifact_id,
            raw_artifact_handle: None,
            duration_ms,
        },
    )?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request(query: &str) -> packet28_reducer_core::SearchRequest {
        packet28_reducer_core::SearchRequest {
            query: query.to_string(),
            ..packet28_reducer_core::SearchRequest::default()
        }
    }

    fn sample_match(path: &str, line: usize, text: &str) -> packet28_reducer_core::SearchMatch {
        packet28_reducer_core::SearchMatch {
            path: path.to_string(),
            line,
            text: text.to_string(),
        }
    }

    fn sample_result(
        backend: &str,
        path: &str,
        line: usize,
        text: &str,
    ) -> packet28_reducer_core::SearchResult {
        let item = sample_match(path, line, text);
        packet28_reducer_core::SearchResult {
            query: "Alpha".to_string(),
            match_count: 1,
            returned_match_count: 1,
            paths: vec![path.to_string()],
            regions: vec![packet28_reducer_core::format_region(path, line, line)],
            symbols: vec!["Alpha".to_string()],
            groups: vec![packet28_reducer_core::SearchGroup {
                path: path.to_string(),
                match_count: 1,
                displayed_match_count: 1,
                truncated: false,
                matches: vec![item],
            }],
            compact_preview: format!("Search found 1 matches in 1 files.\n- {path} (1)"),
            engine: Some(packet28_reducer_core::SearchEngineStats {
                engine: backend.to_string(),
                ..packet28_reducer_core::SearchEngineStats::default()
            }),
            ..packet28_reducer_core::SearchResult::default()
        }
    }

    #[test]
    fn hybrid_shadowing_triggers_for_zero_hit_regex() {
        let request = sample_request(r"Alpha|Beta");
        let result = packet28_reducer_core::SearchResult {
            query: request.query.clone(),
            engine: Some(packet28_reducer_core::SearchEngineStats {
                engine: "indexed_regex".to_string(),
                ..packet28_reducer_core::SearchEngineStats::default()
            }),
            ..packet28_reducer_core::SearchResult::default()
        };
        assert!(should_shadow_with_native(
            &request,
            &result,
            Packet28SearchStrategy::Hybrid
        ));
    }

    #[test]
    fn hybrid_shadowing_skips_successful_fixed_string_hits() {
        let request = packet28_reducer_core::SearchRequest {
            query: "AlphaUniqueToken".to_string(),
            fixed_string: true,
            ..packet28_reducer_core::SearchRequest::default()
        };
        let result = sample_result("indexed_regex", "src/alpha.rs", 4, "struct Alpha;");
        assert!(!should_shadow_with_native(
            &request,
            &result,
            Packet28SearchStrategy::Hybrid
        ));
    }

    #[test]
    fn merge_search_results_unions_new_paths_and_matches() {
        let request = sample_request("Alpha");
        let primary = sample_result("indexed_regex", "src/alpha.rs", 4, "struct Alpha;");
        let secondary = sample_result("legacy_rg", "src/beta.rs", 2, "fn beta_alpha() {}");
        let (merged, added_displayed_matches, added_paths) =
            merge_search_results(&request, primary, &secondary);

        assert_eq!(added_displayed_matches, 1);
        assert_eq!(added_paths, 1);
        assert!(merged.paths.iter().any(|path| path == "src/alpha.rs"));
        assert!(merged.paths.iter().any(|path| path == "src/beta.rs"));
        assert!(merged
            .regions
            .iter()
            .any(|region| region == "src/beta.rs:2-2"));
        assert_eq!(merged.groups.len(), 2);
    }

    #[test]
    fn slim_payload_exposes_navigation_fields() {
        let result = sample_result("indexed_regex", "src/alpha.rs", 4, "struct Alpha;");
        let execution = Packet28SearchExecution {
            strategy: Packet28SearchStrategy::Hybrid,
            primary_backend: "indexed_regex".to_string(),
            secondary_backend: Some("legacy_rg".to_string()),
            shadowed: true,
            added_displayed_matches: 1,
            added_paths: 0,
            notes: vec!["native recall verification confirmed the indexed result".to_string()],
        };
        let payload =
            build_search_slim_payload(&result, Some("artifact-1".to_string()), &execution);

        assert_eq!(payload["response_mode"], "slim");
        assert_eq!(payload["search_strategy"], "hybrid");
        assert_eq!(payload["paths"][0], "src/alpha.rs");
        assert_eq!(payload["regions"][0], "src/alpha.rs:4-4");
        assert_eq!(
            payload["compact_preview"],
            "Search found 1 matches in 1 files."
        );
        assert!(payload["engine"].is_object());
        assert!(payload["hybrid"].is_object());
    }

    #[test]
    fn slim_payload_can_omit_artifact_id() {
        let result = sample_result("indexed_regex", "src/alpha.rs", 4, "struct Alpha;");
        let execution = Packet28SearchExecution {
            strategy: Packet28SearchStrategy::Hybrid,
            primary_backend: "indexed_regex".to_string(),
            secondary_backend: None,
            shadowed: false,
            added_displayed_matches: 0,
            added_paths: 0,
            notes: Vec::new(),
        };
        let payload = build_search_slim_payload(&result, None, &execution);

        assert!(payload.get("artifact_id").is_none());
        assert_eq!(payload["response_mode"], "slim");
    }

    #[test]
    fn slim_payload_omits_empty_search_metadata() {
        let result = sample_result("indexed_regex", "src/alpha.rs", 4, "struct Alpha;");
        let execution = Packet28SearchExecution {
            strategy: Packet28SearchStrategy::Hybrid,
            primary_backend: "indexed_regex".to_string(),
            secondary_backend: None,
            shadowed: false,
            added_displayed_matches: 0,
            added_paths: 0,
            notes: Vec::new(),
        };
        let payload =
            build_search_slim_payload(&result, Some("artifact-1".to_string()), &execution);

        assert_eq!(payload["engine"]["engine"], "indexed_regex");
        assert!(payload["engine"].get("fallback_reason").is_none());
        assert_eq!(payload["hybrid"]["primary_backend"], "indexed_regex");
        assert!(payload["hybrid"].get("secondary_backend").is_none());
        assert!(payload["hybrid"].get("notes").is_none());
        assert!(payload.get("returned_match_count").is_none());
        assert!(payload.get("truncated").is_none());
    }

    #[test]
    fn search_fast_native_full_payload_omits_task_fields() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("sample.txt");
        fs::write(&file_path, "derive_agent_snapshot\n").unwrap();
        let session = Arc::new(Mutex::new(McpSessionState::default()));

        let payload = handle_packet28_search_fast(
            dir.path(),
            &session,
            Packet28SearchFastArgs {
                query: "derive_agent_snapshot".to_string(),
                search_strategy: Packet28SearchStrategy::Native,
                response_mode: Packet28SearchResponseMode::Full,
                ..Packet28SearchFastArgs::default()
            },
        )
        .unwrap();

        assert_eq!(payload["response_mode"], "full");
        assert!(payload.get("artifact_id").is_none());
        assert!(payload.get("task_id").is_none());
        assert!(payload.get("invocation_id").is_none());
        assert!(payload.get("sequence").is_none());
        assert_eq!(payload["match_count"], 1);
    }
}
