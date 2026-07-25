use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Args;
use packet28_daemon_core::{load_task_registry, task_artifact_dir};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::memory_store::{
    feedback_stats, graph_stats, list_memories, local_store_stats, memory_health, memory_topics,
    transcript_stats, FeedbackStats, GraphStats, MemoryHealthReport, MemoryTopicStats,
    TranscriptStats,
};
use crate::savings_analytics::load_run_savings;

#[path = "cmd_dashboard_render.rs"]
mod render;
use render::{
    context_hidden_sample_summary, render_dashboard_html, render_dashboard_tui, run_dashboard_tui,
    DashboardPanel,
};

const MAX_CONTEXT_ANOMALIES: usize = 3;

#[derive(Args)]
pub struct DashboardArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    /// Output format: text, json, or html
    #[arg(long, default_value = "text")]
    pub format: String,
    /// Write rendered dashboard output to this path
    #[arg(long)]
    pub output: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    /// Keep the terminal dashboard open and accept navigation commands on stdin
    #[arg(long)]
    pub interactive: bool,
    /// Read context anomaly history from this JSONL file instead of .packet28 history
    #[arg(long, value_name = "PATH")]
    pub context_anomaly_history: Option<String>,
}

#[derive(Debug, Serialize)]
struct DashboardReport {
    token_savings: TokenSavings,
    commands_reduced: usize,
    sessions: usize,
    top_saved_routes: Vec<DashboardRouteRoi>,
    top_noisy_commands: Vec<String>,
    missed_savings: Vec<String>,
    memory_count: i64,
    recent_memories: Vec<String>,
    memory_topics: Vec<MemoryTopicStats>,
    memory_health: MemoryHealthReport,
    graph_concepts: usize,
    graph_relations: usize,
    graph_stats: GraphStats,
    feedback_corrections: i64,
    feedback_stats: FeedbackStats,
    transcript_stats: TranscriptStats,
    mcp_call_history: i64,
    hook_event_history: i64,
    pending_extractions: i64,
    integration_health: BTreeMap<String, String>,
    windsurf_doctor_status: String,
    handoff_readiness: HandoffReadinessTile,
    reducer_drift: ReducerDriftTile,
    memory_lint: MemoryLintTile,
    context_anomalies: ContextAnomalyTile,
}

#[derive(Debug, Serialize, Default)]
pub(crate) struct ContextAnomalyDigest {
    pub(crate) anomaly_count: usize,
    pub(crate) truncated_count: usize,
    pub(crate) hidden_categories: Vec<String>,
    #[serde(skip_serializing)]
    pub(crate) hidden_samples: Vec<ContextHiddenSample>,
    pub(crate) anomalies: Vec<ContextAnomaly>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub(crate) struct ContextHiddenSample {
    pub(crate) category: String,
    pub(crate) signal: String,
}

#[derive(Debug, Serialize, Default)]
pub(crate) struct ContextAnomaly {
    pub(crate) category: String,
    pub(crate) severity: String,
    pub(crate) signal: String,
    pub(crate) next_check: String,
    pub(crate) repair_hint: String,
}

#[derive(Debug, Serialize, Default)]
struct TokenSavings {
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
    saved_est_tokens: u64,
    savings_percent: f64,
}

#[derive(Debug, Serialize, Default, Clone)]
struct DashboardRouteRoi {
    route: String,
    invocation_count: usize,
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
    saved_est_tokens: u64,
    savings_percent: f64,
}

#[derive(Debug, Serialize, Default)]
struct HandoffReadinessTile {
    artifact_count: usize,
    latest_status: String,
    latest_blocking_categories: Vec<String>,
    recurring_categories: Vec<String>,
    regression_count: usize,
}

#[derive(Debug, Serialize, Default)]
struct ReducerDriftTile {
    run_count: usize,
    latest_status: String,
    latest_issue_count: usize,
    latest_failing_families: Vec<String>,
    recurring_issue_kinds: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct ReducerDriftHistoryRecord {
    created_at_unix_ms: i64,
    ok: bool,
    case_count: u64,
    issue_count: u64,
    failing_families: Vec<String>,
    issue_kinds: Vec<String>,
}

#[derive(Debug, Serialize, Default)]
struct MemoryLintTile {
    run_count: usize,
    latest_status: String,
    latest_issue_count: usize,
    latest_issue_kinds: Vec<String>,
    recurring_issue_kinds: Vec<String>,
}

#[derive(Debug, Serialize, Default)]
struct ContextAnomalyTile {
    run_count: usize,
    latest_status: String,
    latest_anomaly_count: usize,
    latest_high_count: usize,
    latest_age_ms: u64,
    oldest_recurring_hidden_age_ms: u64,
    latest_hidden_categories: Vec<String>,
    recurring_hidden_categories: Vec<String>,
    recurring_hidden_samples: Vec<ContextHiddenSample>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct MemoryLintHistoryRecord {
    created_at_unix_ms: i64,
    ok: bool,
    memory_count: u64,
    issue_count: u64,
    issue_kinds: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(default)]
struct ContextAnomalyHistoryRecord {
    created_at_unix_ms: i64,
    ok: bool,
    anomaly_count: u64,
    high_count: u64,
    hidden_categories: Vec<String>,
    hidden_samples: Vec<ContextHiddenSample>,
}

pub fn run(args: DashboardArgs) -> Result<i32> {
    let root = crate::cmd_daemon::resolve_root_arg(&args.root);
    let cwd = crate::cmd_common::caller_cwd()?;
    let context_anomaly_history_path = args
        .context_anomaly_history
        .as_deref()
        .map(|path| PathBuf::from(crate::cmd_common::resolve_path_from_cwd(path, &cwd)));
    let savings = load_run_savings(&root, 100)?;
    let mut token_savings = TokenSavings::default();
    let mut top_noisy_commands = Vec::new();
    let mut missed_savings = Vec::new();
    let mut route_roi = BTreeMap::<String, DashboardRouteRoi>::new();
    for record in &savings {
        token_savings.raw_est_tokens += record.raw_est_tokens;
        token_savings.reduced_est_tokens += record.reduced_est_tokens;
        record_dashboard_route_roi(&mut route_roi, record);
        if record.raw_est_tokens > 0 {
            top_noisy_commands.push(record.command.clone());
        }
        if record.fallback_reason.is_some() || record.savings_percent < 10.0 {
            missed_savings.push(record.command.clone());
        }
    }
    token_savings.saved_est_tokens = token_savings
        .raw_est_tokens
        .saturating_sub(token_savings.reduced_est_tokens);
    token_savings.savings_percent = if token_savings.raw_est_tokens == 0 {
        0.0
    } else {
        (token_savings.saved_est_tokens as f64 / token_savings.raw_est_tokens as f64) * 100.0
    };
    top_noisy_commands.truncate(5);
    missed_savings.truncate(5);
    let mut top_saved_routes = route_roi.into_values().collect::<Vec<_>>();
    top_saved_routes.sort_by(|a, b| {
        b.saved_est_tokens
            .cmp(&a.saved_est_tokens)
            .then_with(|| a.route.cmp(&b.route))
    });
    top_saved_routes.truncate(5);

    let sessions = load_task_registry(&root)
        .map(|registry| registry.tasks.len())
        .unwrap_or_default();
    let store_stats = local_store_stats()?;
    let recent_memories = list_memories(5)?
        .into_iter()
        .map(|memory| memory.content)
        .collect::<Vec<_>>();
    let topics = memory_topics()?;
    let health = memory_health(None, 30, 10)?;
    let graph = graph_stats()?;
    let feedback = feedback_stats()?;
    let transcripts = transcript_stats()?;
    let handoff_readiness = handoff_readiness_tile(&root)?;
    let reducer_drift = reducer_drift_tile(&root)?;
    let memory_lint = memory_lint_tile(&root)?;
    let context_anomalies = context_anomaly_tile(&root, context_anomaly_history_path.as_deref())?;
    let windsurf_rules = root.join(".windsurf").join("rules").join("packet28.md");
    let windsurf_status = if windsurf_rules.exists() {
        "rules_present"
    } else {
        "rules_missing"
    };
    let mut integration_health = BTreeMap::new();
    integration_health.insert("windsurf".to_string(), windsurf_status.to_string());
    integration_health.insert("local_dashboard".to_string(), "ok".to_string());

    let report = DashboardReport {
        token_savings,
        commands_reduced: savings
            .iter()
            .filter(|record| record.fallback_reason.is_none())
            .count(),
        sessions,
        top_saved_routes,
        top_noisy_commands,
        missed_savings,
        memory_count: store_stats.memory_count,
        recent_memories,
        memory_topics: topics,
        memory_health: health,
        graph_concepts: graph.concept_count as usize,
        graph_relations: graph.relation_count as usize,
        graph_stats: graph,
        feedback_corrections: feedback.feedback_count,
        feedback_stats: feedback,
        transcript_stats: transcripts,
        mcp_call_history: store_stats.mcp_call_count,
        hook_event_history: store_stats.hook_event_count,
        pending_extractions: store_stats.pending_extraction_count,
        integration_health,
        windsurf_doctor_status: windsurf_status.to_string(),
        handoff_readiness,
        reducer_drift,
        memory_lint,
        context_anomalies,
    };

    let format = args.format.trim().to_ascii_lowercase();
    if args.json || format == "json" {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else if format == "html" {
        let html = render_dashboard_html(&report);
        if let Some(path) = args.output.as_deref() {
            fs::write(path, html)?;
            println!("dashboard_html={path}");
        } else {
            print!("{html}");
        }
    } else if format == "tui" {
        if args.interactive {
            run_dashboard_tui(&report)?;
        } else {
            print!(
                "{}",
                render_dashboard_tui(&report, DashboardPanel::Overview)
            );
        }
    } else {
        println!(
            "token_savings.saved_est_tokens={}",
            report.token_savings.saved_est_tokens
        );
        println!("commands_reduced={}", report.commands_reduced);
        println!("top_saved_routes={}", report.top_saved_routes.len());
        println!("sessions={}", report.sessions);
        println!("memory_count={}", report.memory_count);
        println!("memory_topics={}", report.memory_topics.len());
        println!(
            "topics_needing_consolidation={}",
            report.memory_health.topics_needing_consolidation
        );
        println!("graph_concepts={}", report.graph_concepts);
        println!("graph_relations={}", report.graph_relations);
        println!("feedback_corrections={}", report.feedback_corrections);
        println!(
            "transcript_messages={}",
            report.transcript_stats.message_count
        );
        println!("mcp_call_history={}", report.mcp_call_history);
        println!("hook_event_history={}", report.hook_event_history);
        println!("pending_extractions={}", report.pending_extractions);
        println!("windsurf_doctor_status={}", report.windsurf_doctor_status);
        println!(
            "handoff_latest_status={}",
            report.handoff_readiness.latest_status
        );
        println!(
            "handoff_regression_count={}",
            report.handoff_readiness.regression_count
        );
        println!(
            "reducer_drift_latest_status={}",
            report.reducer_drift.latest_status
        );
        println!(
            "reducer_drift_latest_issue_count={}",
            report.reducer_drift.latest_issue_count
        );
        println!(
            "memory_lint_latest_status={}",
            report.memory_lint.latest_status
        );
        println!(
            "memory_lint_latest_issue_count={}",
            report.memory_lint.latest_issue_count
        );
        println!(
            "context_anomaly_latest_status={}",
            report.context_anomalies.latest_status
        );
        println!(
            "context_anomaly_latest_high_count={}",
            report.context_anomalies.latest_high_count
        );
        println!(
            "context_anomaly_latest_age_ms={}",
            report.context_anomalies.latest_age_ms
        );
        println!(
            "context_anomaly_oldest_recurring_hidden_age_ms={}",
            report.context_anomalies.oldest_recurring_hidden_age_ms
        );
        println!(
            "context_anomaly_recurring_hidden_samples={}",
            context_hidden_sample_summary(&report.context_anomalies.recurring_hidden_samples)
        );
    }
    Ok(0)
}

pub(crate) fn handoff_readiness_payload(root: &Path) -> Result<Value> {
    Ok(serde_json::to_value(handoff_readiness_tile(root)?)?)
}

pub(crate) fn record_reducer_drift_history(root: &Path, payload: &Value) -> Result<()> {
    let record = reducer_drift_history_record(payload);
    let path = reducer_drift_history_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(&record)?;
    line.push('\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

pub(crate) fn record_memory_lint_history(root: &Path, payload: &Value) -> Result<()> {
    let record = memory_lint_history_record(payload);
    let path = memory_lint_history_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(&record)?;
    line.push('\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

pub(crate) fn record_context_anomaly_history(root: &Path, payload: &Value) -> Result<()> {
    let record = context_anomaly_history_record(payload);
    let path = context_anomaly_history_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(&record)?;
    line.push('\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

fn reducer_drift_history_record(payload: &Value) -> ReducerDriftHistoryRecord {
    let mut case_family = BTreeMap::<String, String>::new();
    if let Some(summaries) = payload.get("summaries").and_then(Value::as_array) {
        for summary in summaries {
            if let (Some(case_id), Some(family)) = (
                summary.get("case_id").and_then(Value::as_str),
                summary.get("family").and_then(Value::as_str),
            ) {
                case_family.insert(case_id.to_string(), family.to_string());
            }
        }
    }
    let mut failing_families = BTreeSet::<String>::new();
    let mut issue_kinds = BTreeSet::<String>::new();
    if let Some(issues) = payload.get("issues").and_then(Value::as_array) {
        for issue in issues {
            if let Some(kind) = issue.get("kind").and_then(Value::as_str) {
                issue_kinds.insert(kind.to_string());
            }
            if let Some(case_id) = issue.get("case_id").and_then(Value::as_str) {
                if let Some(family) = case_family.get(case_id) {
                    failing_families.insert(family.clone());
                }
            }
        }
    }
    ReducerDriftHistoryRecord {
        created_at_unix_ms: now_unix_ms(),
        ok: payload.get("ok").and_then(Value::as_bool).unwrap_or(false),
        case_count: payload
            .get("case_count")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        issue_count: payload
            .get("issue_count")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        failing_families: failing_families.into_iter().collect(),
        issue_kinds: issue_kinds.into_iter().collect(),
    }
}

fn memory_lint_history_record(payload: &Value) -> MemoryLintHistoryRecord {
    let mut issue_kinds = BTreeSet::<String>::new();
    if let Some(issues) = payload
        .get("lint")
        .and_then(|lint| lint.get("issues"))
        .and_then(Value::as_array)
    {
        for issue in issues {
            if let Some(kind) = issue.get("kind").and_then(Value::as_str) {
                issue_kinds.insert(kind.to_string());
            }
        }
    }
    MemoryLintHistoryRecord {
        created_at_unix_ms: now_unix_ms(),
        ok: payload.get("ok").and_then(Value::as_bool).unwrap_or(false),
        memory_count: payload
            .get("memory_count")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        issue_count: payload
            .get("issue_count")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        issue_kinds: issue_kinds.into_iter().collect(),
    }
}

fn context_anomaly_history_record(payload: &Value) -> ContextAnomalyHistoryRecord {
    ContextAnomalyHistoryRecord {
        created_at_unix_ms: now_unix_ms(),
        ok: payload.get("ok").and_then(Value::as_bool).unwrap_or(false),
        anomaly_count: payload
            .get("anomaly_count")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        high_count: payload
            .get("high_count")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        hidden_categories: payload
            .get("hidden_categories")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        hidden_samples: payload
            .get("hidden_samples")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|sample| {
                Some(ContextHiddenSample {
                    category: sample.get("category")?.as_str()?.to_string(),
                    signal: sample.get("signal")?.as_str()?.to_string(),
                })
            })
            .collect(),
    }
}

fn reducer_drift_tile(root: &Path) -> Result<ReducerDriftTile> {
    let records = load_reducer_drift_history(root, 32)?;
    if records.is_empty() {
        return Ok(ReducerDriftTile {
            latest_status: "none".to_string(),
            ..ReducerDriftTile::default()
        });
    }
    let latest = records.last().expect("non-empty reducer drift history");
    let mut counts = BTreeMap::<String, usize>::new();
    for record in &records {
        for kind in &record.issue_kinds {
            *counts.entry(kind.clone()).or_default() += 1;
        }
    }
    let recurring_issue_kinds = counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(kind, _)| kind.clone())
        .collect::<Vec<_>>();
    Ok(ReducerDriftTile {
        run_count: records.len(),
        latest_status: if latest.ok { "ready" } else { "blocked" }.to_string(),
        latest_issue_count: latest.issue_count as usize,
        latest_failing_families: latest.failing_families.clone(),
        recurring_issue_kinds,
    })
}

fn memory_lint_tile(root: &Path) -> Result<MemoryLintTile> {
    let records = load_memory_lint_history(root, 32)?;
    if records.is_empty() {
        return Ok(MemoryLintTile {
            latest_status: "none".to_string(),
            ..MemoryLintTile::default()
        });
    }
    let latest = records.last().expect("non-empty memory lint history");
    let mut counts = BTreeMap::<String, usize>::new();
    for record in &records {
        for kind in &record.issue_kinds {
            *counts.entry(kind.clone()).or_default() += 1;
        }
    }
    let recurring_issue_kinds = counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(kind, _)| kind.clone())
        .collect::<Vec<_>>();
    Ok(MemoryLintTile {
        run_count: records.len(),
        latest_status: if latest.ok { "ready" } else { "blocked" }.to_string(),
        latest_issue_count: latest.issue_count as usize,
        latest_issue_kinds: latest.issue_kinds.clone(),
        recurring_issue_kinds,
    })
}

fn context_anomaly_tile(root: &Path, history_path: Option<&Path>) -> Result<ContextAnomalyTile> {
    let records = match history_path {
        Some(path) => load_context_anomaly_history_from_path(path, 32)?,
        None => load_context_anomaly_history(root, 32)?,
    };
    if records.is_empty() {
        return Ok(ContextAnomalyTile {
            latest_status: "none".to_string(),
            ..ContextAnomalyTile::default()
        });
    }
    let latest = records.last().expect("non-empty context anomaly history");
    let (latest_age_ms, oldest_recurring_hidden_age_ms) =
        context_anomaly_age_summary(&records, now_unix_ms());
    Ok(ContextAnomalyTile {
        run_count: records.len(),
        latest_status: if latest.ok { "ready" } else { "blocked" }.to_string(),
        latest_anomaly_count: latest.anomaly_count as usize,
        latest_high_count: latest.high_count as usize,
        latest_age_ms,
        oldest_recurring_hidden_age_ms,
        latest_hidden_categories: latest.hidden_categories.clone(),
        recurring_hidden_categories: recurring_context_hidden_categories(&records),
        recurring_hidden_samples: recurring_context_hidden_samples(&records),
    })
}

fn load_reducer_drift_history(root: &Path, limit: usize) -> Result<Vec<ReducerDriftHistoryRecord>> {
    let path = reducer_drift_history_path(root);
    let Ok(raw) = fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    let mut records = raw
        .lines()
        .filter_map(|line| serde_json::from_str::<ReducerDriftHistoryRecord>(line).ok())
        .collect::<Vec<_>>();
    if records.len() > limit {
        records = records.split_off(records.len().saturating_sub(limit));
    }
    Ok(records)
}

fn load_memory_lint_history(root: &Path, limit: usize) -> Result<Vec<MemoryLintHistoryRecord>> {
    let path = memory_lint_history_path(root);
    let Ok(raw) = fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    let mut records = raw
        .lines()
        .filter_map(|line| serde_json::from_str::<MemoryLintHistoryRecord>(line).ok())
        .collect::<Vec<_>>();
    if records.len() > limit {
        records = records.split_off(records.len().saturating_sub(limit));
    }
    Ok(records)
}

fn reducer_drift_history_path(root: &Path) -> std::path::PathBuf {
    root.join(".packet28").join("reducer-drift-history.jsonl")
}

fn memory_lint_history_path(root: &Path) -> std::path::PathBuf {
    root.join(".packet28").join("memory-lint-history.jsonl")
}

fn context_anomaly_history_path(root: &Path) -> std::path::PathBuf {
    root.join(".packet28").join("context-anomaly-history.jsonl")
}

pub(crate) fn context_anomaly_digest(root: &Path) -> Result<ContextAnomalyDigest> {
    let savings = load_run_savings(root, 32)?;
    let handoff = handoff_readiness_tile(root)?;
    let reducer = reducer_drift_tile(root)?;
    let memory = memory_lint_tile(root)?;
    let anomaly_history = load_context_anomaly_history(root, 32)?;
    let mut anomalies = Vec::new();

    if handoff.regression_count > 0 || handoff.latest_status == "blocked" {
        let blockers = if handoff.latest_blocking_categories.is_empty() {
            "unknown".to_string()
        } else {
            handoff.latest_blocking_categories.join(",")
        };
        anomalies.push(ContextAnomaly {
            category: "handoff_readiness".to_string(),
            severity: if handoff.regression_count > 0 {
                "high"
            } else {
                "medium"
            }
            .to_string(),
            signal: format!(
                "latest_status={} blockers={} regressions={}",
                handoff.latest_status, blockers, handoff.regression_count
            ),
            next_check: "Packet28 verify handoffs --root . --max-regressions 0".to_string(),
            repair_hint: "refresh handoff with existing paths and runnable checks".to_string(),
        });
    }

    if reducer.latest_issue_count > 0 || !reducer.recurring_issue_kinds.is_empty() {
        let issue_kinds = if reducer.recurring_issue_kinds.is_empty() {
            "latest".to_string()
        } else {
            reducer.recurring_issue_kinds.join(",")
        };
        anomalies.push(ContextAnomaly {
            category: "reducer_drift".to_string(),
            severity: if reducer.latest_issue_count > 0 {
                "high"
            } else {
                "medium"
            }
            .to_string(),
            signal: format!(
                "latest_issues={} recurring={}",
                reducer.latest_issue_count, issue_kinds
            ),
            next_check: "Packet28 verify reducer-drift --root . --json".to_string(),
            repair_hint: "update fixtures or restore compact markers".to_string(),
        });
    }

    if memory.latest_issue_count > 0 || !memory.recurring_issue_kinds.is_empty() {
        let issue_kinds = if memory.latest_issue_kinds.is_empty() {
            memory.recurring_issue_kinds.join(",")
        } else {
            memory.latest_issue_kinds.join(",")
        };
        anomalies.push(ContextAnomaly {
            category: "memory_lint".to_string(),
            severity: if memory.latest_issue_count > 0 {
                "high"
            } else {
                "medium"
            }
            .to_string(),
            signal: format!(
                "latest_issues={} kinds={}",
                memory.latest_issue_count, issue_kinds
            ),
            next_check: "Packet28 verify memory-lint --root . --json".to_string(),
            repair_hint: "remove stale runtime memories or add hook evidence".to_string(),
        });
    }

    let changed_paths = savings
        .iter()
        .flat_map(|record| record.changed_paths.iter())
        .filter(|path| !path.trim().is_empty())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(3)
        .collect::<Vec<_>>();
    if let Some(first_path) = changed_paths.first() {
        anomalies.push(ContextAnomaly {
            category: "stale_changed_paths".to_string(),
            severity: "medium".to_string(),
            signal: format!("changed_paths={}", changed_paths.join(",")),
            next_check: format!("packet28.read_regions path={first_path} regions=[]"),
            repair_hint: "reread changed paths before using earlier context".to_string(),
        });
    }

    let fallback_records = savings
        .iter()
        .filter(|record| record.fallback_reason.is_some())
        .collect::<Vec<_>>();
    if let Some(latest) = fallback_records.first() {
        let reason = latest.fallback_reason.as_deref().unwrap_or("unknown");
        anomalies.push(ContextAnomaly {
            category: "fallback_provenance".to_string(),
            severity: "medium".to_string(),
            signal: format!(
                "recent_fallbacks={} latest_reason={}",
                fallback_records.len(),
                reason
            ),
            next_check: "Packet28 gain --failures".to_string(),
            repair_hint: "inspect fallback provenance before treating output as success"
                .to_string(),
        });
    }

    let recurring_hidden = recurring_context_hidden_categories(&anomaly_history);
    if !recurring_hidden.is_empty() {
        anomalies.push(ContextAnomaly {
            category: "context_anomaly_trend".to_string(),
            severity: "medium".to_string(),
            signal: format!("recurring_hidden={}", recurring_hidden.join(",")),
            next_check: "Packet28 verify context-anomalies --root . --json".to_string(),
            repair_hint: "inspect recurring hidden anomaly categories".to_string(),
        });
    }

    let hidden_anomalies = finalize_context_anomalies(&mut anomalies);
    let hidden_categories = hidden_anomalies
        .iter()
        .map(|anomaly| anomaly.category.clone())
        .collect::<Vec<_>>();
    let hidden_samples = context_hidden_samples(&hidden_anomalies);
    Ok(ContextAnomalyDigest {
        anomaly_count: anomalies.len(),
        truncated_count: hidden_categories.len(),
        hidden_categories,
        hidden_samples,
        anomalies,
    })
}

pub(crate) fn context_anomaly_trend_age_summary(root: &Path) -> Result<(u64, u64)> {
    let records = load_context_anomaly_history(root, 32)?;
    Ok(context_anomaly_age_summary(&records, now_unix_ms()))
}

fn load_context_anomaly_history(
    root: &Path,
    limit: usize,
) -> Result<Vec<ContextAnomalyHistoryRecord>> {
    let path = context_anomaly_history_path(root);
    load_context_anomaly_history_from_path(&path, limit)
}

fn load_context_anomaly_history_from_path(
    path: &Path,
    limit: usize,
) -> Result<Vec<ContextAnomalyHistoryRecord>> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    let mut records = raw
        .lines()
        .filter_map(|line| serde_json::from_str::<ContextAnomalyHistoryRecord>(line).ok())
        .collect::<Vec<_>>();
    if records.len() > limit {
        records = records.split_off(records.len().saturating_sub(limit));
    }
    Ok(records)
}

fn recurring_context_hidden_categories(records: &[ContextAnomalyHistoryRecord]) -> Vec<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for record in records {
        for category in &record.hidden_categories {
            *counts.entry(category.clone()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(category, _)| category)
        .collect()
}

fn recurring_context_hidden_samples(
    records: &[ContextAnomalyHistoryRecord],
) -> Vec<ContextHiddenSample> {
    let recurring = recurring_context_hidden_categories(records)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut samples = BTreeMap::<String, String>::new();
    for record in records.iter().rev() {
        for sample in &record.hidden_samples {
            if recurring.contains(&sample.category) {
                samples
                    .entry(sample.category.clone())
                    .or_insert_with(|| sample.signal.clone());
            }
        }
    }
    samples
        .into_iter()
        .map(|(category, signal)| ContextHiddenSample { category, signal })
        .collect()
}

fn context_anomaly_age_summary(records: &[ContextAnomalyHistoryRecord], now_ms: i64) -> (u64, u64) {
    let Some(latest) = records.last() else {
        return (0, 0);
    };
    let latest_age_ms = now_ms.saturating_sub(latest.created_at_unix_ms).max(0) as u64;
    let recurring_hidden = recurring_context_hidden_categories(records)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let oldest_recurring = records
        .iter()
        .filter(|record| {
            record
                .hidden_categories
                .iter()
                .any(|category| recurring_hidden.contains(category))
        })
        .map(|record| record.created_at_unix_ms)
        .min();
    let oldest_recurring_hidden_age_ms = oldest_recurring
        .map(|created_at| now_ms.saturating_sub(created_at).max(0) as u64)
        .unwrap_or_default();
    (latest_age_ms, oldest_recurring_hidden_age_ms)
}

fn finalize_context_anomalies(anomalies: &mut Vec<ContextAnomaly>) -> Vec<ContextAnomaly> {
    anomalies.sort_by_key(|anomaly| {
        (
            context_anomaly_severity_rank(&anomaly.severity),
            context_anomaly_category_rank(&anomaly.category),
            anomaly.category.clone(),
        )
    });
    if anomalies.len() <= MAX_CONTEXT_ANOMALIES {
        return Vec::new();
    }
    anomalies.split_off(MAX_CONTEXT_ANOMALIES)
}

fn context_hidden_samples(hidden_anomalies: &[ContextAnomaly]) -> Vec<ContextHiddenSample> {
    let mut samples = BTreeMap::<String, String>::new();
    for anomaly in hidden_anomalies {
        samples
            .entry(anomaly.category.clone())
            .or_insert_with(|| anomaly.signal.chars().take(120).collect());
    }
    samples
        .into_iter()
        .map(|(category, signal)| ContextHiddenSample { category, signal })
        .collect()
}

fn context_anomaly_severity_rank(severity: &str) -> usize {
    match severity {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    }
}

fn context_anomaly_category_rank(category: &str) -> usize {
    match category {
        "handoff_readiness" => 0,
        "reducer_drift" => 1,
        "memory_lint" => 2,
        "stale_changed_paths" => 3,
        "context_anomaly_trend" => 4,
        "fallback_provenance" => 5,
        _ => 99,
    }
}

fn handoff_readiness_tile(root: &Path) -> Result<HandoffReadinessTile> {
    let mut records = Vec::<Vec<String>>::new();
    for task_id in dashboard_task_ids(root)? {
        let versions_dir = task_artifact_dir(root, &task_id).join("versions");
        if !versions_dir.exists() {
            continue;
        }
        let mut paths = fs::read_dir(&versions_dir)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths.into_iter().take(12) {
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let Ok(payload) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            let categories = dashboard_handoff_categories(root, &payload);
            records.push(categories);
        }
    }
    if records.is_empty() {
        return Ok(HandoffReadinessTile {
            latest_status: "none".to_string(),
            ..HandoffReadinessTile::default()
        });
    }
    let latest = records.last().cloned().unwrap_or_default();
    let mut counts = BTreeMap::<String, usize>::new();
    for categories in &records {
        for category in categories {
            *counts.entry(category.clone()).or_default() += 1;
        }
    }
    let recurring_categories = counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(category, _)| category.clone())
        .collect::<Vec<_>>();
    let regression_count = latest
        .iter()
        .filter(|category| category_was_cleared_then_reintroduced(&records, category))
        .count();
    Ok(HandoffReadinessTile {
        artifact_count: records.len(),
        latest_status: if latest.is_empty() {
            "ready"
        } else {
            "blocked"
        }
        .to_string(),
        latest_blocking_categories: latest,
        recurring_categories,
        regression_count,
    })
}

fn dashboard_task_ids(root: &Path) -> Result<Vec<String>> {
    let mut ids = load_task_registry(root)
        .map(|registry| registry.tasks.into_keys().collect::<Vec<_>>())
        .unwrap_or_default();
    let probe = task_artifact_dir(root, "__packet28_probe__");
    if let Some(tasks_dir) = probe.parent() {
        if tasks_dir.exists() {
            for entry in fs::read_dir(tasks_dir)? {
                let entry = entry?;
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        let candidate = name.to_string();
                        if !ids.iter().any(|id| id == &candidate) {
                            ids.push(candidate);
                        }
                    }
                }
            }
        }
    }
    ids.sort();
    Ok(ids)
}

fn dashboard_handoff_categories(root: &Path, payload: &Value) -> Vec<String> {
    let mut categories = Vec::new();
    let text = dashboard_handoff_text(payload);
    if dashboard_missing_path_reference(root, &text) {
        categories.push("paths".to_string());
    }
    if dashboard_missing_test_command(&text) {
        categories.push("tests".to_string());
    }
    if dashboard_missing_env_reference(&text) {
        categories.push("environment".to_string());
    }
    categories
}

fn dashboard_handoff_text(payload: &Value) -> String {
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
    blocks.join("\n")
}

fn dashboard_missing_path_reference(root: &Path, text: &str) -> bool {
    text.split_whitespace().any(|token| {
        let token = dashboard_clean_token(token);
        !token.starts_with('/')
            && token.contains('/')
            && token
                .rsplit('/')
                .next()
                .is_some_and(|name| name.contains('.'))
            && !root.join(token).exists()
    })
}

fn dashboard_missing_test_command(text: &str) -> bool {
    let mentioned = text
        .split_whitespace()
        .map(dashboard_clean_token)
        .any(|token| token.ends_with("_test") || token.starts_with("test_"));
    mentioned && !text.lines().any(dashboard_line_contains_test_command)
}

fn dashboard_line_contains_test_command(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("cargo test")
        || lower.contains("npm test")
        || lower.contains("pnpm test")
        || lower.contains("pytest")
        || lower.contains("go test")
}

fn dashboard_missing_env_reference(text: &str) -> bool {
    text.split('$').skip(1).any(|tail| {
        let var = tail
            .chars()
            .take_while(|ch| *ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit())
            .collect::<String>();
        !var.is_empty() && std::env::var_os(var).is_none()
    })
}

fn dashboard_clean_token(token: &str) -> &str {
    token.trim_matches(|ch: char| {
        matches!(
            ch,
            '`' | '\'' | '"' | ',' | '.' | ';' | ':' | ')' | '(' | '[' | ']'
        )
    })
}

fn category_was_cleared_then_reintroduced(records: &[Vec<String>], category: &str) -> bool {
    let mut seen = false;
    for categories in records.iter().take(records.len().saturating_sub(1)) {
        if categories.iter().any(|candidate| candidate == category) {
            seen = true;
        } else if seen {
            return true;
        }
    }
    false
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn record_dashboard_route_roi(
    route_roi: &mut BTreeMap<String, DashboardRouteRoi>,
    record: &crate::savings_analytics::RunSavingsRecord,
) {
    let route = if let Some(reason) = &record.fallback_reason {
        format!("run_fallback:{reason}")
    } else {
        format!("run_reducer:{}", record.family)
    };
    let entry = route_roi
        .entry(route.clone())
        .or_insert_with(|| DashboardRouteRoi {
            route,
            ..DashboardRouteRoi::default()
        });
    entry.invocation_count += 1;
    entry.raw_est_tokens = entry.raw_est_tokens.saturating_add(record.raw_est_tokens);
    entry.reduced_est_tokens = entry
        .reduced_est_tokens
        .saturating_add(record.reduced_est_tokens);
    entry.saved_est_tokens = entry
        .raw_est_tokens
        .saturating_sub(entry.reduced_est_tokens);
    entry.savings_percent = if entry.raw_est_tokens == 0 {
        0.0
    } else {
        (entry.saved_est_tokens as f64 / entry.raw_est_tokens as f64) * 100.0
    };
}

#[cfg(test)]
#[path = "cmd_dashboard_tests.rs"]
mod tests;
