use super::*;
use crate::cmd_mcp::support::{
    next_task_invocation, packet28_search_via_session, packet28_search_via_session_with_force,
    store_result_artifact, write_auto_capture_state_batch_via_session,
};

#[path = "cmd_mcp_native_args.rs"]
mod args;
#[path = "cmd_mcp_native_artifacts.rs"]
mod artifacts;
#[path = "cmd_mcp_native_fff.rs"]
mod fff;
#[path = "cmd_mcp_native_handoff.rs"]
mod handoff;
#[path = "cmd_mcp_native_read.rs"]
mod read;
#[path = "cmd_mcp_native_search.rs"]
mod search;

pub(crate) use args::{
    Packet28ActionCriticArgs, Packet28FetchContextArgs, Packet28FetchRawOutputArgs,
    Packet28FetchToolResultArgs, Packet28GlobArgs, Packet28HandoffCompressionArgs,
    Packet28HandoffDependencyLintArgs, Packet28HandoffDiffArgs, Packet28HandoffEnvironmentLintArgs,
    Packet28HandoffFixPlanArgs, Packet28HandoffLintAllArgs, Packet28HandoffLintRegressionArgs,
    Packet28HandoffLintTrendArgs, Packet28HandoffPathLintArgs, Packet28HandoffRepairVerifyArgs,
    Packet28HandoffStaleCommandLintArgs, Packet28HandoffTestLintArgs, Packet28PatchRiskArgs,
    Packet28PrepareHandoffArgs, Packet28PromptPressureArgs, Packet28ReadRegionsArgs,
    Packet28RecommendNextToolArgs, Packet28SearchArgs, Packet28SearchFastArgs,
    Packet28SearchResponseMode, Packet28SearchStrategy, Packet28ValidatePlanArgs,
    Packet28ValidateToolOutcomeArgs, Packet28VerifyHandoffArgs,
};

#[cfg(test)]
use artifacts::compact_fetched_tool_result_payload;
pub(crate) use artifacts::{
    handle_packet28_fetch_context, handle_packet28_fetch_raw_output,
    handle_packet28_fetch_tool_result,
};
pub(crate) use handoff::{
    handle_packet28_handoff_compress, handle_packet28_handoff_diff,
    handle_packet28_handoff_fix_plan, handle_packet28_handoff_lint_all,
    handle_packet28_handoff_lint_dependencies, handle_packet28_handoff_lint_environment,
    handle_packet28_handoff_lint_paths, handle_packet28_handoff_lint_regressions,
    handle_packet28_handoff_lint_stale_commands, handle_packet28_handoff_lint_tests,
    handle_packet28_handoff_lint_trends, handle_packet28_handoff_repair_verify,
    handle_packet28_prepare_handoff, handle_packet28_prompt_pressure,
    handle_packet28_verify_handoff,
};
#[cfg(test)]
use read::{build_read_regions_full_payload, build_read_regions_response_payload};
pub(crate) use read::{handle_packet28_glob, handle_packet28_read_regions};
#[cfg(test)]
use search::build_search_slim_payload;
use search::{
    build_search_full_payload, build_search_request, build_search_response_payload,
    merge_search_results, search_backend_name, should_shadow_with_native, Packet28SearchExecution,
};

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
        .find(|record| command_filter.is_none_or(|needle| record.command.contains(needle)));
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
