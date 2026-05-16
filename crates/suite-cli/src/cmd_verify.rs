use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use packet28_reducer_core::{classify_command_argv, reduce_command_output};
use serde::Deserialize;
use serde_json::json;

#[derive(Args)]
pub struct VerifyArgs {
    #[command(subcommand)]
    pub command: VerifyCommands,
}

#[derive(Subcommand)]
pub enum VerifyCommands {
    /// Run inline tests from Packet28/RTK-compatible TOML output filters
    Filters(FilterVerifyArgs),
    /// Verify experiment manifests reference concrete evidence artifacts
    Experiments(ExperimentVerifyArgs),
    /// Verify handoff lint readiness for CI logs
    Handoffs(HandoffVerifyArgs),
    /// Verify reducer output keeps decisive markers from golden raw fixtures
    ReducerDrift(ReducerDriftVerifyArgs),
    /// Verify cross-agent memory lint fixtures catch stale runtime-specific advice
    MemoryLint(MemoryLintVerifyArgs),
    /// Verify context anomaly digest stays below configured thresholds
    ContextAnomalies(ContextAnomalyVerifyArgs),
}

#[derive(Args)]
pub struct FilterVerifyArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub filter: Option<String>,
    #[arg(long)]
    pub require_all: bool,
    #[arg(long)]
    pub trust: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct ExperimentVerifyArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long, default_value = "docs/experiments/manifest.json")]
    pub manifest: String,
    #[arg(long = "require-workflow")]
    pub require_workflows: Vec<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    #[arg(long)]
    pub score: bool,
}

#[derive(Args)]
pub struct HandoffVerifyArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long, default_value_t = 0)]
    pub max_regressions: u64,
    #[arg(long)]
    pub require_ready: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct ReducerDriftVerifyArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long, default_value = "docs/reducer-drift/fixtures.json")]
    pub fixture: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryLintVerifyArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long, default_value = "docs/memory-lint/fixtures.json")]
    pub fixture: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct ContextAnomalyVerifyArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long, default_value_t = 999)]
    pub max_anomalies: usize,
    #[arg(long, default_value_t = 0)]
    pub max_high: usize,
    #[arg(long)]
    pub max_trend_age_ms: Option<u64>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ExperimentManifest {
    experiments: Vec<ExperimentEntry>,
}

impl Default for ExperimentManifest {
    fn default() -> Self {
        Self {
            experiments: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ExperimentEntry {
    id: String,
    workflow: String,
    commands: Vec<String>,
    artifacts: Vec<String>,
    metrics: Vec<ExperimentMetric>,
    runtime_versions: Vec<ExperimentRuntimeVersion>,
    fallback_reasons: Vec<String>,
    allow_fallbacks: bool,
}

impl Default for ExperimentEntry {
    fn default() -> Self {
        Self {
            id: String::new(),
            workflow: String::new(),
            commands: Vec::new(),
            artifacts: Vec::new(),
            metrics: Vec::new(),
            runtime_versions: Vec::new(),
            fallback_reasons: Vec::new(),
            allow_fallbacks: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ExperimentMetric {
    name: String,
    value: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    evidence: Vec<String>,
}

impl Default for ExperimentMetric {
    fn default() -> Self {
        Self {
            name: String::new(),
            value: None,
            min: None,
            max: None,
            evidence: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ExperimentRuntimeVersion {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ReducerDriftFixture {
    cases: Vec<ReducerDriftCase>,
}

impl Default for ReducerDriftFixture {
    fn default() -> Self {
        Self { cases: Vec::new() }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ReducerDriftCase {
    id: String,
    command_argv: Vec<String>,
    stdout: String,
    stderr: String,
    exit_code: i32,
    required_markers: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct MemoryLintFixture {
    memories: Vec<MemoryLintFixtureMemory>,
    hook_events: Vec<MemoryLintFixtureHookEvent>,
    expected_issue_count: usize,
    expected_issue_kinds: Vec<String>,
    clean_memory_ids: Vec<i64>,
}

impl Default for MemoryLintFixture {
    fn default() -> Self {
        Self {
            memories: Vec::new(),
            hook_events: Vec::new(),
            expected_issue_count: 0,
            expected_issue_kinds: Vec::new(),
            clean_memory_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct MemoryLintFixtureMemory {
    id: i64,
    content: String,
    tags: Option<String>,
    topic: String,
    importance: String,
    keywords: Option<String>,
    project: Option<String>,
    source: Option<String>,
    raw_excerpt: Option<String>,
}

impl Default for MemoryLintFixtureMemory {
    fn default() -> Self {
        Self {
            id: 0,
            content: String::new(),
            tags: None,
            topic: "general".to_string(),
            importance: "medium".to_string(),
            keywords: None,
            project: None,
            source: None,
            raw_excerpt: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct MemoryLintFixtureHookEvent {
    runtime: String,
    event_kind: String,
}

impl Default for MemoryLintFixtureHookEvent {
    fn default() -> Self {
        Self {
            runtime: String::new(),
            event_kind: String::new(),
        }
    }
}

impl Default for ReducerDriftCase {
    fn default() -> Self {
        Self {
            id: String::new(),
            command_argv: Vec::new(),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            required_markers: Vec::new(),
        }
    }
}

impl Default for ExperimentRuntimeVersion {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: String::new(),
        }
    }
}

#[derive(Debug)]
struct ExperimentIssue {
    experiment_id: String,
    kind: String,
    detail: String,
}

pub fn run(args: VerifyArgs) -> Result<i32> {
    match args.command {
        VerifyCommands::Filters(args) => run_filters(args),
        VerifyCommands::Experiments(args) => run_experiments(args),
        VerifyCommands::Handoffs(args) => run_handoffs(args),
        VerifyCommands::ReducerDrift(args) => run_reducer_drift(args),
        VerifyCommands::MemoryLint(args) => run_memory_lint(args),
        VerifyCommands::ContextAnomalies(args) => run_context_anomalies(args),
    }
}

fn run_filters(args: FilterVerifyArgs) -> Result<i32> {
    anyhow::ensure!(
        !args.trust || args.filter.is_none(),
        "--trust cannot be combined with --filter because trust applies to the whole filter file"
    );
    let cwd = crate::cmd_common::caller_cwd()?;
    let root = std::path::PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.root, &cwd));
    let results = crate::toml_filters::run_filter_tests(&root, args.filter.as_deref())?;
    let total = results.outcomes.len();
    let passed = results
        .outcomes
        .iter()
        .filter(|outcome| outcome.passed)
        .count();
    let failed = total.saturating_sub(passed);
    let missing = results.filters_without_tests.len();
    let success = failed == 0 && (!args.require_all || missing == 0);
    let trusted_filters = if success && args.trust {
        crate::toml_filters::trust_project_filters(&root)?
    } else {
        Vec::new()
    };

    if args.json {
        let outcomes = results
            .outcomes
            .iter()
            .map(|outcome| {
                json!({
                    "filter": outcome.filter_name,
                    "test": outcome.test_name,
                    "source": outcome.source,
                    "passed": outcome.passed,
                    "expected": outcome.expected,
                    "actual": outcome.actual,
                })
            })
            .collect::<Vec<_>>();
        crate::cmd_common::emit_json(
            &json!({
                "ok": success,
                "total": total,
                "passed": passed,
                "failed": failed,
                "filters_without_tests": results.filters_without_tests,
                "trusted_filters": trusted_filters,
                "outcomes": outcomes,
            }),
            args.pretty,
        )?;
    } else {
        for outcome in &results.outcomes {
            if !outcome.passed {
                eprintln!(
                    "FAIL [{}] {} ({})\n  expected: {:?}\n  actual:   {:?}",
                    outcome.filter_name,
                    outcome.test_name,
                    outcome.source,
                    outcome.expected,
                    outcome.actual
                );
            }
        }
        if total == 0 {
            println!("No inline filter tests found.");
        } else {
            println!("{passed}/{total} filter tests passed");
        }
        if args.require_all {
            for name in &results.filters_without_tests {
                eprintln!("MISSING tests for filter: {name}");
            }
        }
        for path in &trusted_filters {
            println!("Trusted filter config: {path}");
        }
    }

    if success {
        Ok(0)
    } else {
        Ok(1)
    }
}

fn run_experiments(args: ExperimentVerifyArgs) -> Result<i32> {
    let cwd = crate::cmd_common::caller_cwd()?;
    let root = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.root, &cwd));
    let manifest_path = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(
        &args.manifest,
        &root,
    ));
    let payload =
        verify_experiments_payload(&root, &manifest_path, &args.require_workflows, args.score)?;
    let ok = payload.get("ok").and_then(serde_json::Value::as_bool) == Some(true);

    if args.json {
        crate::cmd_common::emit_json(&payload, args.pretty)?;
    } else {
        let experiment_count = payload
            .get("experiment_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        if ok {
            if args.score {
                println!(
                    "{experiment_count} experiment(s) scored from {}",
                    manifest_path.display()
                );
                if let Some(scores) = payload.get("scores").and_then(serde_json::Value::as_array) {
                    for score in scores {
                        println!(
                            "{} score={} passing={}",
                            score
                                .get("id")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("<unknown>"),
                            score
                                .get("score")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or_default(),
                            score
                                .get("passing")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false)
                        );
                    }
                }
            } else {
                println!(
                    "{experiment_count} experiment(s) verified from {}",
                    manifest_path.display()
                );
            }
        } else if let Some(issues) = payload.get("issues").and_then(serde_json::Value::as_array) {
            for issue in issues {
                eprintln!(
                    "FAIL [{}] {}: {}",
                    issue
                        .get("experiment_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("<unknown>"),
                    issue
                        .get("kind")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("issue"),
                    issue
                        .get("detail")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                );
            }
        }
    }

    if ok {
        Ok(0)
    } else {
        Ok(1)
    }
}

fn run_handoffs(args: HandoffVerifyArgs) -> Result<i32> {
    let cwd = crate::cmd_common::caller_cwd()?;
    let root = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.root, &cwd));
    let summary = crate::cmd_dashboard::handoff_readiness_payload(&root)?;
    let regression_count = summary
        .get("regression_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let latest_status = summary
        .get("latest_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("none");
    let ok = regression_count <= args.max_regressions
        && (!args.require_ready || latest_status == "ready" || latest_status == "none");
    let payload = json!({
        "ok": ok,
        "max_regressions": args.max_regressions,
        "require_ready": args.require_ready,
        "handoff_readiness": summary,
    });

    if args.json {
        crate::cmd_common::emit_json(&payload, args.pretty)?;
    } else {
        println!("handoff_latest_status={latest_status}");
        println!("handoff_regression_count={regression_count}");
        println!("handoff_max_regressions={}", args.max_regressions);
        println!("handoff_require_ready={}", args.require_ready);
        println!("handoff_ok={ok}");
    }

    if ok {
        Ok(0)
    } else {
        Ok(1)
    }
}

fn run_reducer_drift(args: ReducerDriftVerifyArgs) -> Result<i32> {
    let cwd = crate::cmd_common::caller_cwd()?;
    let root = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.root, &cwd));
    let fixture_path = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(
        &args.fixture,
        &root,
    ));
    let payload = verify_reducer_drift_payload(&fixture_path)?;
    crate::cmd_dashboard::record_reducer_drift_history(&root, &payload)?;
    let ok = payload.get("ok").and_then(serde_json::Value::as_bool) == Some(true);

    if args.json {
        crate::cmd_common::emit_json(&payload, args.pretty)?;
    } else {
        println!("reducer_drift_fixture={}", fixture_path.display());
        println!(
            "reducer_drift_cases={}",
            payload
                .get("case_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default()
        );
        println!(
            "reducer_drift_issues={}",
            payload
                .get("issue_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default()
        );
        println!("reducer_drift_ok={ok}");
        if let Some(issues) = payload.get("issues").and_then(serde_json::Value::as_array) {
            for issue in issues {
                eprintln!(
                    "FAIL [{}] {}: {}",
                    issue
                        .get("case_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("<unknown>"),
                    issue
                        .get("kind")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("issue"),
                    issue
                        .get("detail")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                );
            }
        }
    }

    if ok {
        Ok(0)
    } else {
        Ok(1)
    }
}

fn run_memory_lint(args: MemoryLintVerifyArgs) -> Result<i32> {
    let cwd = crate::cmd_common::caller_cwd()?;
    let root = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.root, &cwd));
    let fixture_path = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(
        &args.fixture,
        &root,
    ));
    let payload = verify_memory_lint_payload(&root, &fixture_path)?;
    crate::cmd_dashboard::record_memory_lint_history(&root, &payload)?;
    let ok = payload.get("ok").and_then(serde_json::Value::as_bool) == Some(true);

    if args.json {
        crate::cmd_common::emit_json(&payload, args.pretty)?;
    } else {
        println!("memory_lint_fixture={}", fixture_path.display());
        println!(
            "memory_lint_issues={}",
            payload
                .get("issue_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default()
        );
        println!("memory_lint_ok={ok}");
        if let Some(issues) = payload
            .get("expectation_issues")
            .and_then(serde_json::Value::as_array)
        {
            for issue in issues {
                eprintln!("FAIL memory_lint_expectation: {issue}");
            }
        }
    }

    if ok {
        Ok(0)
    } else {
        Ok(1)
    }
}

fn run_context_anomalies(args: ContextAnomalyVerifyArgs) -> Result<i32> {
    let cwd = crate::cmd_common::caller_cwd()?;
    let root = PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.root, &cwd));
    let payload = verify_context_anomalies_payload(
        &root,
        args.max_anomalies,
        args.max_high,
        args.max_trend_age_ms,
    )?;
    crate::cmd_dashboard::record_context_anomaly_history(&root, &payload)?;
    let ok = payload.get("ok").and_then(serde_json::Value::as_bool) == Some(true);

    if args.json {
        crate::cmd_common::emit_json(&payload, args.pretty)?;
    } else {
        println!(
            "context_anomaly_count={}",
            payload
                .get("anomaly_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default()
        );
        println!(
            "context_anomaly_high_count={}",
            payload
                .get("high_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default()
        );
        println!("context_anomaly_max_anomalies={}", args.max_anomalies);
        println!("context_anomaly_max_high={}", args.max_high);
        if let Some(max_trend_age_ms) = args.max_trend_age_ms {
            println!("context_anomaly_max_trend_age_ms={max_trend_age_ms}");
            println!(
                "context_anomaly_trend_latest_age_ms={}",
                payload
                    .get("trend_latest_age_ms")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default()
            );
            println!(
                "context_anomaly_trend_oldest_recurring_hidden_age_ms={}",
                payload
                    .get("trend_oldest_recurring_hidden_age_ms")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default()
            );
            if let Some(hint) = payload
                .get("trend_repair_hint")
                .and_then(serde_json::Value::as_str)
            {
                println!("context_anomaly_trend_repair_hint={hint}");
            }
        }
        println!("context_anomaly_ok={ok}");
    }

    if ok {
        Ok(0)
    } else {
        Ok(1)
    }
}

pub(crate) fn verify_experiments_payload(
    root: &Path,
    manifest_path: &Path,
    required_workflows: &[String],
    include_score: bool,
) -> Result<serde_json::Value> {
    let manifest_raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read '{}'", manifest_path.display()))?;
    let manifest: ExperimentManifest = serde_json::from_str(&manifest_raw)
        .with_context(|| format!("failed to parse '{}'", manifest_path.display()))?;
    let issues = verify_experiment_manifest(root, &manifest, required_workflows);
    let ok = issues.is_empty();
    let mut payload = json!({
        "ok": ok,
        "manifest": manifest_path.display().to_string(),
        "experiment_count": manifest.experiments.len(),
        "required_workflows": required_workflows,
        "issue_count": issues.len(),
        "issues": issues.iter().map(|issue| {
            json!({
                "experiment_id": issue.experiment_id,
                "kind": issue.kind,
                "detail": issue.detail,
            })
        }).collect::<Vec<_>>(),
    });
    if include_score {
        payload["scores"] = json!(score_experiment_manifest(root, &manifest, &issues));
    }
    Ok(payload)
}

pub(crate) fn verify_context_anomalies_payload(
    root: &Path,
    max_anomalies: usize,
    max_high: usize,
    max_trend_age_ms: Option<u64>,
) -> Result<serde_json::Value> {
    let digest = crate::cmd_dashboard::context_anomaly_digest(root)?;
    let high_count = digest
        .anomalies
        .iter()
        .filter(|anomaly| anomaly.severity == "high")
        .count();
    let mut ok = digest.anomaly_count <= max_anomalies && high_count <= max_high;
    let mut payload = json!({
        "ok": ok,
        "max_anomalies": max_anomalies,
        "max_high": max_high,
        "anomaly_count": digest.anomaly_count,
        "high_count": high_count,
        "anomalies": digest.anomalies,
    });
    if let Some(max_trend_age_ms) = max_trend_age_ms {
        let (latest_age_ms, oldest_recurring_hidden_age_ms) =
            crate::cmd_dashboard::context_anomaly_trend_age_summary(root)?;
        let trend_age_ok =
            latest_age_ms <= max_trend_age_ms && oldest_recurring_hidden_age_ms <= max_trend_age_ms;
        ok = ok && trend_age_ok;
        payload["ok"] = json!(ok);
        payload["max_trend_age_ms"] = json!(max_trend_age_ms);
        payload["trend_latest_age_ms"] = json!(latest_age_ms);
        payload["trend_oldest_recurring_hidden_age_ms"] = json!(oldest_recurring_hidden_age_ms);
        payload["trend_age_ok"] = json!(trend_age_ok);
        if !trend_age_ok {
            payload["trend_repair_hint"] =
                json!("rerun verifier or clear stale context anomaly history");
        }
    }
    Ok(payload)
}

pub(crate) fn verify_memory_lint_payload(
    root: &Path,
    fixture_path: &Path,
) -> Result<serde_json::Value> {
    let fixture_raw = std::fs::read_to_string(fixture_path)
        .with_context(|| format!("failed to read '{}'", fixture_path.display()))?;
    let fixture: MemoryLintFixture = serde_json::from_str(&fixture_raw)
        .with_context(|| format!("failed to parse '{}'", fixture_path.display()))?;
    let memories = fixture
        .memories
        .iter()
        .map(memory_lint_fixture_record)
        .collect::<Vec<_>>();
    let hook_events = fixture
        .hook_events
        .iter()
        .map(memory_lint_fixture_hook_event)
        .collect::<Vec<_>>();
    let report = crate::memory_store::lint_memory_records(root, &memories, &hook_events);
    let mut expectation_issues = Vec::new();
    if report.issue_count < fixture.expected_issue_count {
        expectation_issues.push(format!(
            "expected at least {} issue(s), got {}",
            fixture.expected_issue_count, report.issue_count
        ));
    }
    for kind in &fixture.expected_issue_kinds {
        if !report.issues.iter().any(|issue| issue.kind == *kind) {
            expectation_issues.push(format!("missing expected issue kind '{kind}'"));
        }
    }
    for clean_id in &fixture.clean_memory_ids {
        if report
            .issues
            .iter()
            .any(|issue| issue.memory_id == *clean_id)
        {
            expectation_issues.push(format!("clean memory {clean_id} produced an issue"));
        }
    }
    Ok(json!({
        "ok": expectation_issues.is_empty(),
        "fixture": fixture_path.display().to_string(),
        "memory_count": report.memory_count,
        "issue_count": report.issue_count,
        "expected_issue_count": fixture.expected_issue_count,
        "expected_issue_kinds": fixture.expected_issue_kinds,
        "clean_memory_ids": fixture.clean_memory_ids,
        "expectation_issues": expectation_issues,
        "lint": report,
    }))
}

fn memory_lint_fixture_record(
    memory: &MemoryLintFixtureMemory,
) -> crate::memory_store::MemoryRecord {
    crate::memory_store::MemoryRecord {
        id: memory.id,
        content: memory.content.clone(),
        tags: memory.tags.clone(),
        topic: memory.topic.clone(),
        importance: memory.importance.clone(),
        keywords: memory.keywords.clone(),
        project: memory.project.clone(),
        source: memory.source.clone(),
        raw_excerpt: memory.raw_excerpt.clone(),
        weight: 1.0,
        recall_score: None,
        created_at_unix_ms: 1,
        updated_at_unix_ms: 1,
    }
}

fn memory_lint_fixture_hook_event(
    event: &MemoryLintFixtureHookEvent,
) -> crate::memory_store::HookEventRecord {
    crate::memory_store::HookEventRecord {
        id: 0,
        runtime: event.runtime.clone(),
        event_kind: event.event_kind.clone(),
        session_id: None,
        task_id: None,
        matcher: None,
        payload_json: "{}".to_string(),
        created_at_unix_ms: 1,
    }
}

pub(crate) fn verify_reducer_drift_payload(fixture_path: &Path) -> Result<serde_json::Value> {
    let fixture_raw = std::fs::read_to_string(fixture_path)
        .with_context(|| format!("failed to read '{}'", fixture_path.display()))?;
    let fixture: ReducerDriftFixture = serde_json::from_str(&fixture_raw)
        .with_context(|| format!("failed to parse '{}'", fixture_path.display()))?;
    let mut issues = Vec::new();
    let mut summaries = Vec::new();

    if fixture.cases.is_empty() {
        issues.push(json!({
            "case_id": "<fixture>",
            "kind": "missing_cases",
            "detail": "fixture must contain at least one reducer drift case",
        }));
    }

    for (index, case) in fixture.cases.iter().enumerate() {
        let id = case.id.trim();
        let case_id = if id.is_empty() {
            format!("<case-{index}>")
        } else {
            id.to_string()
        };
        if id.is_empty() {
            issues.push(json!({
                "case_id": case_id,
                "kind": "missing_id",
                "detail": "case id is required",
            }));
        }
        if case.command_argv.is_empty() {
            issues.push(json!({
                "case_id": case_id,
                "kind": "missing_command_argv",
                "detail": "command_argv must contain the command and arguments",
            }));
            continue;
        }
        if case.required_markers.is_empty() {
            issues.push(json!({
                "case_id": case_id,
                "kind": "missing_required_markers",
                "detail": "required_markers must name reducer output that cannot drift away",
            }));
        }

        let command = case.command_argv.join(" ");
        let Some(spec) = classify_command_argv(&command, &case.command_argv) else {
            issues.push(json!({
                "case_id": case_id,
                "kind": "unclassified_command",
                "detail": command,
            }));
            continue;
        };
        let reduction = reduce_command_output(&spec, &case.stdout, &case.stderr, case.exit_code)?;
        let reduced_text = if reduction.compact_preview.trim().is_empty() {
            reduction.summary.clone()
        } else {
            format!("{}\n{}", reduction.summary, reduction.compact_preview)
        };
        for marker in &case.required_markers {
            let marker = marker.trim();
            if marker.is_empty() {
                issues.push(json!({
                    "case_id": case_id,
                    "kind": "empty_marker",
                    "detail": "required marker is empty",
                }));
            } else if !reduced_text.contains(marker) {
                issues.push(json!({
                    "case_id": case_id,
                    "kind": "missing_marker",
                    "detail": marker,
                }));
            }
        }
        summaries.push(json!({
            "case_id": case_id,
            "family": reduction.family,
            "canonical_kind": reduction.canonical_kind,
            "failed": reduction.failed,
            "exit_code": reduction.exit_code,
            "summary": reduction.summary,
            "compact_preview": reduction.compact_preview,
        }));
    }

    Ok(json!({
        "ok": issues.is_empty(),
        "fixture": fixture_path.display().to_string(),
        "case_count": fixture.cases.len(),
        "issue_count": issues.len(),
        "issues": issues,
        "summaries": summaries,
    }))
}

fn score_experiment_manifest(
    root: &Path,
    manifest: &ExperimentManifest,
    issues: &[ExperimentIssue],
) -> Vec<serde_json::Value> {
    manifest
        .experiments
        .iter()
        .map(|experiment| {
            let id = experiment.id.trim();
            let issue_id = if id.is_empty() { "<missing-id>" } else { id };
            let experiment_issues = issues
                .iter()
                .filter(|issue| issue.experiment_id == issue_id)
                .collect::<Vec<_>>();
            let artifact_text = experiment_artifact_text(root, experiment);
            let reproducibility = experiment_reproducibility_score(root, experiment);
            let artifact_backing =
                experiment_artifact_backing_score(experiment, &artifact_text, &experiment_issues);
            let fallback_clarity =
                experiment_fallback_clarity_score(experiment, &artifact_text, &experiment_issues);
            let freshness = experiment_freshness_score(experiment, &artifact_text);
            let score = ((reproducibility * 30
                + artifact_backing * 35
                + fallback_clarity * 15
                + freshness * 20)
                / 100)
                .min(100);
            json!({
                "id": issue_id,
                "workflow": experiment.workflow,
                "score": score,
                "passing": score >= 80 && experiment_issues.is_empty(),
                "reproducibility": reproducibility,
                "artifact_backing": artifact_backing,
                "fallback_clarity": fallback_clarity,
                "freshness": freshness,
                "issue_count": experiment_issues.len(),
            })
        })
        .collect()
}

fn experiment_reproducibility_score(root: &Path, experiment: &ExperimentEntry) -> u64 {
    let has_command = experiment
        .commands
        .iter()
        .any(|command| !command.trim().is_empty());
    if !has_command {
        return 0;
    }
    let missing_local_scripts = experiment
        .commands
        .iter()
        .filter_map(|command| local_script_command_path(command.trim()))
        .filter(|path| !root.join(path).exists())
        .count() as u64;
    let mut score = 100_u64.saturating_sub(missing_local_scripts.saturating_mul(35));
    if experiment.runtime_versions.is_empty() {
        score = score.saturating_sub(15);
    }
    score
}

fn experiment_artifact_backing_score(
    experiment: &ExperimentEntry,
    artifact_text: &str,
    issues: &[&ExperimentIssue],
) -> u64 {
    if experiment.artifacts.is_empty() {
        return 0;
    }
    let mut score = 100_u64;
    for issue in issues {
        match issue.kind.as_str() {
            "missing_artifact" => score = score.saturating_sub(45),
            "missing_metric_evidence" | "missing_metric_artifact_evidence" => {
                score = score.saturating_sub(30)
            }
            "missing_metric_name" | "missing_metric_value" => score = score.saturating_sub(20),
            "metric_below_min" | "metric_above_max" => score = score.saturating_sub(35),
            _ => {}
        }
    }
    let evidence_count = experiment
        .metrics
        .iter()
        .flat_map(|metric| &metric.evidence)
        .filter(|evidence| !evidence.trim().is_empty())
        .count();
    if evidence_count == 0 || artifact_text.trim().is_empty() {
        score = score.saturating_sub(35);
    }
    score
}

fn experiment_fallback_clarity_score(
    experiment: &ExperimentEntry,
    artifact_text: &str,
    issues: &[&ExperimentIssue],
) -> u64 {
    let mut score = 100_u64;
    for issue in issues {
        match issue.kind.as_str() {
            "unexpected_fallback" | "missing_fallback_artifact_evidence" => {
                score = score.saturating_sub(45)
            }
            _ => {}
        }
    }
    if experiment.allow_fallbacks {
        let reasons = experiment
            .fallback_reasons
            .iter()
            .filter(|reason| !reason.trim().is_empty())
            .count();
        if reasons == 0 {
            score = score.saturating_sub(30);
        }
        if artifact_text.trim().is_empty() {
            score = score.saturating_sub(20);
        }
    }
    score
}

fn experiment_freshness_score(experiment: &ExperimentEntry, artifact_text: &str) -> u64 {
    if artifact_text.trim().is_empty() {
        return 0;
    }
    let has_date_marker = artifact_text
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|part| {
            (part.len() == 8 || part.len() == 9 || part.len() == 15 || part.len() == 16)
                && part.starts_with("20")
                && part.chars().take(8).all(|ch| ch.is_ascii_digit())
        })
        || artifact_text.contains("20")
            && artifact_text.contains("-")
            && artifact_text.contains(":");
    let has_runtime_versions = !experiment.runtime_versions.is_empty();
    match (has_date_marker, has_runtime_versions) {
        (true, true) => 100,
        (true, false) => 85,
        (false, true) => 75,
        (false, false) => 55,
    }
}

fn experiment_artifact_text(root: &Path, experiment: &ExperimentEntry) -> String {
    experiment
        .artifacts
        .iter()
        .filter_map(|artifact| {
            let artifact = artifact.trim();
            if artifact.is_empty() {
                return None;
            }
            std::fs::read_to_string(root.join(artifact)).ok()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn verify_experiment_manifest(
    root: &Path,
    manifest: &ExperimentManifest,
    required_workflows: &[String],
) -> Vec<ExperimentIssue> {
    let mut issues = Vec::new();
    let mut seen_ids = BTreeSet::new();
    let covered_workflows = manifest
        .experiments
        .iter()
        .map(|experiment| experiment.workflow.trim())
        .filter(|workflow| !workflow.is_empty())
        .collect::<BTreeSet<_>>();
    for workflow in required_workflows {
        let workflow = workflow.trim();
        if !workflow.is_empty() && !covered_workflows.contains(workflow) {
            issues.push(ExperimentIssue {
                experiment_id: "<manifest>".to_string(),
                kind: "missing_required_workflow".to_string(),
                detail: workflow.to_string(),
            });
        }
    }
    if manifest.experiments.is_empty() {
        issues.push(ExperimentIssue {
            experiment_id: "<manifest>".to_string(),
            kind: "missing_experiments".to_string(),
            detail: "manifest must contain at least one experiment".to_string(),
        });
    }
    for experiment in &manifest.experiments {
        let id = experiment.id.trim();
        let issue_id = if id.is_empty() { "<missing-id>" } else { id };
        if id.is_empty() {
            issues.push(ExperimentIssue {
                experiment_id: issue_id.to_string(),
                kind: "missing_id".to_string(),
                detail: "experiment id is required".to_string(),
            });
        } else if !seen_ids.insert(id.to_string()) {
            issues.push(ExperimentIssue {
                experiment_id: issue_id.to_string(),
                kind: "duplicate_id".to_string(),
                detail: "experiment id appears more than once".to_string(),
            });
        }
        if experiment.workflow.trim().is_empty() {
            issues.push(ExperimentIssue {
                experiment_id: issue_id.to_string(),
                kind: "uncovered_workflow".to_string(),
                detail: "workflow must name the agent workflow this experiment covers".to_string(),
            });
        }
        if experiment
            .commands
            .iter()
            .all(|command| command.trim().is_empty())
        {
            issues.push(ExperimentIssue {
                experiment_id: issue_id.to_string(),
                kind: "missing_command_evidence".to_string(),
                detail: "at least one non-empty command is required".to_string(),
            });
        }
        for command in experiment
            .commands
            .iter()
            .map(|command| command.trim())
            .filter(|command| !command.is_empty())
        {
            if let Some(command_path) = local_script_command_path(command) {
                if !root.join(&command_path).exists() {
                    issues.push(ExperimentIssue {
                        experiment_id: issue_id.to_string(),
                        kind: "missing_command_path".to_string(),
                        detail: command_path,
                    });
                }
            }
        }
        if experiment.artifacts.is_empty() {
            issues.push(ExperimentIssue {
                experiment_id: issue_id.to_string(),
                kind: "missing_artifact".to_string(),
                detail: "at least one artifact path is required".to_string(),
            });
        }
        for artifact in &experiment.artifacts {
            let artifact = artifact.trim();
            if artifact.is_empty() {
                issues.push(ExperimentIssue {
                    experiment_id: issue_id.to_string(),
                    kind: "missing_artifact".to_string(),
                    detail: "artifact path is empty".to_string(),
                });
                continue;
            }
            if !root.join(artifact).exists() {
                issues.push(ExperimentIssue {
                    experiment_id: issue_id.to_string(),
                    kind: "missing_artifact".to_string(),
                    detail: artifact.to_string(),
                });
            }
        }
        let artifact_evidence = experiment
            .artifacts
            .iter()
            .filter_map(|artifact| {
                let artifact = artifact.trim();
                if artifact.is_empty() {
                    return None;
                }
                std::fs::read_to_string(root.join(artifact)).ok()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for metric in &experiment.metrics {
            let metric_name = metric.name.trim();
            if metric_name.is_empty() {
                issues.push(ExperimentIssue {
                    experiment_id: issue_id.to_string(),
                    kind: "missing_metric_name".to_string(),
                    detail: "metric name is required".to_string(),
                });
            }
            let Some(value) = metric.value else {
                issues.push(ExperimentIssue {
                    experiment_id: issue_id.to_string(),
                    kind: "missing_metric_value".to_string(),
                    detail: metric_name.to_string(),
                });
                continue;
            };
            if let Some(min) = metric.min {
                if value < min {
                    issues.push(ExperimentIssue {
                        experiment_id: issue_id.to_string(),
                        kind: "metric_below_min".to_string(),
                        detail: format!("{metric_name} value={value} min={min}"),
                    });
                }
            }
            if let Some(max) = metric.max {
                if value > max {
                    issues.push(ExperimentIssue {
                        experiment_id: issue_id.to_string(),
                        kind: "metric_above_max".to_string(),
                        detail: format!("{metric_name} value={value} max={max}"),
                    });
                }
            }
            let evidence = metric
                .evidence
                .iter()
                .map(|evidence| evidence.trim())
                .filter(|evidence| !evidence.is_empty())
                .collect::<Vec<_>>();
            if evidence.is_empty() {
                issues.push(ExperimentIssue {
                    experiment_id: issue_id.to_string(),
                    kind: "missing_metric_evidence".to_string(),
                    detail: metric_name.to_string(),
                });
            }
            for evidence in evidence {
                if !artifact_evidence.contains(evidence) {
                    issues.push(ExperimentIssue {
                        experiment_id: issue_id.to_string(),
                        kind: "missing_metric_artifact_evidence".to_string(),
                        detail: format!("{metric_name}: {evidence}"),
                    });
                }
            }
        }
        for runtime in &experiment.runtime_versions {
            if runtime.name.trim().is_empty() || runtime.version.trim().is_empty() {
                issues.push(ExperimentIssue {
                    experiment_id: issue_id.to_string(),
                    kind: "missing_runtime_version".to_string(),
                    detail: "runtime version entries require non-empty name and version"
                        .to_string(),
                });
            }
        }
        if !experiment.allow_fallbacks && !experiment.fallback_reasons.is_empty() {
            issues.push(ExperimentIssue {
                experiment_id: issue_id.to_string(),
                kind: "unexpected_fallback".to_string(),
                detail: experiment.fallback_reasons.join("; "),
            });
        }
        if experiment.allow_fallbacks {
            for reason in experiment
                .fallback_reasons
                .iter()
                .map(|reason| reason.trim())
                .filter(|reason| !reason.is_empty())
            {
                if !artifact_evidence.contains(reason) {
                    issues.push(ExperimentIssue {
                        experiment_id: issue_id.to_string(),
                        kind: "missing_fallback_artifact_evidence".to_string(),
                        detail: reason.to_string(),
                    });
                }
            }
        }
    }
    issues
}

fn local_script_command_path(command: &str) -> Option<String> {
    let first = command.split_whitespace().next()?.trim_matches(['"', '\'']);
    if first.contains('<') || first.contains('$') {
        return None;
    }
    let looks_like_experiment_script = first.starts_with("docs/experiments/")
        || first.starts_with("./docs/experiments/")
        || first.starts_with("scripts/")
        || first.starts_with("./scripts/");
    if looks_like_experiment_script {
        Some(first.trim_start_matches("./").to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_experiments_score_passes_backed_artifact_under_compact_budget() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("docs/experiments")).unwrap();
        std::fs::write(
            root.path().join("docs/experiments/evidence.md"),
            "20260515T120000Z\nPassed: 1\nPacket28 0.2.59\n",
        )
        .unwrap();
        let manifest_path = root.path().join("docs/experiments/manifest.json");
        std::fs::write(
            &manifest_path,
            r#"{
              "experiments": [{
                "id": "score-pass",
                "workflow": "Score passing workflow",
                "commands": ["Packet28 verify experiments --json"],
                "artifacts": ["docs/experiments/evidence.md"],
                "metrics": [{
                  "name": "passed",
                  "value": 1,
                  "min": 1,
                  "evidence": ["Passed: 1"]
                }],
                "runtime_versions": [{"name": "Packet28", "version": "0.2.59"}]
              }]
            }"#,
        )
        .unwrap();

        let payload = verify_experiments_payload(root.path(), &manifest_path, &[], true).unwrap();
        assert_eq!(payload["ok"], true);
        let score = &payload["scores"][0];
        assert_eq!(score["passing"], true);
        assert_eq!(score["score"], 100);
        assert!(serde_json::to_string(score).unwrap().len() < 1024);
    }

    #[test]
    fn verify_experiments_score_fails_missing_evidence_below_threshold() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("docs/experiments")).unwrap();
        let manifest_path = root.path().join("docs/experiments/manifest.json");
        std::fs::write(
            &manifest_path,
            r#"{
              "experiments": [{
                "id": "score-missing-evidence",
                "workflow": "Score missing evidence workflow",
                "commands": ["Packet28 verify experiments --json"],
                "artifacts": ["docs/experiments/missing.md"],
                "metrics": [{
                  "name": "passed",
                  "value": 1,
                  "min": 1,
                  "evidence": ["Passed: 1"]
                }],
                "runtime_versions": [{"name": "Packet28", "version": "0.2.59"}]
              }]
            }"#,
        )
        .unwrap();

        let payload = verify_experiments_payload(root.path(), &manifest_path, &[], true).unwrap();
        assert_eq!(payload["ok"], false);
        let score = &payload["scores"][0];
        assert_eq!(score["passing"], false);
        assert!(score["score"].as_u64().unwrap() < 80);
        assert!(serde_json::to_string(score).unwrap().len() < 1024);
    }

    #[test]
    fn verify_reducer_drift_keeps_failing_test_markers_under_compact_budget() {
        let root = tempfile::tempdir().unwrap();
        let fixture_path = root.path().join("reducer-drift.json");
        std::fs::write(
            &fixture_path,
            r#"{
              "cases": [{
                "id": "cargo-failing-test",
                "command_argv": ["cargo", "test", "-p", "suite-cli", "drift_marker"],
                "stdout": "running 1 test\ntest drift_marker ... FAILED\n\nfailures:\n\n---- drift_marker stdout ----\nthread 'drift_marker' panicked at crates/suite-cli/src/lib.rs:12:5:\nassertion failed: left == right\n\nfailures:\n    drift_marker\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n",
                "stderr": "",
                "exit_code": 101,
                "required_markers": [
                  "cargo test reported 0 passed and 1 failed",
                  "FAIL drift_marker"
                ]
              }]
            }"#,
        )
        .unwrap();

        let payload = verify_reducer_drift_payload(&fixture_path).unwrap();
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["issue_count"], 0);
        assert!(
            serde_json::to_string(&payload["summaries"][0])
                .unwrap()
                .len()
                < 1024
        );
    }

    #[test]
    fn verify_reducer_drift_flags_missing_marker() {
        let root = tempfile::tempdir().unwrap();
        let fixture_path = root.path().join("reducer-drift.json");
        std::fs::write(
            &fixture_path,
            r#"{
              "cases": [{
                "id": "removed-failing-line",
                "command_argv": ["cargo", "test", "removed_failure"],
                "stdout": "running 1 test\ntest removed_failure ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n",
                "stderr": "",
                "exit_code": 101,
                "required_markers": ["FAIL removed_failure"]
              }]
            }"#,
        )
        .unwrap();

        let payload = verify_reducer_drift_payload(&fixture_path).unwrap();
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["issues"][0]["kind"], "missing_marker");
        assert_eq!(payload["issues"][0]["detail"], "FAIL removed_failure");
    }

    #[test]
    fn verify_memory_lint_fixture_flags_stale_runtime_memory_only() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        std::fs::write(root.path().join("docs/current.md"), "ok").unwrap();
        let fixture_path = root.path().join("memory-lint.json");
        std::fs::write(
            &fixture_path,
            r#"{
              "memories": [
                {
                  "id": 1,
                  "content": "Windsurf must use transparent rewrite hooks documented in docs/missing.md",
                  "tags": "agent-specific",
                  "topic": "runtime",
                  "importance": "medium",
                  "source": "fixture"
                },
                {
                  "id": 2,
                  "content": "Project reducers preserve raw artifacts; see docs/current.md for evidence.",
                  "topic": "general",
                  "importance": "medium",
                  "source": "fixture"
                }
              ],
              "expected_issue_count": 3,
              "expected_issue_kinds": [
                "runtime_specific_memory",
                "stale_path",
                "unsupported_runtime_assumption"
              ],
              "clean_memory_ids": [2]
            }"#,
        )
        .unwrap();

        let payload = verify_memory_lint_payload(root.path(), &fixture_path).unwrap();
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["issue_count"], 3);
        assert!(payload["expectation_issues"].as_array().unwrap().is_empty());
        assert!(serde_json::to_string(&payload["lint"]).unwrap().len() < 768);
    }

    #[test]
    fn verify_context_anomalies_fails_high_anomaly_threshold() {
        let root = tempfile::tempdir().unwrap();
        crate::cmd_dashboard::record_memory_lint_history(
            root.path(),
            &json!({
                "ok": false,
                "memory_count": 1,
                "issue_count": 1,
                "lint": {
                    "issues": [{
                        "kind": "runtime_specific_memory",
                        "detail": "mentions windsurf"
                    }]
                }
            }),
        )
        .unwrap();

        let payload = verify_context_anomalies_payload(root.path(), usize::MAX, 0, None).unwrap();

        assert_eq!(payload["ok"], false);
        assert_eq!(payload["anomaly_count"], 1);
        assert_eq!(payload["high_count"], 1);
        assert!(payload["anomalies"][0]["next_check"]
            .as_str()
            .unwrap()
            .contains("memory-lint"));
        assert!(payload["anomalies"][0]["repair_hint"]
            .as_str()
            .unwrap()
            .contains("stale runtime"));
        assert!(serde_json::to_string(&payload).unwrap().len() < 1024);
    }

    #[test]
    fn verify_context_anomalies_fails_stale_trend_age_threshold() {
        let root = tempfile::tempdir().unwrap();
        let history_path = root
            .path()
            .join(".packet28")
            .join("context-anomaly-history.jsonl");
        std::fs::create_dir_all(history_path.parent().unwrap()).unwrap();
        std::fs::write(
            &history_path,
            [
                r#"{"created_at_unix_ms":1,"ok":true,"anomaly_count":3,"high_count":0,"hidden_categories":["fallback_provenance"]}"#,
                r#"{"created_at_unix_ms":2,"ok":true,"anomaly_count":3,"high_count":0,"hidden_categories":["fallback_provenance"]}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let payload =
            verify_context_anomalies_payload(root.path(), usize::MAX, usize::MAX, Some(1)).unwrap();

        assert_eq!(payload["ok"], false);
        assert_eq!(payload["max_trend_age_ms"], 1);
        assert_eq!(payload["trend_age_ok"], false);
        assert_eq!(
            payload["trend_repair_hint"],
            "rerun verifier or clear stale context anomaly history"
        );
        assert!(payload["trend_repair_hint"].as_str().unwrap().len() < 120);
        assert!(payload["trend_latest_age_ms"].as_u64().unwrap() > 1);
        assert!(
            payload["trend_oldest_recurring_hidden_age_ms"]
                .as_u64()
                .unwrap()
                > 1
        );
        assert!(serde_json::to_string(&payload).unwrap().len() < 1024);
    }
}
