use super::*;
use crate::cmd_mcp::support::{
    load_raw_output_artifact, load_tool_result_artifact, next_task_invocation,
    packet28_search_via_session, packet28_search_via_session_with_force, store_result_artifact,
    write_auto_capture_state_batch_via_session,
};
use glob::Pattern;

#[path = "cmd_mcp_native_fff.rs"]
mod fff;
#[path = "cmd_mcp_native_search.rs"]
mod search;

#[cfg(test)]
use search::build_search_slim_payload;
use search::{
    build_search_full_payload, build_search_request, build_search_response_payload,
    merge_search_results, search_backend_name, should_shadow_with_native, Packet28SearchExecution,
};

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
    Fff,
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
            Self::Fff => "fff",
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
pub(crate) struct Packet28ValidatePlanArgs {
    pub(crate) task_id: String,
    pub(crate) steps: Vec<BrokerPlanStep>,
    pub(crate) require_read_before_edit: Option<bool>,
    pub(crate) require_test_gate: Option<bool>,
    pub(crate) budget_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct Packet28ActionCriticArgs {
    pub(crate) task_id: String,
    pub(crate) action: BrokerAction,
    pub(crate) query: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) focus_paths: Vec<String>,
    pub(crate) focus_symbols: Vec<String>,
    pub(crate) budget_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28RecommendNextToolArgs {
    pub(crate) task_id: String,
    pub(crate) query: Option<String>,
    pub(crate) focus_paths: Vec<String>,
    pub(crate) focus_symbols: Vec<String>,
    pub(crate) max_recommendations: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28ValidateToolOutcomeArgs {
    pub(crate) task_id: String,
    pub(crate) command: Option<String>,
    pub(crate) focus_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28PatchRiskArgs {
    pub(crate) task_id: String,
    pub(crate) paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28VerifyHandoffArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28PromptPressureArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
    pub(crate) next_prompt: Option<String>,
    pub(crate) budget_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffDiffArgs {
    pub(crate) task_id: String,
    pub(crate) left_artifact_id: Option<String>,
    pub(crate) left_context_version: Option<String>,
    pub(crate) right_artifact_id: Option<String>,
    pub(crate) right_context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffCompressionArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
    pub(crate) next_prompt: Option<String>,
    pub(crate) budget_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffDependencyLintArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffPathLintArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffTestLintArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffStaleCommandLintArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffEnvironmentLintArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffLintAllArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffFixPlanArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffRepairVerifyArgs {
    pub(crate) task_id: String,
    pub(crate) before_artifact_id: Option<String>,
    pub(crate) before_context_version: Option<String>,
    pub(crate) after_artifact_id: Option<String>,
    pub(crate) after_context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffLintTrendArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_ids: Vec<String>,
    pub(crate) max_artifacts: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffLintRegressionArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_ids: Vec<String>,
    pub(crate) max_artifacts: Option<usize>,
}

impl Default for Packet28ActionCriticArgs {
    fn default() -> Self {
        Self {
            task_id: String::new(),
            action: BrokerAction::ChooseTool,
            query: None,
            tool_name: None,
            focus_paths: Vec::new(),
            focus_symbols: Vec::new(),
            budget_tokens: None,
        }
    }
}

#[derive(Debug, Clone)]
struct ToolRecommendation {
    command: String,
    reason: String,
    evidence: Vec<String>,
    expected_savings_tokens: u64,
    risk: String,
    score: u64,
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

fn estimate_tokens_for_text(value: &str) -> u64 {
    ((value.len() as f64) / 4.0).ceil() as u64
}

fn append_unique(items: &mut Vec<String>, value: impl Into<String>) {
    let value = value.into();
    if !value.is_empty() && !items.iter().any(|existing| existing == &value) {
        items.push(value);
    }
}

fn execute_search_primary(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    request: &packet28_reducer_core::SearchRequest,
) -> Result<packet28_reducer_core::SearchResult> {
    let fff_prefer_error = if fff::mcp_fff_auto_prefer_requested() {
        match fff::execute_fff_search_with_session(root, session, request) {
            Ok(mut result) => {
                append_unique(
                    &mut result.diagnostics,
                    "auto preferred persistent fff MCP backend",
                );
                return Ok(result);
            }
            Err(error) => Some(error.to_string()),
        }
    } else {
        None
    };

    let mut result = match packet28_search_via_session(root, session, request.clone()) {
        Ok(result) => result,
        Err(daemon_error) => {
            let mut fallback = packet28_reducer_core::search(root, request)?;
            if let Some(engine) = fallback.engine.as_mut() {
                engine.fallback_reason = Some(daemon_error.to_string());
            }
            fallback
        }
    };

    if let Some(error) = fff_prefer_error {
        append_unique(
            &mut result.diagnostics,
            format!("fff auto preferred backend failed: {error}"),
        );
        if let Some(engine) = result.engine.as_mut() {
            engine.fallback_reason = match engine.fallback_reason.take() {
                Some(existing) => Some(format!(
                    "fff auto preferred backend failed: {error}; {existing}"
                )),
                None => Some(format!("fff auto preferred backend failed: {error}")),
            };
        }
    }

    Ok(result)
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
        Packet28SearchStrategy::Fff => {
            let result = fff::execute_fff_search_with_session(root, session, request)?;
            let execution = Packet28SearchExecution {
                strategy,
                primary_backend: search_backend_name(&result),
                secondary_backend: None,
                shadowed: false,
                added_displayed_matches: 0,
                added_paths: 0,
                notes: vec!["used persistent fff MCP backend".to_string()],
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
    compact_fetched_tool_result_payload(&mut payload);
    Ok(payload)
}

fn compact_fetched_tool_result_payload(payload: &mut Value) {
    if payload.get("groups").and_then(Value::as_array).is_some() {
        compact_fetched_search_payload(payload);
    }
}

fn compact_fetched_search_payload(payload: &mut Value) {
    let content = render_search_artifact_content(payload);
    if !content.is_empty() {
        payload["content"] = json!(content);
        payload["line_count"] = json!(payload["content"].as_str().unwrap_or("").lines().count());
        payload["content_format"] = json!("path:line:text");
    }
    if let Some(object) = payload.as_object_mut() {
        object.remove("groups");
    }
}

fn render_search_artifact_content(payload: &Value) -> String {
    let mut lines = Vec::new();
    let Some(groups) = payload.get("groups").and_then(Value::as_array) else {
        return String::new();
    };
    for group in groups {
        let Some(matches) = group.get("matches").and_then(Value::as_array) else {
            continue;
        };
        let group_path = group
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for item in matches {
            let path = item
                .get("path")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(group_path);
            let Some(line) = item.get("line").and_then(Value::as_u64) else {
                continue;
            };
            let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
            lines.push(format!("{path}:{line}:{text}"));
        }
    }
    lines.join("\n")
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

pub(crate) fn handle_packet28_verify_handoff(
    root: &Path,
    args: Packet28VerifyHandoffArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.verify_handoff requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.verify_handoff requires artifact_id or context_version")
    })?;
    let path = task_version_json_path(root, task_id, &artifact_id);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "failed to read stored handoff context artifact '{}'",
            path.display()
        )
    })?;
    let payload: Value = serde_json::from_slice(&bytes)?;
    let mut missing = Vec::new();
    let brief = payload
        .get("brief")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !brief.contains("Task Objective") && !brief.contains("task_objective") {
        missing.push("objective".to_string());
    }
    let has_next_action = payload
        .get("next_action_summary")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        || payload
            .get("latest_intention")
            .is_some_and(|value| !value.is_null());
    if !has_next_action {
        missing.push("next_action".to_string());
    }
    let has_debt_signal = section_exists(&payload, "context_debt")
        || section_exists(&payload, "evidence_freshness")
        || payload
            .get("changed_paths_since_checkpoint")
            .and_then(Value::as_array)
            .is_some_and(|paths| !paths.is_empty())
        || payload
            .get("open_questions")
            .and_then(Value::as_array)
            .is_some_and(|questions| !questions.is_empty());
    if !has_debt_signal {
        missing.push("debt_signal".to_string());
    }
    let has_evidence_handle = payload
        .get("artifact_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        || payload
            .get("evidence_artifact_ids")
            .and_then(Value::as_array)
            .is_some_and(|ids| !ids.is_empty());
    if !has_evidence_handle {
        missing.push("evidence_handle".to_string());
    }
    let score = 100_u64.saturating_sub((missing.len() as u64).saturating_mul(25));
    let ready = missing.is_empty();
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ready": ready,
        "score": score,
        "missing": missing,
        "summary": if ready {
            "handoff_replay_ready"
        } else {
            "handoff_replay_incomplete"
        },
    }))
}

pub(crate) fn handle_packet28_prompt_pressure(
    root: &Path,
    args: Packet28PromptPressureArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.prompt_pressure requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.prompt_pressure requires artifact_id or context_version")
    })?;
    let path = task_version_json_path(root, task_id, &artifact_id);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "failed to read stored handoff context artifact '{}'",
            path.display()
        )
    })?;
    let payload: Value = serde_json::from_slice(&bytes)?;
    let budget_tokens = args.budget_tokens.unwrap_or(8_000).max(1);
    let next_prompt = args.next_prompt.unwrap_or_default();
    let context_tokens = estimate_tokens_for_value(&payload);
    let next_prompt_tokens = estimate_tokens_for_text(&next_prompt);
    let total_tokens = context_tokens.saturating_add(next_prompt_tokens);
    let remaining_tokens = budget_tokens as i64 - total_tokens as i64;
    let mut removable_sections = Vec::new();
    if let Some(sections) = payload.get("sections").and_then(Value::as_array) {
        for section in sections {
            let id = section
                .get("id")
                .or_else(|| section.get("title"))
                .and_then(Value::as_str)
                .unwrap_or("section");
            removable_sections.push((id.to_string(), estimate_tokens_for_value(section)));
        }
    }
    removable_sections.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let largest_removable_sections: Vec<Value> = removable_sections
        .into_iter()
        .take(3)
        .map(|(id, tokens)| {
            json!({
                "id": id,
                "tokens": tokens,
            })
        })
        .collect();
    let pressure = if total_tokens > budget_tokens {
        "over_budget"
    } else if total_tokens.saturating_mul(100) >= budget_tokens.saturating_mul(85) {
        "tight"
    } else {
        "ok"
    };
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "budget_tokens": budget_tokens,
        "context_tokens": context_tokens,
        "next_prompt_tokens": next_prompt_tokens,
        "total_tokens": total_tokens,
        "remaining_tokens": remaining_tokens,
        "pressure": pressure,
        "over_budget": total_tokens > budget_tokens,
        "largest_removable_sections": largest_removable_sections,
        "summary": format!("prompt_pressure={pressure} total_tokens={total_tokens} remaining_tokens={remaining_tokens}"),
    }))
}

pub(crate) fn handle_packet28_handoff_diff(
    root: &Path,
    args: Packet28HandoffDiffArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_diff requires task_id"));
    }
    let left_artifact_id = args
        .left_artifact_id
        .or(args.left_context_version)
        .ok_or_else(|| {
            anyhow!("packet28.handoff_diff requires left_artifact_id or left_context_version")
        })?;
    let right_artifact_id = args
        .right_artifact_id
        .or(args.right_context_version)
        .ok_or_else(|| {
            anyhow!("packet28.handoff_diff requires right_artifact_id or right_context_version")
        })?;
    let left = read_handoff_payload(root, task_id, &left_artifact_id, "handoff diff")?;
    let right = read_handoff_payload(root, task_id, &right_artifact_id, "handoff diff")?;
    let mut deltas = Vec::new();
    push_handoff_delta(
        &mut deltas,
        "next_action",
        handoff_next_action(&left),
        handoff_next_action(&right),
    );
    push_handoff_delta(
        &mut deltas,
        "objective",
        handoff_objective(&left),
        handoff_objective(&right),
    );
    push_handoff_delta(
        &mut deltas,
        "evidence_handles",
        handoff_evidence_handle_summary(&left),
        handoff_evidence_handle_summary(&right),
    );
    push_handoff_delta(
        &mut deltas,
        "debt_signal",
        handoff_debt_signal(&left).to_string(),
        handoff_debt_signal(&right).to_string(),
    );
    let top_delta = deltas
        .first()
        .and_then(|delta| delta.get("field"))
        .and_then(Value::as_str)
        .unwrap_or("none");
    Ok(json!({
        "task_id": task_id,
        "left_artifact_id": left_artifact_id,
        "right_artifact_id": right_artifact_id,
        "delta_count": deltas.len(),
        "top_delta": top_delta,
        "deltas": deltas,
        "summary": format!("handoff_diff delta_count={} top_delta={top_delta}", deltas.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_compress(
    root: &Path,
    args: Packet28HandoffCompressionArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_compress requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_compress requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff compression")?;
    let budget_tokens = args.budget_tokens.unwrap_or(8_000).max(1);
    let next_prompt = args.next_prompt.unwrap_or_default();
    let context_tokens = estimate_tokens_for_value(&payload);
    let total_tokens = context_tokens.saturating_add(estimate_tokens_for_text(&next_prompt));
    let mut needed_savings = total_tokens.saturating_sub(budget_tokens);
    let mut candidates = Vec::new();
    if let Some(sections) = payload.get("sections").and_then(Value::as_array) {
        for section in sections {
            let id = section_identifier(section);
            let tokens = estimate_tokens_for_value(section);
            let protected = is_replay_critical_section(section);
            if protected {
                continue;
            }
            candidates.push((id, tokens));
        }
    }
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut selected_tokens = 0_u64;
    let mut recommendations = Vec::new();
    for (id, tokens) in candidates.into_iter().take(4) {
        if needed_savings == 0 {
            break;
        }
        selected_tokens = selected_tokens.saturating_add(tokens);
        needed_savings = needed_savings.saturating_sub(tokens);
        recommendations.push(json!({
            "action": "drop_section",
            "id": id,
            "tokens": tokens,
            "reason": "non_replay_critical_section",
        }));
    }
    let projected_total_tokens = total_tokens.saturating_sub(selected_tokens);
    let projected_over_budget = projected_total_tokens > budget_tokens;
    let recommendation_count = recommendations.len();
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "budget_tokens": budget_tokens,
        "total_tokens": total_tokens,
        "needed_savings_tokens": total_tokens.saturating_sub(budget_tokens),
        "projected_total_tokens": projected_total_tokens,
        "projected_over_budget": projected_over_budget,
        "recommendations": recommendations,
        "summary": format!(
            "handoff_compress recommendations={} projected_over_budget={}",
            recommendation_count,
            projected_over_budget
        ),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_dependencies(
    root: &Path,
    args: Packet28HandoffDependencyLintArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!(
            "packet28.handoff_lint_dependencies requires task_id"
        ));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_dependencies requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff dependency lint")?;
    let available_artifacts = available_handoff_artifacts(&payload);
    let referenced_artifacts = referenced_handoff_artifacts(&payload);
    let mut issues = Vec::new();
    for reference in referenced_artifacts {
        if !available_artifacts
            .iter()
            .any(|available| available == &reference)
        {
            issues.push(json!({
                "kind": "missing_artifact",
                "reference": reference,
                "reason": "referenced artifact handle is absent from artifact_id and evidence_artifact_ids",
            }));
        }
    }
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": issues.is_empty(),
        "issue_count": issues.len(),
        "issues": issues,
        "summary": format!("handoff_dependency_lint issue_count={}", issues.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_paths(
    root: &Path,
    args: Packet28HandoffPathLintArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_lint_paths requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_paths requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff path lint")?;
    let changed_paths = available_handoff_paths(&payload);
    let referenced_paths = referenced_handoff_paths(&payload);
    let mut issues = Vec::new();
    for reference in referenced_paths {
        let exists_on_disk = root.join(&reference).exists();
        let listed_as_changed = changed_paths.iter().any(|path| path == &reference);
        if !exists_on_disk && !listed_as_changed {
            issues.push(json!({
                "kind": "missing_path",
                "reference": reference,
                "reason": "referenced path is absent on disk and not listed in changed_paths_since_checkpoint",
            }));
        }
    }
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": issues.is_empty(),
        "issue_count": issues.len(),
        "issues": issues,
        "summary": format!("handoff_path_lint issue_count={}", issues.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_tests(
    root: &Path,
    args: Packet28HandoffTestLintArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_lint_tests requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_tests requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff test lint")?;
    let text_blocks = handoff_text_blocks(&payload);
    let mut mentioned_tests = Vec::new();
    let mut command_backed_tests = Vec::new();
    for text in &text_blocks {
        collect_test_mentions(text, &mut mentioned_tests);
        collect_command_backed_tests(text, &mut command_backed_tests);
    }
    let mut issues = Vec::new();
    for test_name in mentioned_tests {
        if !command_backed_tests
            .iter()
            .any(|command_test| command_test == &test_name)
        {
            issues.push(json!({
                "kind": "missing_test_command",
                "reference": test_name,
                "reason": "test-like name is mentioned without a runnable test command in the same handoff",
            }));
        }
    }
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": issues.is_empty(),
        "issue_count": issues.len(),
        "issues": issues,
        "summary": format!("handoff_test_lint issue_count={}", issues.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_stale_commands(
    root: &Path,
    args: Packet28HandoffStaleCommandLintArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!(
            "packet28.handoff_lint_stale_commands requires task_id"
        ));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_stale_commands requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff stale-command lint")?;
    let command_refs = referenced_handoff_commands(&payload);
    let changed_paths = available_handoff_paths(&payload);
    let events = load_task_events(root, task_id).unwrap_or_default();
    let latest_edit_at = latest_relevant_edit_at(&events, &changed_paths);
    let mut issues = Vec::new();
    if let Some(latest_edit_at) = latest_edit_at {
        for command in command_refs {
            if let Some(command_at) = latest_command_event_at(&events, &command) {
                if command_at < latest_edit_at {
                    issues.push(json!({
                        "kind": "stale_command",
                        "reference": command,
                        "command_at_unix": command_at,
                        "latest_edit_at_unix": latest_edit_at,
                        "reason": "referenced command ran before the latest relevant edit",
                    }));
                }
            }
        }
    }
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": issues.is_empty(),
        "issue_count": issues.len(),
        "issues": issues,
        "summary": format!("handoff_stale_command_lint issue_count={}", issues.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_environment(
    root: &Path,
    args: Packet28HandoffEnvironmentLintArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!(
            "packet28.handoff_lint_environment requires task_id"
        ));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_environment requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff environment lint")?;
    let command_refs = referenced_handoff_commands(&payload);
    let mut issues = Vec::new();
    for command in command_refs {
        if let Some(executable) = command_executable(&command) {
            if !executable_exists(&executable) {
                issues.push(json!({
                    "kind": "missing_tool",
                    "reference": executable,
                    "command": command,
                    "reason": "command executable was not found on PATH",
                }));
            }
        }
        for env_var in command_env_vars(&command) {
            if std::env::var_os(&env_var).is_none() {
                issues.push(json!({
                    "kind": "missing_env",
                    "reference": env_var,
                    "command": command,
                    "reason": "command references an environment variable that is not set",
                }));
            }
        }
    }
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": issues.is_empty(),
        "issue_count": issues.len(),
        "issues": issues,
        "summary": format!("handoff_environment_lint issue_count={}", issues.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_all(
    root: &Path,
    args: Packet28HandoffLintAllArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_lint_all requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_all requires artifact_id or context_version")
    })?;
    let checks = vec![
        handoff_lint_check(
            "replay",
            handle_packet28_verify_handoff(
                root,
                Packet28VerifyHandoffArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
        handoff_lint_check(
            "dependencies",
            handle_packet28_handoff_lint_dependencies(
                root,
                Packet28HandoffDependencyLintArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
        handoff_lint_check(
            "paths",
            handle_packet28_handoff_lint_paths(
                root,
                Packet28HandoffPathLintArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
        handoff_lint_check(
            "tests",
            handle_packet28_handoff_lint_tests(
                root,
                Packet28HandoffTestLintArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
        handoff_lint_check(
            "stale_commands",
            handle_packet28_handoff_lint_stale_commands(
                root,
                Packet28HandoffStaleCommandLintArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
        handoff_lint_check(
            "environment",
            handle_packet28_handoff_lint_environment(
                root,
                Packet28HandoffEnvironmentLintArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
    ];
    let failing_categories: Vec<String> = checks
        .iter()
        .filter(|check| !check.get("ok").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|check| {
            check
                .get("category")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let issue_count: u64 = checks
        .iter()
        .map(|check| {
            check
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        })
        .sum();
    let ok = failing_categories.is_empty();
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": ok,
        "status": if ok { "ready" } else { "blocked" },
        "issue_count": issue_count,
        "failing_categories": failing_categories,
        "checks": checks,
        "summary": format!("handoff_lint_all status={} issue_count={issue_count}", if ok { "ready" } else { "blocked" }),
    }))
}

pub(crate) fn handle_packet28_handoff_fix_plan(
    root: &Path,
    args: Packet28HandoffFixPlanArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_fix_plan requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_fix_plan requires artifact_id or context_version")
    })?;
    let lint = handle_packet28_handoff_lint_all(
        root,
        Packet28HandoffLintAllArgs {
            task_id: task_id.to_string(),
            artifact_id: Some(artifact_id.clone()),
            context_version: None,
        },
    )?;
    let actions = handoff_fix_actions_from_lint(&lint);
    let action_count = actions.len();
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "status": if action_count == 0 { "ready" } else { "needs_fix" },
        "action_count": action_count,
        "actions": actions,
        "summary": format!("handoff_fix_plan action_count={action_count}"),
    }))
}

pub(crate) fn handle_packet28_handoff_repair_verify(
    root: &Path,
    args: Packet28HandoffRepairVerifyArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_repair_verify requires task_id"));
    }
    let before_artifact_id = args
        .before_artifact_id
        .or(args.before_context_version)
        .ok_or_else(|| {
            anyhow!("packet28.handoff_repair_verify requires before_artifact_id or before_context_version")
        })?;
    let after_artifact_id = args
        .after_artifact_id
        .or(args.after_context_version)
        .ok_or_else(|| {
            anyhow!("packet28.handoff_repair_verify requires after_artifact_id or after_context_version")
        })?;
    let before = handle_packet28_handoff_lint_all(
        root,
        Packet28HandoffLintAllArgs {
            task_id: task_id.to_string(),
            artifact_id: Some(before_artifact_id.clone()),
            context_version: None,
        },
    )?;
    let after = handle_packet28_handoff_lint_all(
        root,
        Packet28HandoffLintAllArgs {
            task_id: task_id.to_string(),
            artifact_id: Some(after_artifact_id.clone()),
            context_version: None,
        },
    )?;
    let before_categories = lint_failing_categories(&before);
    let after_categories = lint_failing_categories(&after);
    let cleared_categories: Vec<String> = before_categories
        .iter()
        .filter(|category| !after_categories.iter().any(|after| after == *category))
        .cloned()
        .collect();
    let regressed_categories: Vec<String> = after_categories
        .iter()
        .filter(|category| !before_categories.iter().any(|before| before == *category))
        .cloned()
        .collect();
    let verified = after_categories.is_empty();
    let cleared_count = cleared_categories.len();
    Ok(json!({
        "task_id": task_id,
        "before_artifact_id": before_artifact_id,
        "after_artifact_id": after_artifact_id,
        "verified": verified,
        "cleared_categories": cleared_categories,
        "remaining_categories": after_categories,
        "regressed_categories": regressed_categories,
        "summary": format!("handoff_repair_verify verified={verified} cleared={cleared_count}"),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_trends(
    root: &Path,
    args: Packet28HandoffLintTrendArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_lint_trends requires task_id"));
    }
    let max_artifacts = args.max_artifacts.unwrap_or(8).clamp(1, 24);
    let artifact_ids = if args.artifact_ids.is_empty() {
        discover_handoff_artifact_ids(root, task_id, max_artifacts)?
    } else {
        args.artifact_ids
            .into_iter()
            .filter(|id| !id.trim().is_empty())
            .take(max_artifacts)
            .collect()
    };
    let mut records = Vec::new();
    let mut category_counts = std::collections::BTreeMap::<String, u64>::new();
    let mut latest_categories = Vec::<String>::new();
    for artifact_id in &artifact_ids {
        let lint = handle_packet28_handoff_lint_all(
            root,
            Packet28HandoffLintAllArgs {
                task_id: task_id.to_string(),
                artifact_id: Some(artifact_id.clone()),
                context_version: None,
            },
        )?;
        let categories = lint_failing_categories(&lint);
        latest_categories = categories.clone();
        for category in &categories {
            *category_counts.entry(category.clone()).or_default() += 1;
        }
        records.push(json!({
            "artifact_id": artifact_id,
            "failing_categories": categories,
        }));
    }
    let recurring_categories: Vec<Value> = category_counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(category, count)| {
            json!({
                "category": category,
                "count": count,
            })
        })
        .collect();
    let cleared_categories: Vec<String> = category_counts
        .keys()
        .filter(|category| !latest_categories.iter().any(|latest| latest == *category))
        .cloned()
        .collect();
    Ok(json!({
        "task_id": task_id,
        "artifact_count": records.len(),
        "latest_artifact_id": artifact_ids.last().cloned().unwrap_or_default(),
        "latest_blocking_categories": latest_categories,
        "recurring_categories": recurring_categories,
        "cleared_categories": cleared_categories,
        "records": records,
        "summary": format!(
            "handoff_lint_trends artifacts={} recurring={} cleared={}",
            artifact_ids.len(),
            category_counts.values().filter(|count| **count > 1).count(),
            category_counts.keys().filter(|category| {
                !lint_latest_category_contains(&latest_categories, category)
            }).count()
        ),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_regressions(
    root: &Path,
    args: Packet28HandoffLintRegressionArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!(
            "packet28.handoff_lint_regressions requires task_id"
        ));
    }
    let max_artifacts = args.max_artifacts.unwrap_or(8).clamp(1, 24);
    let artifact_ids = if args.artifact_ids.is_empty() {
        discover_handoff_artifact_ids(root, task_id, max_artifacts)?
    } else {
        args.artifact_ids
            .into_iter()
            .filter(|id| !id.trim().is_empty())
            .take(max_artifacts)
            .collect()
    };
    let mut snapshots = Vec::<(String, Vec<String>)>::new();
    for artifact_id in &artifact_ids {
        let lint = handle_packet28_handoff_lint_all(
            root,
            Packet28HandoffLintAllArgs {
                task_id: task_id.to_string(),
                artifact_id: Some(artifact_id.clone()),
                context_version: None,
            },
        )?;
        snapshots.push((artifact_id.clone(), lint_failing_categories(&lint)));
    }
    let latest_artifact_id = snapshots
        .last()
        .map(|(artifact_id, _)| artifact_id.clone())
        .unwrap_or_default();
    let latest_categories = snapshots
        .last()
        .map(|(_, categories)| categories.clone())
        .unwrap_or_default();
    let mut regressions = Vec::new();
    for category in &latest_categories {
        let mut seen_before = false;
        let mut cleared_before_latest = false;
        for (_, categories) in snapshots.iter().take(snapshots.len().saturating_sub(1)) {
            if categories.iter().any(|candidate| candidate == category) {
                seen_before = true;
            } else if seen_before {
                cleared_before_latest = true;
            }
        }
        if seen_before && cleared_before_latest {
            regressions.push(json!({
                "category": category,
                "latest_artifact_id": latest_artifact_id,
                "reason": "category was previously cleared and reappeared in the latest artifact",
            }));
        }
    }
    let regression_count = regressions.len();
    Ok(json!({
        "task_id": task_id,
        "artifact_count": snapshots.len(),
        "ok": regression_count == 0,
        "regression_count": regression_count,
        "regressions": regressions,
        "summary": format!("handoff_lint_regressions count={regression_count}"),
    }))
}

fn read_handoff_payload(
    root: &Path,
    task_id: &str,
    artifact_id: &str,
    label: &str,
) -> Result<Value> {
    let path = task_version_json_path(root, task_id, artifact_id);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "failed to read stored {label} context artifact '{}'",
            path.display()
        )
    })?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn available_handoff_artifacts(payload: &Value) -> Vec<String> {
    let mut artifacts = Vec::new();
    if let Some(artifact_id) = payload.get("artifact_id").and_then(Value::as_str) {
        append_unique(&mut artifacts, artifact_id.to_string());
    }
    if let Some(ids) = payload
        .get("evidence_artifact_ids")
        .and_then(Value::as_array)
    {
        for id in ids.iter().filter_map(Value::as_str) {
            append_unique(&mut artifacts, id.to_string());
        }
    }
    artifacts
}

fn available_handoff_paths(payload: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(changed_paths) = payload
        .get("changed_paths_since_checkpoint")
        .and_then(Value::as_array)
    {
        for path in changed_paths.iter().filter_map(Value::as_str) {
            append_unique(&mut paths, path.to_string());
        }
    }
    paths
}

fn handoff_text_blocks(payload: &Value) -> Vec<String> {
    let mut blocks = Vec::new();
    if let Some(brief) = payload.get("brief").and_then(Value::as_str) {
        blocks.push(brief.to_string());
    }
    if let Some(next_action) = payload.get("next_action_summary").and_then(Value::as_str) {
        blocks.push(next_action.to_string());
    }
    if let Some(sections) = payload.get("sections").and_then(Value::as_array) {
        for section in sections {
            if let Some(body) = section.get("body").and_then(Value::as_str) {
                blocks.push(body.to_string());
            }
        }
    }
    blocks
}

fn referenced_handoff_artifacts(payload: &Value) -> Vec<String> {
    let mut references = Vec::new();
    collect_artifact_references(
        payload
            .get("brief")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        &mut references,
    );
    if let Some(sections) = payload.get("sections").and_then(Value::as_array) {
        for section in sections {
            collect_artifact_references(
                section
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                &mut references,
            );
        }
    }
    references
}

fn referenced_handoff_paths(payload: &Value) -> Vec<String> {
    let mut references = Vec::new();
    collect_path_references(
        payload
            .get("brief")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        &mut references,
    );
    if let Some(sections) = payload.get("sections").and_then(Value::as_array) {
        for section in sections {
            collect_path_references(
                section
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                &mut references,
            );
        }
    }
    references
}

fn referenced_handoff_commands(payload: &Value) -> Vec<String> {
    let mut commands = Vec::new();
    for block in handoff_text_blocks(payload) {
        for line in block.lines() {
            if let Some(command) = extract_test_command_reference(line) {
                append_unique(&mut commands, command);
            }
        }
    }
    commands
}

fn collect_artifact_references(text: &str, references: &mut Vec<String>) {
    for token in text.split_whitespace() {
        let token = token.trim_matches(|ch: char| {
            matches!(
                ch,
                '`' | '\'' | '"' | ',' | '.' | ';' | ':' | ')' | '(' | '[' | ']'
            )
        });
        if token.starts_with("artifact-") || token.starts_with("raw-") {
            append_unique(references, token.to_string());
        }
    }
}

fn collect_test_mentions(text: &str, tests: &mut Vec<String>) {
    for token in text.split_whitespace() {
        let token = clean_reference_token(token);
        if is_test_name_reference(token) {
            append_unique(tests, token.to_string());
        }
    }
}

fn collect_command_backed_tests(text: &str, tests: &mut Vec<String>) {
    for line in text.lines() {
        if contains_test_command(line) {
            collect_test_mentions(line, tests);
        }
    }
}

fn contains_test_command(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    line.contains("cargo test")
        || line.contains("cargo nextest")
        || line.contains("npm test")
        || line.contains("pnpm test")
        || line.contains("yarn test")
        || line.contains("bun test")
        || line.contains("pytest")
        || line.contains("go test")
        || line.contains("mvn test")
        || line.contains("gradle test")
}

fn extract_test_command_reference(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let markers = [
        "cargo test",
        "cargo nextest",
        "npm test",
        "pnpm test",
        "yarn test",
        "bun test",
        "pytest",
        "go test",
        "mvn test",
        "gradle test",
    ];
    let start = markers
        .iter()
        .filter_map(|marker| lower.find(marker))
        .min()?;
    let command = clean_command_reference(&line[start..]);
    (!command.is_empty()).then_some(command)
}

fn clean_command_reference(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch: char| matches!(ch, '`' | '\'' | '"' | ',' | '.' | ';' | ')' | ']'))
        .to_string()
}

fn command_executable(command: &str) -> Option<String> {
    command
        .split_whitespace()
        .find(|part| !part.contains('='))
        .map(clean_command_token)
        .filter(|part| !part.is_empty())
}

fn clean_command_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| matches!(ch, '`' | '\'' | '"' | ',' | '.' | ';' | ')' | ']'))
        .to_string()
}

fn executable_exists(executable: &str) -> bool {
    if executable.contains('/') {
        return Path::new(executable).exists();
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|path| path.join(executable).exists())
    })
}

fn command_env_vars(command: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let chars: Vec<char> = command.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '$' {
            index += 1;
            continue;
        }
        let start = index + 1;
        if start >= chars.len() || !is_env_var_start(chars[start]) {
            index += 1;
            continue;
        }
        let mut end = start + 1;
        while end < chars.len() && is_env_var_char(chars[end]) {
            end += 1;
        }
        append_unique(&mut vars, chars[start..end].iter().collect::<String>());
        index = end;
    }
    vars
}

fn handoff_lint_check(category: &str, payload: Value) -> Value {
    let ok = payload
        .get("ok")
        .and_then(Value::as_bool)
        .or_else(|| payload.get("ready").and_then(Value::as_bool))
        .unwrap_or(false);
    let issue_count = payload
        .get("issue_count")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            payload
                .get("missing")
                .and_then(Value::as_array)
                .map(|missing| missing.len() as u64)
                .unwrap_or_default()
        });
    let references = payload
        .get("issues")
        .and_then(Value::as_array)
        .map(|issues| {
            issues
                .iter()
                .filter_map(|issue| issue.get("reference").and_then(Value::as_str))
                .take(3)
                .map(|reference| json!(reference))
                .collect::<Vec<Value>>()
        })
        .or_else(|| {
            payload
                .get("missing")
                .and_then(Value::as_array)
                .map(|missing| {
                    missing
                        .iter()
                        .filter_map(Value::as_str)
                        .take(3)
                        .map(|reference| json!(reference))
                        .collect::<Vec<Value>>()
                })
        })
        .unwrap_or_default();
    json!({
        "category": category,
        "ok": ok,
        "issue_count": issue_count,
        "references": references,
    })
}

fn handoff_fix_actions_from_lint(lint: &Value) -> Vec<Value> {
    let mut actions = Vec::new();
    let Some(checks) = lint.get("checks").and_then(Value::as_array) else {
        return actions;
    };
    for check in checks {
        if check.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let category = check
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let references = check
            .get("references")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let first_reference = references
            .iter()
            .filter_map(Value::as_str)
            .next()
            .unwrap_or_default();
        let action = match category {
            "replay" => json!({
                "kind": "repair_handoff_packet",
                "reference": first_reference,
                "next": "regenerate handoff with objective, next action, debt signal, and evidence handle",
                "command": "Packet28 prepare_handoff",
            }),
            "dependencies" => json!({
                "kind": "attach_missing_artifact",
                "reference": first_reference,
                "next": "attach referenced artifact handle or remove the stale reference",
                "command": format!("packet28.fetch_tool_result handle={first_reference}"),
            }),
            "paths" => json!({
                "kind": "read_or_correct_path",
                "reference": first_reference,
                "next": "read the referenced path or correct the handoff path before replay",
                "command": format!("rg --files | rg '{}'", path_search_fragment(first_reference)),
            }),
            "tests" => json!({
                "kind": "add_test_command",
                "reference": first_reference,
                "next": "add or run a concrete command for the mentioned test",
                "command": format!("cargo test {first_reference}"),
            }),
            "stale_commands" => json!({
                "kind": "rerun_stale_command",
                "reference": first_reference,
                "next": "rerun the command after the latest relevant edit",
                "command": first_reference,
            }),
            "environment" => json!({
                "kind": "setup_environment",
                "reference": first_reference,
                "next": "set the missing variable or remove the command dependency",
                "command": format!("export {first_reference}=<value>"),
            }),
            _ => continue,
        };
        actions.push(action);
    }
    actions.truncate(6);
    actions
}

fn path_search_fragment(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).replace('\'', "")
}

fn lint_failing_categories(lint: &Value) -> Vec<String> {
    lint.get("failing_categories")
        .and_then(Value::as_array)
        .map(|categories| {
            categories
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn discover_handoff_artifact_ids(
    root: &Path,
    task_id: &str,
    max_artifacts: usize,
) -> Result<Vec<String>> {
    let probe = task_version_json_path(root, task_id, "__packet28_probe__");
    let Some(dir) = probe.parent() else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    if !dir.exists() {
        return Ok(ids);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
            ids.push(stem.to_string());
        }
    }
    ids.sort();
    if ids.len() > max_artifacts {
        ids = ids.split_off(ids.len() - max_artifacts);
    }
    Ok(ids)
}

fn lint_latest_category_contains(latest_categories: &[String], category: &str) -> bool {
    latest_categories.iter().any(|latest| latest == category)
}

fn is_env_var_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_uppercase()
}

fn is_env_var_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit()
}

fn is_test_name_reference(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    token.len() >= 6
        && (lower.starts_with("test_")
            || lower.ends_with("_test")
            || lower.ends_with("_tests")
            || lower.contains("::tests::")
            || lower.contains("test::"))
}

fn clean_reference_token(token: &str) -> &str {
    token.trim_matches(|ch: char| {
        matches!(
            ch,
            '`' | '\'' | '"' | ',' | '.' | ';' | ':' | ')' | '(' | '[' | ']'
        )
    })
}

fn collect_path_references(text: &str, references: &mut Vec<String>) {
    for token in text.split_whitespace() {
        let token = clean_reference_token(token);
        if is_repo_relative_path_reference(token) {
            append_unique(references, token.to_string());
        }
    }
}

fn is_repo_relative_path_reference(token: &str) -> bool {
    !token.starts_with('/')
        && !token.contains("://")
        && token.contains('/')
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-' | '.'))
        && token
            .rsplit('/')
            .next()
            .is_some_and(|name| name.contains('.'))
}

fn latest_relevant_edit_at(
    events: &[packet28_daemon_core::DaemonEventFrame],
    changed_paths: &[String],
) -> Option<u64> {
    events
        .iter()
        .filter(|frame| is_edit_event(frame, changed_paths))
        .map(|frame| frame.event.occurred_at_unix)
        .max()
}

fn latest_command_event_at(
    events: &[packet28_daemon_core::DaemonEventFrame],
    command_ref: &str,
) -> Option<u64> {
    events
        .iter()
        .filter(|frame| {
            frame
                .event
                .data
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| command == command_ref || command.contains(command_ref))
        })
        .map(|frame| frame.event.occurred_at_unix)
        .max()
}

fn is_edit_event(frame: &packet28_daemon_core::DaemonEventFrame, changed_paths: &[String]) -> bool {
    let kind = frame.event.kind.to_ascii_lowercase();
    if !kind.contains("edit") && !kind.contains("write") {
        return false;
    }
    if changed_paths.is_empty() {
        return true;
    }
    frame_event_paths(frame)
        .iter()
        .any(|path| changed_paths.iter().any(|changed| changed == path))
}

fn frame_event_paths(frame: &packet28_daemon_core::DaemonEventFrame) -> Vec<String> {
    frame
        .event
        .data
        .get("paths")
        .or_else(|| frame.event.data.get("changed_paths"))
        .and_then(Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn section_identifier(section: &Value) -> String {
    section
        .get("id")
        .or_else(|| section.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("section")
        .to_string()
}

fn is_replay_critical_section(section: &Value) -> bool {
    let id = section_identifier(section).to_ascii_lowercase();
    let title = section
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    id.contains("objective")
        || id.contains("next_action")
        || id.contains("context_debt")
        || id.contains("evidence_freshness")
        || title.contains("objective")
        || title.contains("next action")
        || title.contains("context debt")
        || title.contains("evidence freshness")
}

fn push_handoff_delta(deltas: &mut Vec<Value>, field: &str, left: String, right: String) {
    if left != right {
        deltas.push(json!({
            "field": field,
            "left": compact_handoff_text(&left),
            "right": compact_handoff_text(&right),
        }));
    }
}

fn compact_handoff_text(value: &str) -> String {
    let value = value.trim();
    let mut compact = String::new();
    for ch in value.chars().take(120) {
        compact.push(ch);
    }
    if value.chars().count() > 120 {
        compact.push_str("...");
    }
    compact
}

fn handoff_objective(payload: &Value) -> String {
    payload
        .get("brief")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .lines()
        .find(|line| !line.trim().is_empty() && !line.contains("Task Objective"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn handoff_next_action(payload: &Value) -> String {
    payload
        .get("next_action_summary")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            payload
                .get("latest_intention")
                .filter(|value| !value.is_null())
                .map(Value::to_string)
        })
        .unwrap_or_default()
}

fn handoff_evidence_handle_summary(payload: &Value) -> String {
    let artifact_id = payload
        .get("artifact_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let evidence_count = payload
        .get("evidence_artifact_ids")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    format!("artifact_id={artifact_id} evidence_count={evidence_count}")
}

fn handoff_debt_signal(payload: &Value) -> bool {
    section_exists(payload, "context_debt")
        || section_exists(payload, "evidence_freshness")
        || payload
            .get("changed_paths_since_checkpoint")
            .and_then(Value::as_array)
            .is_some_and(|paths| !paths.is_empty())
        || payload
            .get("open_questions")
            .and_then(Value::as_array)
            .is_some_and(|questions| !questions.is_empty())
}

fn section_exists(payload: &Value, id: &str) -> bool {
    payload
        .get("sections")
        .and_then(Value::as_array)
        .is_some_and(|sections| {
            sections
                .iter()
                .any(|section| section.get("id").and_then(Value::as_str) == Some(id))
        })
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

pub(crate) fn handle_packet28_validate_plan(
    root: &Path,
    args: Packet28ValidatePlanArgs,
) -> Result<Value> {
    if args.task_id.trim().is_empty() {
        return Err(anyhow!("packet28.validate_plan requires task_id"));
    }
    if args.steps.is_empty() {
        return Err(anyhow!("packet28.validate_plan requires at least one step"));
    }
    let response = crate::broker_client::validate_plan(
        root,
        BrokerValidatePlanRequest {
            task_id: args.task_id,
            steps: args.steps,
            require_read_before_edit: args.require_read_before_edit,
            require_test_gate: args.require_test_gate,
            budget_tokens: args.budget_tokens,
        },
    )?;
    Ok(serde_json::to_value(response)?)
}

pub(crate) fn handle_packet28_action_critic(
    root: &Path,
    args: Packet28ActionCriticArgs,
) -> Result<Value> {
    if args.task_id.trim().is_empty() {
        return Err(anyhow!("packet28.action_critic requires task_id"));
    }
    if !matches!(args.action, BrokerAction::ChooseTool | BrokerAction::Edit) {
        return Err(anyhow!(
            "packet28.action_critic action must be choose_tool or edit"
        ));
    }
    let response = crate::broker_client::get_context(
        root,
        packet28_daemon_core::BrokerGetContextRequest {
            task_id: args.task_id.clone(),
            action: Some(args.action),
            focus_paths: args.focus_paths,
            focus_symbols: args.focus_symbols,
            tool_name: args.tool_name,
            query: args.query,
            include_sections: vec!["action_critic".to_string()],
            max_sections: Some(1),
            default_max_items_per_section: Some(8),
            budget_tokens: args.budget_tokens,
            persist_artifacts: Some(false),
            ..packet28_daemon_core::BrokerGetContextRequest::default()
        },
    )?;
    let section = response
        .sections
        .iter()
        .find(|section| section.id == "action_critic");
    let warnings = section
        .map(|section| {
            section
                .body
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| line.trim_start_matches("- ").to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(json!({
        "task_id": args.task_id,
        "action": args.action,
        "context_version": response.context_version,
        "warning_count": warnings.len(),
        "warnings": warnings,
        "section": section,
    }))
}

pub(crate) fn handle_packet28_recommend_next_tool(
    root: &Path,
    args: Packet28RecommendNextToolArgs,
) -> Result<Value> {
    let records = crate::savings_analytics::load_run_savings(root, 200)?;
    let mut recommendations = Vec::new();
    if let Some(recommendation) = recommend_focus_refresh(&records, &args.focus_paths) {
        recommendations.push(recommendation);
    }
    if let Some(recommendation) = recommend_failure_fix(&records) {
        recommendations.push(recommendation);
    }
    if let Some(recommendation) = recommend_high_roi_route(&records) {
        recommendations.push(recommendation);
    }
    if recommendations.is_empty() {
        recommendations.push(default_context_recommendation(&args));
    }
    recommendations.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.expected_savings_tokens.cmp(&a.expected_savings_tokens))
            .then_with(|| a.command.cmp(&b.command))
    });
    recommendations.dedup_by(|a, b| a.command == b.command);
    let max_recommendations = args.max_recommendations.unwrap_or(2).clamp(1, 4);
    recommendations.truncate(max_recommendations);
    let token_estimate = recommendations
        .iter()
        .map(|recommendation| {
            recommendation.command.len()
                + recommendation.reason.len()
                + recommendation
                    .evidence
                    .iter()
                    .map(String::len)
                    .sum::<usize>()
                + recommendation.risk.len()
        })
        .sum::<usize>()
        .saturating_add(3)
        / 4;
    Ok(json!({
        "task_id": args.task_id,
        "recommendation_count": recommendations.len(),
        "token_estimate": token_estimate,
        "recommendations": recommendations.iter().map(|recommendation| {
            json!({
                "command": recommendation.command,
                "reason": recommendation.reason,
                "evidence": recommendation.evidence,
                "expected_savings_tokens": recommendation.expected_savings_tokens,
                "risk": recommendation.risk,
                "score": recommendation.score,
            })
        }).collect::<Vec<_>>(),
    }))
}

pub(crate) fn handle_packet28_validate_tool_outcome(
    root: &Path,
    args: Packet28ValidateToolOutcomeArgs,
) -> Result<Value> {
    let records = crate::savings_analytics::load_run_savings(root, 200)?;
    let command_filter = args
        .command
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let record = records
        .iter()
        .find(|record| command_filter.map_or(true, |needle| record.command.contains(needle)));
    let Some(record) = record else {
        return Ok(json!({
            "task_id": args.task_id,
            "status": "missing_artifact",
            "valid_success": false,
            "summary": "missing_artifact: no recorded Packet28 run-savings outcome matched the request",
            "next_action": "rerun the command through Packet28 or fetch the stored tool artifact before relying on it",
        }));
    };
    let changed_focus = args
        .focus_paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .find(|focus| {
            record
                .changed_paths
                .iter()
                .any(|changed| changed == focus || changed.starts_with(*focus))
        });
    let (status, valid_success, next_action) = if record.exit_code != 0 {
        (
            "failure",
            false,
            "inspect the failure summary or rerun after applying the likely fix",
        )
    } else if record
        .fallback_reason
        .as_deref()
        .is_some_and(|reason| !reason.trim().is_empty())
    {
        (
            "fallback",
            false,
            "treat the compact result as degraded and inspect fallback provenance before proceeding",
        )
    } else if changed_focus.is_some() {
        (
            "stale_artifact",
            false,
            "refresh focused path evidence before relying on this prior outcome",
        )
    } else {
        (
            "success",
            true,
            "safe to rely on this recorded successful outcome",
        )
    };
    let saved_tokens = record
        .raw_est_tokens
        .saturating_sub(record.reduced_est_tokens);
    let evidence = if let Some(reason) = record.fallback_reason.as_deref() {
        format!("fallback_reason={reason}")
    } else {
        format!(
            "exit_code={} saved_tokens={} savings_percent={:.1}",
            record.exit_code, saved_tokens, record.savings_percent
        )
    };
    Ok(json!({
        "task_id": args.task_id,
        "status": status,
        "valid_success": valid_success,
        "summary": format!("{status}: {}", record.command),
        "next_action": next_action,
        "command": record.command,
        "evidence": evidence,
        "changed_paths": record.changed_paths,
    }))
}

pub(crate) fn handle_packet28_patch_risk(
    root: &Path,
    args: Packet28PatchRiskArgs,
) -> Result<Value> {
    let paths = args
        .paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut score = 10_u64;
    let mut reasons = Vec::new();
    let mut required_checks = Vec::new();
    if paths.is_empty() {
        score = score.saturating_add(25);
        reasons.push("missing_patch_scope".to_string());
        required_checks.push("provide patch paths before editing".to_string());
    }
    let shared_paths = paths
        .iter()
        .filter(|path| patch_path_looks_shared(path))
        .count();
    if shared_paths > 0 {
        score = score.saturating_add((shared_paths as u64).saturating_mul(20).min(40));
        reasons.push(format!("shared_paths={shared_paths}"));
    }
    if paths.len() > 2 {
        score = score.saturating_add(((paths.len() - 2) as u64).saturating_mul(8).min(24));
        reasons.push(format!("multi_file_patch={}", paths.len()));
    }

    let testmap_path = root.join(".covy").join("state").join("testmap.bin");
    let testmap = testy_core::pipeline_testmap::load_testmap(&testmap_path).ok();
    if testmap.is_none() {
        score = score.saturating_add(15);
        reasons.push("missing_testmap".to_string());
        required_checks.push("run or refresh testmap before broad edits".to_string());
    }
    let mut missing_mappings = 0_usize;
    if let Some(testmap) = testmap.as_ref() {
        for path in &paths {
            if let Some(tests) = testmap.file_to_tests.get(path) {
                required_checks.extend(tests.iter().take(2).map(|test| format!("run {test}")));
            } else {
                missing_mappings += 1;
            }
        }
    }
    if missing_mappings > 0 {
        score = score.saturating_add((missing_mappings as u64).saturating_mul(15).min(30));
        reasons.push(format!("missing_testmap_mappings={missing_mappings}"));
        required_checks.push("run focused build/test fallback for unmapped paths".to_string());
    }

    let records = crate::savings_analytics::load_run_savings(root, 64)?;
    let recent_failures = records
        .iter()
        .filter(|record| record.exit_code != 0 || record.fallback_reason.is_some())
        .count();
    if recent_failures > 0 {
        score = score.saturating_add((recent_failures as u64).saturating_mul(10).min(30));
        reasons.push(format!("recent_failures_or_fallbacks={recent_failures}"));
    }
    score = score.min(100);
    required_checks.sort();
    required_checks.dedup();
    if required_checks.is_empty() {
        required_checks.push("run mapped focused tests".to_string());
    }
    let risk = if score >= 70 {
        "high"
    } else if score >= 40 {
        "medium"
    } else {
        "low"
    };
    Ok(json!({
        "task_id": args.task_id,
        "risk": risk,
        "score": score,
        "paths": paths,
        "reasons": reasons,
        "required_checks": required_checks,
    }))
}

fn patch_path_looks_shared(path: &str) -> bool {
    let path = path.trim();
    let basename = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path);
    matches!(
        basename,
        "lib.rs" | "main.rs" | "mod.rs" | "index.ts" | "index.tsx" | "package.json" | "Cargo.toml"
    ) || path.split('/').count() <= 2
}

fn recommend_focus_refresh(
    records: &[crate::savings_analytics::RunSavingsRecord],
    focus_paths: &[String],
) -> Option<ToolRecommendation> {
    let focus_paths = focus_paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    if focus_paths.is_empty() {
        return None;
    }
    let changed = records
        .iter()
        .flat_map(|record| &record.changed_paths)
        .find(|path| {
            focus_paths
                .iter()
                .any(|focus| path == focus || path.starts_with(focus))
        })?;
    Some(ToolRecommendation {
        command: format!("packet28.read_regions path={changed} regions=[]"),
        reason: "refresh focused path evidence before relying on cached context".to_string(),
        evidence: vec![format!("recent changed path matched focus: {changed}")],
        expected_savings_tokens: 0,
        risk: "stale_focus_evidence".to_string(),
        score: 95,
    })
}

fn recommend_failure_fix(
    records: &[crate::savings_analytics::RunSavingsRecord],
) -> Option<ToolRecommendation> {
    let failed = records.iter().find(|record| {
        record.exit_code != 0
            && record
                .failure_fingerprint
                .as_deref()
                .is_some_and(|fingerprint| !fingerprint.trim().is_empty())
    })?;
    let success = records.iter().find(|candidate| {
        candidate.cwd == failed.cwd
            && candidate.exit_code == 0
            && candidate.timestamp_unix_ms >= failed.timestamp_unix_ms
    })?;
    Some(ToolRecommendation {
        command: success.command.clone(),
        reason: "reuse the latest successful same-directory command after a matching failure"
            .to_string(),
        evidence: vec![
            format!(
                "failure_fingerprint={}",
                failed.failure_fingerprint.as_deref().unwrap_or_default()
            ),
            format!("success_after_failure={}", success.command),
        ],
        expected_savings_tokens: success
            .raw_est_tokens
            .saturating_sub(success.reduced_est_tokens),
        risk: "repeated_failure".to_string(),
        score: 90,
    })
}

fn recommend_high_roi_route(
    records: &[crate::savings_analytics::RunSavingsRecord],
) -> Option<ToolRecommendation> {
    let best = records
        .iter()
        .filter(|record| record.exit_code == 0 && record.fallback_reason.is_none())
        .max_by_key(|record| {
            record
                .raw_est_tokens
                .saturating_sub(record.reduced_est_tokens)
        })?;
    let saved = best.raw_est_tokens.saturating_sub(best.reduced_est_tokens);
    if saved == 0 {
        return None;
    }
    Some(ToolRecommendation {
        command: best.command.clone(),
        reason: format!("prefer high-ROI Packet28 route for {}", best.family),
        evidence: vec![format!(
            "saved_tokens={} savings_percent={:.1}",
            saved, best.savings_percent
        )],
        expected_savings_tokens: saved,
        risk: "low".to_string(),
        score: 70_u64.saturating_add((saved / 100).min(20)),
    })
}

fn default_context_recommendation(args: &Packet28RecommendNextToolArgs) -> ToolRecommendation {
    let query = args
        .query
        .as_deref()
        .filter(|query| !query.trim().is_empty())
        .unwrap_or("current task context");
    let focus = if args.focus_paths.is_empty() {
        String::new()
    } else {
        format!(" paths={}", args.focus_paths.join(","))
    };
    ToolRecommendation {
        command: format!("packet28.search query={query:?}{focus}"),
        reason: "start with compact Packet28 search because no local ROI or failure advice exists"
            .to_string(),
        evidence: vec!["no run-savings records available".to_string()],
        expected_savings_tokens: 0,
        risk: "unknown_roi".to_string(),
        score: if args.focus_symbols.is_empty() {
            50
        } else {
            55
        },
    }
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
    let full_payload =
        build_read_regions_full_payload(task_id, &invocation_id, sequence, &read_result);
    let artifact_id = Some(store_result_artifact(
        root,
        task_id,
        full_payload["invocation_id"].as_str().unwrap_or_default(),
        &full_payload,
    )?);
    let payload = build_read_regions_response_payload(&full_payload, &args, artifact_id.clone());
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

fn build_read_regions_full_payload(
    task_id: &str,
    invocation_id: &str,
    sequence: u64,
    read_result: &packet28_reducer_core::ReadRegionsResult,
) -> Value {
    json!({
        "task_id": task_id,
        "invocation_id": invocation_id,
        "sequence": sequence,
        "path": read_result.path,
        "regions": read_result.regions,
        "symbols": read_result.symbols,
        "content": render_read_region_content(&read_result.lines),
        "line_count": read_result.lines.len(),
        "compact_preview": read_result.compact_preview,
        "response_mode": "full",
    })
}

fn build_read_regions_response_payload(
    full_payload: &Value,
    args: &Packet28ReadRegionsArgs,
    artifact_id: Option<String>,
) -> Value {
    match args.response_mode {
        Packet28SearchResponseMode::Full => {
            let mut payload = full_payload.clone();
            payload["artifact_id"] = json!(artifact_id);
            payload
        }
        Packet28SearchResponseMode::Slim => {
            let mut payload = json!({
                "path": full_payload["path"].clone(),
                "regions": full_payload["regions"].clone(),
                "symbols": full_payload["symbols"].clone(),
                "line_count": full_payload["line_count"].clone(),
                "compact_preview": full_payload["compact_preview"].clone(),
                "artifact_id": artifact_id,
                "response_mode": "slim",
            });
            if read_regions_request_is_explicit(args) {
                payload["content"] = full_payload["content"].clone();
            }
            payload
        }
    }
}

fn read_regions_request_is_explicit(args: &Packet28ReadRegionsArgs) -> bool {
    !args.regions.is_empty() || args.line_start.is_some() || args.line_end.is_some()
}

fn render_read_region_content(lines: &[packet28_reducer_core::ReadLine]) -> String {
    lines
        .iter()
        .map(|item| format!("{}: {}", item.line, item.text))
        .collect::<Vec<_>>()
        .join("\n")
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
    fn fff_search_strategy_deserializes_for_fast_search() {
        let args: Packet28SearchFastArgs = serde_json::from_value(json!({
            "query": "SearchResult",
            "search_strategy": "fff"
        }))
        .unwrap();

        assert_eq!(args.search_strategy.as_str(), "fff");
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

    #[test]
    fn fetched_search_artifact_uses_text_content_instead_of_match_objects() {
        let result = sample_result("indexed_regex", "src/alpha.rs", 4, "struct Alpha;");
        let execution = Packet28SearchExecution {
            strategy: Packet28SearchStrategy::Hybrid,
            primary_backend: "indexed_regex".to_string(),
            secondary_backend: Some("legacy_rg".to_string()),
            shadowed: true,
            added_displayed_matches: 0,
            added_paths: 0,
            notes: Vec::new(),
        };
        let mut payload = build_search_full_payload(&result, &execution);
        payload["artifact_id"] = json!("artifact-search");

        compact_fetched_tool_result_payload(&mut payload);

        assert_eq!(payload["response_mode"], "full");
        assert_eq!(payload["artifact_id"], "artifact-search");
        assert_eq!(payload["content"], "src/alpha.rs:4:struct Alpha;");
        assert_eq!(payload["content_format"], "path:line:text");
        assert_eq!(payload["line_count"], 1);
        assert_eq!(payload["regions"][0], "src/alpha.rs:4-4");
        assert!(payload.get("groups").is_none());
    }

    #[test]
    fn read_regions_slim_includes_explicit_content_and_full_uses_text_payload() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("sample.rs"),
            "fn alpha() {}\nfn beta() {}\nfn gamma() {}\n",
        )
        .unwrap();
        let args = Packet28ReadRegionsArgs {
            task_id: "task-read".to_string(),
            path: "sample.rs".to_string(),
            regions: vec!["sample.rs:1-3".to_string()],
            response_mode: Packet28SearchResponseMode::Slim,
            ..Packet28ReadRegionsArgs::default()
        };
        let read_result = packet28_reducer_core::read_regions(
            dir.path(),
            &packet28_reducer_core::ReadRegionsRequest {
                path: args.path.clone(),
                regions: args.regions.clone(),
                ..packet28_reducer_core::ReadRegionsRequest::default()
            },
        )
        .unwrap();
        let full_payload = build_read_regions_full_payload("task-read", "tool-1", 1, &read_result);
        let payload = build_read_regions_response_payload(
            &full_payload,
            &args,
            Some("artifact-1".to_string()),
        );

        assert_eq!(payload["response_mode"], "slim");
        assert_eq!(payload["line_count"], 3);
        assert_eq!(
            payload["content"],
            "1: fn alpha() {}\n2: fn beta() {}\n3: fn gamma() {}"
        );
        assert!(payload.get("lines").is_none());

        assert_eq!(full_payload["response_mode"], "full");
        assert_eq!(
            full_payload["content"],
            "1: fn alpha() {}\n2: fn beta() {}\n3: fn gamma() {}"
        );
        assert!(full_payload.get("lines").is_none());
    }
}
