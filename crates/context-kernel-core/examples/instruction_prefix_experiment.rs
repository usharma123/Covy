use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use context_kernel_core::{
    InstructionSummaryPayload, InstructionSummaryRequest, Kernel, KernelRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use suite_packet_core::{
    EnvelopeV1, InstructionCacheMetricsV1, InstructionCacheTelemetryV1,
    InstructionExperimentScenario, InstructionMeasurement, InstructionRenderMode,
    InstructionStableConfig, INSTRUCTION_CACHE_TELEMETRY_SCHEMA_V1,
};

const OUTPUT_SCHEMA: &str = "packet28.instruction_prefix_local_experiment.v1";
const MANIFEST_SCHEMA: &str = "packet28.instruction_prefix_experiment_manifest.v1";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExperimentManifest {
    schema: String,
    experiment_id: String,
    repetitions: usize,
    fixture_path: String,
    source_path: String,
    schema_version: u32,
    budget_tokens: u64,
    stable_config: InstructionStableConfig,
    modes: Vec<InstructionRenderMode>,
    scenarios: Vec<InstructionExperimentScenario>,
}

#[derive(Debug, Serialize)]
struct Assertion {
    name: String,
    passed: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct ExperimentOutput {
    schema: String,
    telemetry_schema: String,
    experiment_id: String,
    manifest_sha256: String,
    fixture_sha256: String,
    evidence_boundary: String,
    records: Vec<InstructionCacheTelemetryV1>,
    provider_metrics_by_mode: BTreeMap<String, InstructionCacheMetricsV1>,
    assertions: Vec<Assertion>,
    ok: bool,
}

fn main() -> Result<()> {
    let manifest_path = parse_manifest_path()?;
    let manifest_bytes = fs::read(&manifest_path)
        .with_context(|| format!("failed to read manifest '{}'", manifest_path.display()))?;
    let manifest: ExperimentManifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("invalid manifest '{}'", manifest_path.display()))?;
    validate_manifest(&manifest)?;
    let fixture_path = resolve_fixture_path(&manifest_path, &manifest.fixture_path);
    let source = fs::read_to_string(&fixture_path)
        .with_context(|| format!("failed to read fixture '{}'", fixture_path.display()))?;

    let mut records = Vec::new();
    let mut assertions = Vec::new();
    for repetition in 0..manifest.repetitions {
        for mode in &manifest.modes {
            let iteration = run_iteration(&manifest, &source, *mode, repetition)?;
            assertions.extend(assert_iteration(&iteration, *mode, repetition));
            records.extend(iteration);
        }
    }
    assertions.push(assert_all_stable_hashes_match(&records));

    let mut provider_metrics_by_mode = BTreeMap::new();
    for mode in &manifest.modes {
        let mode_records = records
            .iter()
            .filter(|record| record.mode == *mode)
            .cloned()
            .collect::<Vec<_>>();
        provider_metrics_by_mode.insert(
            mode.as_str().to_string(),
            InstructionCacheMetricsV1::from_records(&mode_records),
        );
    }
    let ok = assertions.iter().all(|assertion| assertion.passed);
    let output = ExperimentOutput {
        schema: OUTPUT_SCHEMA.to_string(),
        telemetry_schema: INSTRUCTION_CACHE_TELEMETRY_SCHEMA_V1.to_string(),
        experiment_id: manifest.experiment_id,
        manifest_sha256: sha256(&manifest_bytes),
        fixture_sha256: sha256(source.as_bytes()),
        evidence_boundary: "Local renderer/cache observations only. No provider request was made; provider prompt ordering, cache boundaries, creation/read tokens, costs, compaction rewarm, adherence, and net savings remain explicitly unknown.".to_string(),
        records,
        provider_metrics_by_mode,
        assertions,
        ok,
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    if !ok {
        bail!("instruction-prefix experiment invariant failed");
    }
    Ok(())
}

fn parse_manifest_path() -> Result<PathBuf> {
    let mut args = env::args().skip(1);
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--manifest"), Some(path), None) => Ok(PathBuf::from(path)),
        (Some("--help"), None, None) => {
            println!(
                "Usage: instruction_prefix_experiment --manifest <manifest.json>\n\
                 Runs the local passthrough/stable/adaptive renderer-cache experiment."
            );
            std::process::exit(0);
        }
        _ => bail!("expected exactly --manifest <manifest.json>"),
    }
}

fn resolve_fixture_path(manifest_path: &Path, fixture_path: &str) -> PathBuf {
    let fixture_path = PathBuf::from(fixture_path);
    if fixture_path.is_absolute() {
        fixture_path
    } else {
        manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(fixture_path)
    }
}

fn validate_manifest(manifest: &ExperimentManifest) -> Result<()> {
    if manifest.schema != MANIFEST_SCHEMA {
        bail!(
            "unsupported manifest schema '{}'; expected '{MANIFEST_SCHEMA}'",
            manifest.schema
        );
    }
    if manifest.repetitions < 2 {
        bail!("repetitions must be at least 2");
    }
    let actual_modes = manifest.modes.iter().copied().collect::<BTreeSet<_>>();
    let expected_modes = [
        InstructionRenderMode::Passthrough,
        InstructionRenderMode::Stable,
        InstructionRenderMode::Adaptive,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if actual_modes != expected_modes || manifest.modes.len() != expected_modes.len() {
        bail!("modes must contain passthrough, stable, and adaptive exactly once");
    }
    let expected_scenarios = vec![
        InstructionExperimentScenario::ColdStart,
        InstructionExperimentScenario::SecondRequest,
        InstructionExperimentScenario::Compaction,
        InstructionExperimentScenario::TaskSwitch,
        InstructionExperimentScenario::SnapshotDrift,
        InstructionExperimentScenario::FreshWorkerHandoff,
    ];
    if manifest.scenarios != expected_scenarios {
        bail!("scenarios must use the required six transitions in canonical order");
    }
    if manifest.source_path.trim().is_empty() {
        bail!("source_path must not be empty");
    }
    if manifest.schema_version == 0 {
        bail!("schema_version must be explicit");
    }
    if manifest.budget_tokens < 96 {
        bail!("budget_tokens must be at least the renderer minimum of 96");
    }
    Ok(())
}

fn run_iteration(
    manifest: &ExperimentManifest,
    source: &str,
    mode: InstructionRenderMode,
    repetition: usize,
) -> Result<Vec<InstructionCacheTelemetryV1>> {
    let run_id = format!(
        "{}-{}-r{}",
        manifest.experiment_id,
        mode.as_str(),
        repetition + 1
    );
    let mut kernel = Kernel::with_v1_reducers();
    write_focus(
        &kernel,
        "task-a",
        "focus-auth",
        1,
        "src/auth.rs",
        "authenticate",
    )?;
    let mut records = Vec::with_capacity(manifest.scenarios.len());

    for (request_index, scenario) in manifest.scenarios.iter().copied().enumerate() {
        let (task_id, backend_kind, agent_family) = match scenario {
            InstructionExperimentScenario::ColdStart
            | InstructionExperimentScenario::SecondRequest
            | InstructionExperimentScenario::Compaction => ("task-a", "linux_preload", "codex"),
            InstructionExperimentScenario::TaskSwitch => {
                write_focus(
                    &kernel,
                    "task-b",
                    "focus-cache",
                    2,
                    "src/cache.rs",
                    "cache_lookup",
                )?;
                ("task-b", "macos_swap", "claude")
            }
            InstructionExperimentScenario::SnapshotDrift => {
                write_focus(
                    &kernel,
                    "task-b",
                    "focus-release",
                    3,
                    "src/release.rs",
                    "package_release",
                )?;
                ("task-b", "macos_swap", "claude")
            }
            InstructionExperimentScenario::FreshWorkerHandoff => {
                kernel = Kernel::with_v1_reducers();
                write_focus(
                    &kernel,
                    "task-b",
                    "fresh-focus-cache",
                    1,
                    "src/cache.rs",
                    "cache_lookup",
                )?;
                write_focus(
                    &kernel,
                    "task-b",
                    "fresh-focus-release",
                    2,
                    "src/release.rs",
                    "package_release",
                )?;
                ("task-b", "macos_swap", "claude")
            }
        };

        let response = kernel.execute(KernelRequest {
            target: "packet28.instruction.summarize".to_string(),
            reducer_input: serde_json::to_value(InstructionSummaryRequest {
                path: manifest.source_path.clone(),
                content: source.to_string(),
                content_sha256: "caller-hash-is-not-trusted".to_string(),
                mode,
                stable_config: manifest.stable_config.clone(),
                task_id: Some(task_id.to_string()),
                budget_tokens: Some(manifest.budget_tokens),
                schema_version: manifest.schema_version,
                source_kind: Some("instruction_file".to_string()),
                backend_kind: Some(backend_kind.to_string()),
                agent_family: Some(agent_family.to_string()),
            })?,
            policy_context: json!({
                "instruction_mode": mode,
                "task_id": task_id,
            }),
            ..KernelRequest::default()
        })?;
        let envelope: EnvelopeV1<InstructionSummaryPayload> =
            serde_json::from_value(response.output_packets[0].body.clone())?;
        let payload = envelope.payload;
        let renderer_cache_hit = response.metadata["cache"]["hit"].as_bool().unwrap_or(false);
        let provider_reason = "local renderer-only experiment; no provider request was made";
        let prompt_reason =
            "provider prompt assembly and cache breakpoint metadata were not observed";
        let telemetry_reason =
            "provider cache creation/read telemetry was not available in the local run";
        let adherence_reason =
            "instruction adherence requires a provider response and was not measured locally";
        let snapshot_sha256 = payload
            .snapshot_sha256
            .clone()
            .map(InstructionMeasurement::observed)
            .unwrap_or_else(|| {
                InstructionMeasurement::unknown(match mode {
                    InstructionRenderMode::Adaptive => {
                        "adaptive request had no render-relevant snapshot"
                    }
                    _ => "snapshot identity is inapplicable outside adaptive mode",
                })
            });
        records.push(InstructionCacheTelemetryV1 {
            schema: INSTRUCTION_CACHE_TELEMETRY_SCHEMA_V1.to_string(),
            run_id: run_id.clone(),
            request_index: request_index as u64,
            scenario,
            mode,
            source_sha256: payload.content_sha256,
            rendered_prefix_sha256: payload.rendered_sha256,
            stable_config_sha256: payload.stable_config_sha256,
            snapshot_sha256,
            renderer_cache_eligible: mode == InstructionRenderMode::Stable,
            renderer_cache_hit,
            local_prefix_bytes: payload.summary_text.len() as u64,
            local_prefix_est_tokens: payload.summary_text.len().div_ceil(4) as u64,
            provider: InstructionMeasurement::unknown(provider_reason),
            provider_version: InstructionMeasurement::unknown(provider_reason),
            provider_prompt_order: InstructionMeasurement::unknown(prompt_reason),
            provider_cache_boundary: InstructionMeasurement::unknown(prompt_reason),
            cache_creation_tokens: InstructionMeasurement::unknown(telemetry_reason),
            cache_read_tokens: InstructionMeasurement::unknown(telemetry_reason),
            cache_creation_cost_usd: InstructionMeasurement::unknown(telemetry_reason),
            cache_read_cost_usd: InstructionMeasurement::unknown(telemetry_reason),
            compaction_rewarm_tokens: InstructionMeasurement::unknown(
                "provider compaction rewarm telemetry was not available in the local run",
            ),
            instruction_adherence: InstructionMeasurement::unknown(adherence_reason),
            instruction_adherence_method: InstructionMeasurement::unknown(adherence_reason),
        });
    }

    Ok(records)
}

fn write_focus(
    kernel: &Kernel,
    task_id: &str,
    event_id: &str,
    occurred_at_unix: u64,
    path: &str,
    symbol: &str,
) -> Result<()> {
    kernel.execute(KernelRequest {
        target: "agenty.state.write".to_string(),
        reducer_input: json!({
            "task_id": task_id,
            "event_id": event_id,
            "occurred_at_unix": occurred_at_unix,
            "actor": "instruction-prefix-experiment",
            "kind": "focus_set",
            "paths": [path],
            "symbols": [symbol],
            "data": {"type": "focus_set"}
        }),
        ..KernelRequest::default()
    })?;
    Ok(())
}

fn assert_iteration(
    records: &[InstructionCacheTelemetryV1],
    mode: InstructionRenderMode,
    repetition: usize,
) -> Vec<Assertion> {
    let expected_hits = match mode {
        InstructionRenderMode::Stable => [false, true, true, true, true, false],
        InstructionRenderMode::Passthrough | InstructionRenderMode::Adaptive => [false; 6],
    };
    let actual_hits = records
        .iter()
        .map(|record| record.renderer_cache_hit)
        .collect::<Vec<_>>();
    let expected_eligibility = mode == InstructionRenderMode::Stable;
    let actual_eligibility = records
        .iter()
        .map(|record| record.renderer_cache_eligible)
        .collect::<Vec<_>>();
    let mut assertions = vec![Assertion {
        name: format!("{}_r{}_local_cache_pattern", mode.as_str(), repetition + 1),
        passed: actual_hits == expected_hits
            && actual_eligibility
                .iter()
                .all(|eligible| *eligible == expected_eligibility),
        detail: format!(
            "expected_hits={expected_hits:?} actual_hits={actual_hits:?} \
             expected_eligible={expected_eligibility} actual_eligible={actual_eligibility:?}"
        ),
    }];

    match mode {
        InstructionRenderMode::Passthrough => {
            let exact = records.iter().all(|record| {
                record.source_sha256 == record.rendered_prefix_sha256
                    && matches!(
                        record.snapshot_sha256,
                        InstructionMeasurement::Unknown { .. }
                    )
            });
            assertions.push(Assertion {
                name: format!("passthrough_r{}_exact_source_bytes", repetition + 1),
                passed: exact,
                detail: "source and rendered hashes must match for all six transitions".to_string(),
            });
        }
        InstructionRenderMode::Stable => {
            let hashes = records
                .iter()
                .map(|record| &record.rendered_prefix_sha256)
                .collect::<BTreeSet<_>>();
            assertions.push(Assertion {
                name: format!("stable_r{}_byte_identity", repetition + 1),
                passed: hashes.len() == 1,
                detail: format!("unique_rendered_prefix_hashes={}", hashes.len()),
            });
        }
        InstructionRenderMode::Adaptive => {
            let hash_for = |scenario| {
                records
                    .iter()
                    .find(|record| record.scenario == scenario)
                    .map(|record| record.rendered_prefix_sha256.as_str())
            };
            let second = hash_for(InstructionExperimentScenario::SecondRequest);
            let compacted = hash_for(InstructionExperimentScenario::Compaction);
            let task_switch = hash_for(InstructionExperimentScenario::TaskSwitch);
            let drift = hash_for(InstructionExperimentScenario::SnapshotDrift);
            let fresh = hash_for(InstructionExperimentScenario::FreshWorkerHandoff);
            assertions.push(Assertion {
                name: format!("adaptive_r{}_transition_identity", repetition + 1),
                passed: second == compacted
                    && compacted != task_switch
                    && task_switch != drift
                    && drift == fresh,
                detail: "compaction without render-relevant drift stays fixed; task A→B and same-task snapshot drift change bytes; a fresh worker reproduces the latest snapshot bytes".to_string(),
            });
        }
    }
    assertions
}

fn assert_all_stable_hashes_match(records: &[InstructionCacheTelemetryV1]) -> Assertion {
    let hashes = records
        .iter()
        .filter(|record| record.mode == InstructionRenderMode::Stable)
        .map(|record| &record.rendered_prefix_sha256)
        .collect::<BTreeSet<_>>();
    Assertion {
        name: "stable_prefix_is_byte_identical_across_all_repetitions".to_string(),
        passed: hashes.len() == 1,
        detail: format!("unique_rendered_prefix_hashes={}", hashes.len()),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
