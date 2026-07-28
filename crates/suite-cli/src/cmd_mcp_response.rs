use serde_json::{json, Value};

pub(super) fn capabilities_payload() -> Value {
    // Keep this payload minimal — it is injected into every MCP init and
    // counts against the agent's context budget.  Only include fields the
    // agent needs to *decide what to call*; omit anything derivable from
    // tool schemas or MCP protocol defaults.
    json!({
        "response_modes": ["slim", "full"],
        "hooks_first": true,
        "push_notification": "notifications/packet28.context_updated",
        "task_id_optional_after_first": true,
        "relaunch": "daemon_managed",
        "supersession": "replace"
    })
}

pub(super) fn summarize_tool_payload(name: &str, payload: &Value) -> String {
    match name {
        "packet28.search" | "packet28.search_fast" | "packet28.read_regions" | "packet28.glob" => {
            payload
                .get("compact_preview")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "Packet28 compact tool result.".to_string())
        }
        "packet28.fetch_tool_result" => {
            let artifact_id = payload
                .get("artifact_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 fetched tool artifact {artifact_id}.")
        }
        "packet28.fetch_raw_output" => {
            let path = payload
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 fetched raw output from {path}.")
        }
        "packet28.fetch_context" => {
            let artifact_id = payload
                .get("artifact_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 fetched broker context artifact {artifact_id}.")
        }
        "packet28.verify_handoff" => {
            let ready = payload
                .get("ready")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let score = payload
                .get("score")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 handoff replay ready={ready} score={score}.")
        }
        "packet28.prompt_pressure" => {
            let pressure = payload
                .get("pressure")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let remaining = payload
                .get("remaining_tokens")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 prompt pressure={pressure} remaining_tokens={remaining}.")
        }
        "packet28.handoff_diff" => {
            let delta_count = payload
                .get("delta_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let top_delta = payload
                .get("top_delta")
                .and_then(Value::as_str)
                .unwrap_or("none");
            format!("Packet28 handoff diff delta_count={delta_count} top_delta={top_delta}.")
        }
        "packet28.handoff_compress" => {
            let recommendation_count = payload
                .get("recommendations")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            let projected_over_budget = payload
                .get("projected_over_budget")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            format!("Packet28 handoff compression recommendations={recommendation_count} projected_over_budget={projected_over_budget}.")
        }
        "packet28.handoff_lint_dependencies" => {
            let issue_count = payload
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 handoff dependency lint issue_count={issue_count}.")
        }
        "packet28.handoff_lint_paths" => {
            let issue_count = payload
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 handoff path lint issue_count={issue_count}.")
        }
        "packet28.handoff_lint_tests" => {
            let issue_count = payload
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 handoff test lint issue_count={issue_count}.")
        }
        "packet28.handoff_lint_stale_commands" => {
            let issue_count = payload
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 handoff stale-command lint issue_count={issue_count}.")
        }
        "packet28.handoff_lint_environment" => {
            let issue_count = payload
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 handoff environment lint issue_count={issue_count}.")
        }
        "packet28.handoff_lint_all" => {
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let issue_count = payload
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 handoff lint aggregate status={status} issue_count={issue_count}.")
        }
        "packet28.handoff_fix_plan" => {
            let action_count = payload
                .get("action_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 handoff fix plan action_count={action_count}.")
        }
        "packet28.handoff_repair_verify" => {
            let verified = payload
                .get("verified")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            format!("Packet28 handoff repair verified={verified}.")
        }
        "packet28.handoff_lint_trends" => {
            let artifact_count = payload
                .get("artifact_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 handoff lint trends artifacts={artifact_count}.")
        }
        "packet28.handoff_lint_regressions" => {
            let regression_count = payload
                .get("regression_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 handoff lint regressions count={regression_count}.")
        }
        "packet28.prepare_handoff" | "packet28.handoff" => {
            let ready = payload
                .get("handoff_ready")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let reason = payload
                .get("handoff_reason")
                .and_then(Value::as_str)
                .unwrap_or("handoff prepared");
            if ready {
                format!("Packet28 prepared a handoff: {reason}")
            } else {
                format!("Packet28 did not prepare a handoff: {reason}")
            }
        }
        "packet28.validate_plan" => {
            let valid = payload
                .get("valid")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let violations = payload
                .get("violations")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            let warnings = payload
                .get("warnings")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            format!("Packet28 plan validation valid={valid} violations={violations} warnings={warnings}.")
        }
        "packet28.action_critic" => {
            let warning_count = payload
                .get("warnings")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            format!("Packet28 action critic returned {warning_count} warning(s).")
        }
        "packet28.recommend_next_tool" => {
            let recommendation_count = payload
                .get("recommendations")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            let token_estimate = payload
                .get("token_estimate")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!(
                "Packet28 recommended {recommendation_count} next tool(s), estimated {token_estimate} tokens."
            )
        }
        "packet28.validate_tool_outcome" => {
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let valid = payload
                .get("valid_success")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            format!("Packet28 tool outcome status={status} valid_success={valid}.")
        }
        "packet28.patch_risk" => {
            let risk = payload
                .get("risk")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let score = payload
                .get("score")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 patch risk={risk} score={score}.")
        }
        "packet28.verify_experiments" => {
            let ok = payload.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let experiments = payload
                .get("experiment_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let issues = payload
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!(
                "Packet28 experiment manifest ok={ok} experiments={experiments} issues={issues}."
            )
        }
        "packet28.reducer_drift" => {
            let ok = payload.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let cases = payload
                .get("case_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let issues = payload
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 reducer drift ok={ok} cases={cases} issues={issues}.")
        }
        "packet28.hypothesis_add" => {
            let id = payload
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 recorded active hypothesis {id}.")
        }
        "packet28.hypothesis_list" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 listed {count} active hypothesis/hypotheses.")
        }
        "packet28.hypothesis_resolve" => {
            let id = payload
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("resolved");
            format!("Packet28 marked hypothesis {id} {status}.")
        }
        "packet28.reduce" => "Packet28 command reduction.".to_string(),
        "packet28.rewrite" => {
            let route = payload
                .get("route")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 rewrite route: {route}.")
        }
        "packet28.doctor" => "Packet28 doctor report.".to_string(),
        "packet28.memory_store" => {
            let id = payload
                .get("id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 stored memory {id}.")
        }
        "packet28.memory_recall" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 recalled {count} memor(y/ies).")
        }
        "packet28.memory_list" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 listed {count} memor(y/ies).")
        }
        "packet28.memory_update" => {
            let id = payload
                .get("id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 updated memory {id}.")
        }
        "packet28.memory_forget" => {
            let deleted = payload
                .get("deleted")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 deleted {deleted} memor(y/ies).")
        }
        "packet28.memory_topics" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 listed {count} memory topic(s).")
        }
        "packet28.memory_stats" => "Packet28 memory statistics.".to_string(),
        "packet28.memory_health" => {
            let total = payload
                .get("total_memories")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let needs = payload
                .get("topics_needing_consolidation")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!(
                "Packet28 memory health: {total} memories, {needs} topic(s) need consolidation."
            )
        }
        "packet28.memory_lint" => {
            let issues = payload
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let memories = payload
                .get("memory_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 memory lint memories={memories} issues={issues}.")
        }
        "packet28.memory_consolidate" => {
            let count = payload
                .get("source_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 memory consolidation {status} from {count} source memor(y/ies).")
        }
        "packet28.memory_decay" => {
            let count = payload
                .get("decayed_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 decayed {count} memor(y/ies).")
        }
        "packet28.memory_prune" => {
            let deleted = payload
                .get("deleted_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let candidates = payload
                .get("candidate_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 pruned {deleted} of {candidates} candidate memor(y/ies).")
        }
        "packet28.memory_embed" => {
            let count = payload
                .get("embedded_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 embedded {count} memor(y/ies).")
        }
        "packet28.memory_extract_patterns" => {
            let count = payload
                .get("pattern_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 extracted {count} memory pattern(s).")
        }
        "packet28.feedback_record" => {
            let id = payload
                .get("id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 recorded feedback {id}.")
        }
        "packet28.feedback_search" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 found {count} feedback correction(s).")
        }
        "packet28.feedback_list" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 listed {count} feedback correction(s).")
        }
        "packet28.feedback_apply" => {
            let count = payload
                .get("applied_count")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 feedback applied count is now {count}.")
        }
        "packet28.feedback_delete" => {
            let deleted = payload
                .get("deleted")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 deleted {deleted} feedback correction(s).")
        }
        "packet28.feedback_stats" => "Packet28 feedback statistics.".to_string(),
        "packet28.wakeup" => "Packet28 wake-up pack.".to_string(),
        "packet28.learn_project" => {
            let concepts = payload
                .get("total_concepts")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let links = payload
                .get("link_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 learned project graph: {concepts} concepts, {links} links.")
        }
        "packet28.transcript_append" => {
            let id = payload
                .get("id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            format!("Packet28 appended transcript message {id}.")
        }
        "packet28.transcript_list" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 listed {count} transcript session(s).")
        }
        "packet28.transcript_show" | "packet28.transcript_search" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 returned {count} transcript message(s).")
        }
        "packet28.transcript_stats" => "Packet28 transcript statistics.".to_string(),
        "packet28.transcript_export" => {
            let count = payload
                .get("messages")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            format!("Packet28 exported {count} transcript message(s).")
        }
        "packet28.transcript_import" => {
            let count = payload
                .get("imported_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 imported {count} transcript message(s).")
        }
        "packet28.graph_create" => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("default");
            format!("Packet28 graph memoir: {name}.")
        }
        "packet28.graph_list" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 returned {count} graph memoir(s).")
        }
        "packet28.graph_show" => {
            let name = payload
                .get("memoir")
                .and_then(|memoir| memoir.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("default");
            format!("Packet28 graph memoir detail: {name}.")
        }
        "packet28.graph_add_concept" | "packet28.graph_refine" => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("concept");
            format!("Packet28 graph concept: {name}.")
        }
        "packet28.graph_link" => "Packet28 graph relation recorded.".to_string(),
        "packet28.graph_search" => {
            let count = payload.as_array().map(Vec::len).unwrap_or_default();
            format!("Packet28 found {count} graph concept(s).")
        }
        "packet28.graph_export" => {
            let format = payload
                .get("format")
                .and_then(Value::as_str)
                .unwrap_or("json");
            format!("Packet28 graph exported as {format}.")
        }
        "packet28.graph_stats" => "Packet28 graph statistics.".to_string(),
        "packet28.graph_delete" => {
            let deleted = payload
                .get("deleted_concepts")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 deleted {deleted} graph concept(s).")
        }
        "packet28.graph_inspect" => "Packet28 graph inspection.".to_string(),
        "packet28.graph_inspect_concept" => {
            let name = payload
                .get("concept")
                .and_then(|concept| concept.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("concept");
            format!("Packet28 graph concept inspection: {name}.")
        }
        "packet28.graph_distill" => {
            let created = payload
                .get("created_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let refined = payload
                .get("refined_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Packet28 distilled graph concepts: {created} created, {refined} refined.")
        }
        "packet28.task_status" => "Packet28 task status.".to_string(),
        "packet28.agent_status" => {
            let policy = payload
                .get("reducer_cache_safety")
                .and_then(|value| value.get("policy"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Packet28 agent status cache_policy={policy}.")
        }
        "packet28.capabilities" => "Packet28 broker capabilities.".to_string(),
        _ => "Packet28 response.".to_string(),
    }
}
