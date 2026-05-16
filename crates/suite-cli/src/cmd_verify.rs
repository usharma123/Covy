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
    let payload = verify_experiments_payload(&root, &manifest_path, &args.require_workflows)?;
    let ok = payload.get("ok").and_then(serde_json::Value::as_bool) == Some(true);

    if args.json {
        crate::cmd_common::emit_json(&payload, args.pretty)?;
    } else {
        let experiment_count = payload
            .get("experiment_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        if ok {
            println!(
                "{experiment_count} experiment(s) verified from {}",
                manifest_path.display()
            );
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
) -> Result<serde_json::Value> {
    let manifest_raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read '{}'", manifest_path.display()))?;
    let manifest: ExperimentManifest = serde_json::from_str(&manifest_raw)
        .with_context(|| format!("failed to parse '{}'", manifest_path.display()))?;
    let issues = verify_experiment_manifest(root, &manifest, required_workflows);
    let ok = issues.is_empty();
    Ok(json!({
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
    }))
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
