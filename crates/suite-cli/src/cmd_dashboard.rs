use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

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
}

#[derive(Debug, Serialize, Default)]
pub(crate) struct ContextAnomalyDigest {
    pub(crate) anomaly_count: usize,
    pub(crate) anomalies: Vec<ContextAnomaly>,
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

#[derive(Debug, Serialize, Deserialize, Default)]
struct MemoryLintHistoryRecord {
    created_at_unix_ms: i64,
    ok: bool,
    memory_count: u64,
    issue_count: u64,
    issue_kinds: Vec<String>,
}

pub fn run(args: DashboardArgs) -> Result<i32> {
    let root = crate::cmd_daemon::resolve_root_arg(&args.root);
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

pub(crate) fn context_anomaly_digest(root: &Path) -> Result<ContextAnomalyDigest> {
    let savings = load_run_savings(root, 32)?;
    let handoff = handoff_readiness_tile(root)?;
    let reducer = reducer_drift_tile(root)?;
    let memory = memory_lint_tile(root)?;
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
            repair_hint:
                "refresh the handoff packet with existing paths, runnable checks, and current env"
                    .to_string(),
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
            repair_hint: "update reducer fixtures or restore missing decisive compact markers"
                .to_string(),
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
            repair_hint:
                "remove stale runtime-specific memories or add hook evidence for the runtime"
                    .to_string(),
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
            repair_hint: "inspect fallback provenance before treating reduced output as successful"
                .to_string(),
        });
    }

    anomalies.sort_by_key(|anomaly| match anomaly.severity.as_str() {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    });
    Ok(ContextAnomalyDigest {
        anomaly_count: anomalies.len(),
        anomalies,
    })
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

#[derive(Clone, Copy)]
enum DashboardPanel {
    Overview,
    Memory,
    Graph,
    Feedback,
    Integrations,
}

impl DashboardPanel {
    fn from_command(command: &str) -> Option<Self> {
        match command.trim().to_ascii_lowercase().as_str() {
            "1" | "overview" | "o" => Some(Self::Overview),
            "2" | "memory" | "m" => Some(Self::Memory),
            "3" | "graph" | "g" => Some(Self::Graph),
            "4" | "feedback" | "f" => Some(Self::Feedback),
            "5" | "integrations" | "i" => Some(Self::Integrations),
            _ => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Memory => "Memory",
            Self::Graph => "Graph",
            Self::Feedback => "Feedback",
            Self::Integrations => "Integrations",
        }
    }
}

fn run_dashboard_tui(report: &DashboardReport) -> Result<()> {
    let mut panel = DashboardPanel::Overview;
    let mut input = String::new();
    loop {
        print!("{}", render_dashboard_tui(report, panel));
        io::stdout().flush()?;
        input.clear();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        let command = input.trim();
        if matches!(command, "q" | "quit" | "exit") {
            break;
        }
        if let Some(next) = DashboardPanel::from_command(command) {
            panel = next;
        }
    }
    Ok(())
}

fn render_dashboard_tui(report: &DashboardReport, panel: DashboardPanel) -> String {
    let mut out = String::new();
    out.push_str("Packet28 Dashboard\n");
    out.push_str("==================\n");
    out.push_str("1 Overview  2 Memory  3 Graph  4 Feedback  5 Integrations  q Quit\n");
    out.push_str(&format!("panel={}\n\n", panel.title()));
    match panel {
        DashboardPanel::Overview => {
            out.push_str(&format!(
                "saved_tokens={}\nsavings_percent={:.1}\ncommands_reduced={}\nsessions={}\n",
                report.token_savings.saved_est_tokens,
                report.token_savings.savings_percent,
                report.commands_reduced,
                report.sessions
            ));
            out.push_str(&format!(
                "handoff_latest_status={}\nhandoff_regression_count={}\n",
                report.handoff_readiness.latest_status, report.handoff_readiness.regression_count
            ));
            out.push_str(&format!(
                "reducer_drift_latest_status={}\nreducer_drift_latest_issue_count={}\n",
                report.reducer_drift.latest_status, report.reducer_drift.latest_issue_count
            ));
            out.push_str(&format!(
                "memory_lint_latest_status={}\nmemory_lint_latest_issue_count={}\n",
                report.memory_lint.latest_status, report.memory_lint.latest_issue_count
            ));
            out.push_str("handoff_latest_blockers:\n");
            push_tui_list(
                &mut out,
                &report.handoff_readiness.latest_blocking_categories,
            );
            out.push_str("top_saved_routes:\n");
            for route in &report.top_saved_routes {
                out.push_str(&format!(
                    "- {} saved={} pct={:.1}\n",
                    route.route, route.saved_est_tokens, route.savings_percent
                ));
            }
            out.push_str("top_noisy_commands:\n");
            push_tui_list(&mut out, &report.top_noisy_commands);
            out.push_str("missed_savings:\n");
            push_tui_list(&mut out, &report.missed_savings);
        }
        DashboardPanel::Memory => {
            out.push_str(&format!(
                "memory_count={}\ntopics={}\ntopics_needing_consolidation={}\npending_extractions={}\n",
                report.memory_count,
                report.memory_topics.len(),
                report.memory_health.topics_needing_consolidation,
                report.pending_extractions
            ));
            out.push_str("recent_memories:\n");
            push_tui_list(&mut out, &report.recent_memories);
            out.push_str("memory_topics:\n");
            for topic in &report.memory_topics {
                out.push_str(&format!("- {} ({})\n", topic.topic, topic.memory_count));
            }
        }
        DashboardPanel::Graph => {
            out.push_str(&format!(
                "graph_concepts={}\ngraph_relations={}\nrelation_types={}\n",
                report.graph_concepts,
                report.graph_relations,
                report.graph_stats.relation_types.len()
            ));
        }
        DashboardPanel::Feedback => {
            out.push_str(&format!(
                "feedback_corrections={}\ntranscript_messages={}\nmcp_call_history={}\nhook_event_history={}\n",
                report.feedback_corrections,
                report.transcript_stats.message_count,
                report.mcp_call_history,
                report.hook_event_history
            ));
        }
        DashboardPanel::Integrations => {
            out.push_str(&format!(
                "windsurf_doctor_status={}\n",
                report.windsurf_doctor_status
            ));
            for (name, status) in &report.integration_health {
                out.push_str(&format!("{name}={status}\n"));
            }
        }
    }
    out.push('\n');
    out
}

fn push_tui_list(out: &mut String, values: &[String]) {
    if values.is_empty() {
        out.push_str("- none\n");
    } else {
        for value in values {
            out.push_str(&format!("- {}\n", value.replace('\n', " ")));
        }
    }
}

fn render_dashboard_html(report: &DashboardReport) -> String {
    let mut html = String::from(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Packet28 Dashboard</title>
<style>
body{font-family:system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;margin:0;background:#f7f7f4;color:#1f2328}
main{max-width:1120px;margin:0 auto;padding:28px}
h1{font-size:28px;margin:0 0 20px}
h2{font-size:17px;margin:24px 0 10px}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:12px}
.metric{border:1px solid #d8d7d0;background:#fff;padding:14px;border-radius:8px}
.label{font-size:12px;text-transform:uppercase;color:#667085}
.value{font-size:26px;font-weight:700;margin-top:6px}
table{width:100%;border-collapse:collapse;background:#fff;border:1px solid #d8d7d0}
th,td{text-align:left;padding:8px 10px;border-bottom:1px solid #ecebe6;font-size:14px}
th{background:#efeee8}
code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}
</style>
</head>
<body><main>
<h1>Packet28 Dashboard</h1>
"#,
    );
    html.push_str("<section class=\"grid\">");
    push_metric(
        &mut html,
        "Saved tokens",
        &report.token_savings.saved_est_tokens.to_string(),
    );
    push_metric(
        &mut html,
        "Savings",
        &format!("{:.1}%", report.token_savings.savings_percent),
    );
    push_metric(
        &mut html,
        "Commands reduced",
        &report.commands_reduced.to_string(),
    );
    push_metric(&mut html, "Sessions", &report.sessions.to_string());
    push_metric(&mut html, "Memories", &report.memory_count.to_string());
    push_metric(&mut html, "Topics", &report.memory_topics.len().to_string());
    push_metric(
        &mut html,
        "Graph concepts",
        &report.graph_concepts.to_string(),
    );
    push_metric(
        &mut html,
        "Graph relations",
        &report.graph_relations.to_string(),
    );
    push_metric(
        &mut html,
        "Feedback corrections",
        &report.feedback_corrections.to_string(),
    );
    push_metric(
        &mut html,
        "Transcript messages",
        &report.transcript_stats.message_count.to_string(),
    );
    push_metric(
        &mut html,
        "Pending extractions",
        &report.pending_extractions.to_string(),
    );
    push_metric(
        &mut html,
        "Handoff status",
        &report.handoff_readiness.latest_status,
    );
    push_metric(
        &mut html,
        "Handoff regressions",
        &report.handoff_readiness.regression_count.to_string(),
    );
    push_metric(
        &mut html,
        "Reducer drift",
        &report.reducer_drift.latest_status,
    );
    push_metric(&mut html, "Memory lint", &report.memory_lint.latest_status);
    html.push_str("</section>");

    html.push_str("<h2>Memory Topics</h2><table><tr><th>Topic</th><th>Count</th></tr>");
    for topic in &report.memory_topics {
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td></tr>",
            escape_html(&topic.topic),
            topic.memory_count
        ));
    }
    html.push_str("</table>");

    html.push_str("<h2>Top Saved Routes</h2><table><tr><th>Route</th><th>Saved tokens</th><th>Savings</th></tr>");
    for route in &report.top_saved_routes {
        html.push_str(&format!(
            "<tr><td><code>{}</code></td><td>{}</td><td>{:.1}%</td></tr>",
            escape_html(&route.route),
            route.saved_est_tokens,
            route.savings_percent
        ));
    }
    html.push_str("</table>");

    html.push_str("<h2>Top Noisy Commands</h2><table><tr><th>Command</th></tr>");
    for command in &report.top_noisy_commands {
        html.push_str(&format!(
            "<tr><td><code>{}</code></td></tr>",
            escape_html(command)
        ));
    }
    html.push_str("</table>");

    html.push_str("<h2>Handoff Readiness</h2><table><tr><th>Signal</th><th>Value</th></tr>");
    html.push_str(&format!(
        "<tr><td>Latest status</td><td>{}</td></tr>",
        escape_html(&report.handoff_readiness.latest_status)
    ));
    html.push_str(&format!(
        "<tr><td>Latest blockers</td><td><code>{}</code></td></tr>",
        escape_html(
            &report
                .handoff_readiness
                .latest_blocking_categories
                .join(",")
        )
    ));
    html.push_str(&format!(
        "<tr><td>Recurring categories</td><td><code>{}</code></td></tr>",
        escape_html(&report.handoff_readiness.recurring_categories.join(","))
    ));
    html.push_str(&format!(
        "<tr><td>Regressions</td><td>{}</td></tr>",
        report.handoff_readiness.regression_count
    ));
    html.push_str("</table>");

    html.push_str("<h2>Reducer Drift</h2><table><tr><th>Signal</th><th>Value</th></tr>");
    html.push_str(&format!(
        "<tr><td>Latest status</td><td>{}</td></tr>",
        escape_html(&report.reducer_drift.latest_status)
    ));
    html.push_str(&format!(
        "<tr><td>Latest issues</td><td>{}</td></tr>",
        report.reducer_drift.latest_issue_count
    ));
    html.push_str(&format!(
        "<tr><td>Failing families</td><td><code>{}</code></td></tr>",
        escape_html(&report.reducer_drift.latest_failing_families.join(","))
    ));
    html.push_str(&format!(
        "<tr><td>Recurring issues</td><td><code>{}</code></td></tr>",
        escape_html(&report.reducer_drift.recurring_issue_kinds.join(","))
    ));
    html.push_str("</table>");

    html.push_str("<h2>Memory Lint</h2><table><tr><th>Signal</th><th>Value</th></tr>");
    html.push_str(&format!(
        "<tr><td>Latest status</td><td>{}</td></tr>",
        escape_html(&report.memory_lint.latest_status)
    ));
    html.push_str(&format!(
        "<tr><td>Latest issues</td><td>{}</td></tr>",
        report.memory_lint.latest_issue_count
    ));
    html.push_str(&format!(
        "<tr><td>Latest issue kinds</td><td><code>{}</code></td></tr>",
        escape_html(&report.memory_lint.latest_issue_kinds.join(","))
    ));
    html.push_str(&format!(
        "<tr><td>Recurring issues</td><td><code>{}</code></td></tr>",
        escape_html(&report.memory_lint.recurring_issue_kinds.join(","))
    ));
    html.push_str("</table>");

    html.push_str("<h2>Integration Health</h2><table><tr><th>Integration</th><th>Status</th></tr>");
    for (name, status) in &report.integration_health {
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td></tr>",
            escape_html(name),
            escape_html(status)
        ));
    }
    html.push_str("</table>");
    html.push_str("</main></body></html>\n");
    html
}

fn push_metric(html: &mut String, label: &str, value: &str) {
    html.push_str(&format!(
        "<div class=\"metric\"><div class=\"label\">{}</div><div class=\"value\">{}</div></div>",
        escape_html(label),
        escape_html(value)
    ));
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::savings_analytics::{record_run_savings, RunSavingsRecord};

    fn drift_payload(ok: bool, issue_count: u64) -> Value {
        let issues = if issue_count == 0 {
            Vec::new()
        } else {
            vec![serde_json::json!({
                "case_id": "cargo-failing-test-name",
                "kind": "missing_marker",
                "detail": "FAIL drift_marker"
            })]
        };
        serde_json::json!({
            "ok": ok,
            "case_count": 1,
            "issue_count": issue_count,
            "issues": issues,
            "summaries": [{
                "case_id": "cargo-failing-test-name",
                "family": "rust",
                "canonical_kind": "rust_test",
                "summary": "cargo test reported 0 passed and 1 failed"
            }]
        })
    }

    fn memory_lint_payload(ok: bool, issue_count: u64) -> Value {
        let issues = if issue_count == 0 {
            Vec::new()
        } else {
            vec![serde_json::json!({
                "memory_id": 1,
                "kind": "runtime_specific_memory",
                "detail": "mentions windsurf"
            })]
        };
        serde_json::json!({
            "ok": ok,
            "memory_count": 2,
            "issue_count": issue_count,
            "lint": {
                "memory_count": 2,
                "issue_count": issue_count,
                "issues": issues
            }
        })
    }

    #[test]
    fn reducer_drift_tile_reports_recurring_and_cleared_latest_failure() {
        let root = tempfile::tempdir().unwrap();
        record_reducer_drift_history(root.path(), &drift_payload(false, 1)).unwrap();
        record_reducer_drift_history(root.path(), &drift_payload(false, 1)).unwrap();
        record_reducer_drift_history(root.path(), &drift_payload(true, 0)).unwrap();

        let tile = reducer_drift_tile(root.path()).unwrap();

        assert_eq!(tile.run_count, 3);
        assert_eq!(tile.latest_status, "ready");
        assert_eq!(tile.latest_issue_count, 0);
        assert!(tile.latest_failing_families.is_empty());
        assert_eq!(tile.recurring_issue_kinds, vec!["missing_marker"]);
        assert!(serde_json::to_string(&tile).unwrap().len() < 768);
    }

    #[test]
    fn memory_lint_tile_reports_recurring_and_cleared_latest_issue() {
        let root = tempfile::tempdir().unwrap();
        record_memory_lint_history(root.path(), &memory_lint_payload(false, 1)).unwrap();
        record_memory_lint_history(root.path(), &memory_lint_payload(false, 1)).unwrap();
        record_memory_lint_history(root.path(), &memory_lint_payload(true, 0)).unwrap();

        let tile = memory_lint_tile(root.path()).unwrap();

        assert_eq!(tile.run_count, 3);
        assert_eq!(tile.latest_status, "ready");
        assert_eq!(tile.latest_issue_count, 0);
        assert!(tile.latest_issue_kinds.is_empty());
        assert_eq!(tile.recurring_issue_kinds, vec!["runtime_specific_memory"]);
        assert!(serde_json::to_string(&tile).unwrap().len() < 768);
    }

    #[test]
    fn context_anomaly_digest_ranks_drift_and_memory_with_next_checks() {
        let root = tempfile::tempdir().unwrap();
        record_reducer_drift_history(root.path(), &drift_payload(false, 1)).unwrap();
        record_memory_lint_history(root.path(), &memory_lint_payload(false, 1)).unwrap();

        let digest = context_anomaly_digest(root.path()).unwrap();

        assert_eq!(digest.anomaly_count, 2);
        assert_eq!(digest.anomalies[0].category, "reducer_drift");
        assert_eq!(digest.anomalies[0].severity, "high");
        assert!(digest.anomalies[0].next_check.contains("reducer-drift"));
        assert!(digest.anomalies[0].repair_hint.contains("compact markers"));
        assert_eq!(digest.anomalies[1].category, "memory_lint");
        assert_eq!(digest.anomalies[1].severity, "high");
        assert!(digest.anomalies[1].next_check.contains("memory-lint"));
        assert!(digest.anomalies[1].repair_hint.contains("stale runtime"));
        assert!(serde_json::to_string(&digest).unwrap().len() < 1024);
    }

    #[test]
    fn context_anomaly_digest_includes_medium_fallback_provenance() {
        let root = tempfile::tempdir().unwrap();
        record_reducer_drift_history(root.path(), &drift_payload(false, 1)).unwrap();
        record_run_savings(
            root.path(),
            &RunSavingsRecord {
                command: "p28 search --backend fff query".to_string(),
                cwd: root.path().display().to_string(),
                family: "search".to_string(),
                canonical_kind: "search".to_string(),
                exit_code: 0,
                raw_est_tokens: 1200,
                reduced_est_tokens: 200,
                savings_percent: 83.0,
                fallback_reason: Some(
                    "fff auto preferred backend failed: launch error".to_string(),
                ),
                failure_fingerprint: None,
                changed_paths: Vec::new(),
                timestamp_unix_ms: 1,
            },
        )
        .unwrap();

        let digest = context_anomaly_digest(root.path()).unwrap();

        assert_eq!(digest.anomaly_count, 2);
        assert_eq!(digest.anomalies[0].category, "reducer_drift");
        assert_eq!(digest.anomalies[0].severity, "high");
        assert_eq!(digest.anomalies[1].category, "fallback_provenance");
        assert_eq!(digest.anomalies[1].severity, "medium");
        assert!(digest.anomalies[1].next_check.contains("gain --failures"));
        assert!(digest.anomalies[1]
            .repair_hint
            .contains("fallback provenance"));
        assert!(serde_json::to_string(&digest).unwrap().len() < 1024);
    }
}
