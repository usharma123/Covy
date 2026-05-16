use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
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
}
