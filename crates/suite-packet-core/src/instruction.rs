use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::AgentSnapshotPayload;

/// Version of the instruction-cache experiment telemetry schema.
pub const INSTRUCTION_CACHE_TELEMETRY_SCHEMA_V1: &str = "packet28.instruction_cache_experiment.v1";
/// Only deterministic instruction renderer version implemented by this build.
pub const INSTRUCTION_RENDERER_VERSION: u32 = 1;

/// Selects how Packet28 handles an instruction source.
///
/// Passthrough is intentionally the default. Stable and adaptive rewriting are
/// experiment variants that must be selected explicitly by a request or
/// repository experiment configuration.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum InstructionRenderMode {
    /// Return the source bytes unchanged.
    #[default]
    Passthrough,
    /// Render from source, path, schema, budget, and stable configuration only.
    Stable,
    /// Include active task and snapshot state as an experimental comparator.
    Adaptive,
}

impl InstructionRenderMode {
    /// Returns the stable serialized spelling used in hashes and telemetry.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passthrough => "passthrough",
            Self::Stable => "stable",
            Self::Adaptive => "adaptive",
        }
    }

    /// Parses an explicit configuration value.
    #[must_use]
    pub fn from_config_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "passthrough" => Some(Self::Passthrough),
            "stable" => Some(Self::Stable),
            "adaptive" => Some(Self::Adaptive),
            _ => None,
        }
    }
}

/// Stable repository-owned inputs to instruction rendering.
///
/// Values are normalized before rendering or hashing so semantically
/// equivalent configurations have the same identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct InstructionStableConfig {
    /// Version of the deterministic renderer behavior.
    pub renderer_version: u32,
    /// Maximum number of Markdown sections in a rewritten instruction.
    pub max_sections: usize,
    /// Maximum number of excerpt lines selected from each section.
    pub max_lines_per_section: usize,
    /// Maximum number of stable or adaptive focus terms.
    pub max_focus_terms: usize,
    /// Repository-defined terms that are stable across tasks and workers.
    pub focus_terms: Vec<String>,
}

impl Default for InstructionStableConfig {
    fn default() -> Self {
        Self {
            renderer_version: INSTRUCTION_RENDERER_VERSION,
            max_sections: 4,
            max_lines_per_section: 6,
            max_focus_terms: 12,
            focus_terms: Vec::new(),
        }
    }
}

impl InstructionStableConfig {
    /// Returns the bounded canonical form used by rendering and cache identity.
    #[must_use]
    pub fn normalized(&self) -> Self {
        let mut focus_terms = self
            .focus_terms
            .iter()
            .map(|term| term.trim().to_ascii_lowercase())
            .filter(|term| !term.is_empty())
            .collect::<Vec<_>>();
        focus_terms.sort();
        focus_terms.dedup();

        let max_focus_terms = self.max_focus_terms.clamp(1, 64);
        focus_terms.truncate(max_focus_terms);

        Self {
            renderer_version: INSTRUCTION_RENDERER_VERSION,
            max_sections: self.max_sections.clamp(1, 16),
            max_lines_per_section: self.max_lines_per_section.clamp(1, 32),
            max_focus_terms,
            focus_terms,
        }
    }

    /// Returns whether this build implements the requested renderer version.
    #[must_use]
    pub const fn has_supported_renderer_version(&self) -> bool {
        self.renderer_version == INSTRUCTION_RENDERER_VERSION
    }

    /// Computes a deterministic SHA-256 identity for the normalized config.
    #[must_use]
    pub fn fingerprint_sha256(&self) -> String {
        let normalized = self.normalized();
        let mut canonical = format!(
            "renderer_version={}\nmax_sections={}\nmax_lines_per_section={}\nmax_focus_terms={}\n",
            normalized.renderer_version,
            normalized.max_sections,
            normalized.max_lines_per_section,
            normalized.max_focus_terms
        );
        for term in normalized.focus_terms {
            canonical.push_str("focus_term=");
            canonical.push_str(&term);
            canonical.push('\n');
        }
        format!("{:x}", Sha256::digest(canonical.as_bytes()))
    }
}

/// One required transition in the controlled instruction-cache experiment.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum InstructionExperimentScenario {
    /// No prior renderer or provider cache state is available.
    ColdStart,
    /// The same source is requested again without intervening state changes.
    SecondRequest,
    /// Mutable context was compacted and may need provider-cache rewarming.
    Compaction,
    /// The active task changed from task A to task B.
    TaskSwitch,
    /// The active task stayed fixed while its snapshot changed.
    SnapshotDrift,
    /// A new worker receives the same stable instruction plus a mutable brief.
    FreshWorkerHandoff,
}

/// Distinguishes an observed provider value from an explicit evidence gap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InstructionMeasurement<T> {
    /// The provider or harness reported a value.
    Observed {
        /// The observed value.
        value: T,
    },
    /// The value was not available in this run.
    Unknown {
        /// Why the value could not be observed.
        reason: String,
    },
}

impl<T> InstructionMeasurement<T> {
    /// Creates an observed measurement.
    pub fn observed(value: T) -> Self {
        Self::Observed { value }
    }

    /// Creates an explicitly unknown measurement.
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self::Unknown {
            reason: reason.into(),
        }
    }

    /// Returns the value only when it was observed.
    #[must_use]
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Observed { value } => Some(value),
            Self::Unknown { .. } => None,
        }
    }
}

impl<T> Default for InstructionMeasurement<T> {
    fn default() -> Self {
        Self::unknown("not_observed")
    }
}

/// Per-request record for the controlled instruction-cache experiment.
///
/// Provider cache creation/read tokens and costs are separate fields. Local
/// renderer cache hits are not promoted into provider-cache claims.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstructionCacheTelemetryV1 {
    /// Exact telemetry schema identifier.
    pub schema: String,
    /// Stable identifier shared by all records from one controlled run.
    pub run_id: String,
    /// Zero-based request order inside the run.
    pub request_index: u64,
    /// State transition represented by this request.
    pub scenario: InstructionExperimentScenario,
    /// Instruction rendering mode used for this request.
    pub mode: InstructionRenderMode,
    /// SHA-256 of the source bytes.
    pub source_sha256: String,
    /// SHA-256 of the emitted instruction-prefix bytes.
    pub rendered_prefix_sha256: String,
    /// SHA-256 of normalized stable configuration.
    pub stable_config_sha256: String,
    /// Canonical adaptive snapshot identity, or why it was inapplicable.
    pub snapshot_sha256: InstructionMeasurement<String>,
    /// Whether this rendering mode was eligible for Packet28's local cache.
    pub renderer_cache_eligible: bool,
    /// Whether Packet28's local deterministic renderer cache was reused.
    pub renderer_cache_hit: bool,
    /// Emitted instruction size before any provider tokenization.
    pub local_prefix_bytes: u64,
    /// Conservative byte-based local token estimate.
    pub local_prefix_est_tokens: u64,
    /// Provider name/version, when an actual provider request was observed.
    pub provider: InstructionMeasurement<String>,
    /// Provider version, when reported independently from its name.
    pub provider_version: InstructionMeasurement<String>,
    /// Provider prompt-component order and cache boundary metadata.
    pub provider_prompt_order: InstructionMeasurement<Vec<String>>,
    /// Provider cache breakpoint or boundary metadata.
    pub provider_cache_boundary: InstructionMeasurement<String>,
    /// Provider-reported cache creation tokens.
    pub cache_creation_tokens: InstructionMeasurement<u64>,
    /// Provider-reported cache read tokens.
    pub cache_read_tokens: InstructionMeasurement<u64>,
    /// Provider-reported cache creation cost in USD.
    pub cache_creation_cost_usd: InstructionMeasurement<f64>,
    /// Provider-reported cache read cost in USD.
    pub cache_read_cost_usd: InstructionMeasurement<f64>,
    /// Tokens recreated specifically after compaction.
    pub compaction_rewarm_tokens: InstructionMeasurement<u64>,
    /// Whether the request satisfied the run's adherence check.
    pub instruction_adherence: InstructionMeasurement<bool>,
    /// Method used for the adherence check.
    pub instruction_adherence_method: InstructionMeasurement<String>,
}

/// Aggregate metrics required by the controlled instruction-cache experiment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstructionCacheMetricsV1 {
    /// `creation / (creation + read)`.
    pub churn_rate: InstructionMeasurement<f64>,
    /// `read / max(creation, 1)`.
    pub reuse_multiple: InstructionMeasurement<f64>,
    /// `creation_cost + read_cost`.
    pub effective_cache_cost_usd: InstructionMeasurement<f64>,
    /// Provider-reported tokens recreated after compaction.
    pub compaction_rewarm_tokens: InstructionMeasurement<u64>,
}

impl InstructionCacheMetricsV1 {
    /// Computes aggregate metrics without substituting estimates for unknown
    /// provider observations.
    #[must_use]
    pub fn from_records(records: &[InstructionCacheTelemetryV1]) -> Self {
        if records.is_empty() {
            let reason = "no instruction-cache experiment records were observed";
            return Self {
                churn_rate: InstructionMeasurement::unknown(reason),
                reuse_multiple: InstructionMeasurement::unknown(reason),
                effective_cache_cost_usd: InstructionMeasurement::unknown(reason),
                compaction_rewarm_tokens: InstructionMeasurement::unknown(reason),
            };
        }

        let creation_tokens = sum_observed_u64(
            records,
            |record| &record.cache_creation_tokens,
            "cache creation tokens",
        );
        let read_tokens = sum_observed_u64(
            records,
            |record| &record.cache_read_tokens,
            "cache read tokens",
        );
        let churn_rate = match (creation_tokens.value(), read_tokens.value()) {
            (Some(creation), Some(read)) if creation.saturating_add(*read) > 0 => {
                InstructionMeasurement::observed(
                    *creation as f64 / creation.saturating_add(*read) as f64,
                )
            }
            (Some(_), Some(_)) => {
                InstructionMeasurement::unknown("creation plus read tokens is zero")
            }
            _ => InstructionMeasurement::unknown(
                "provider cache creation/read token observations are incomplete",
            ),
        };
        let reuse_multiple = match (creation_tokens.value(), read_tokens.value()) {
            (Some(creation), Some(read)) => {
                InstructionMeasurement::observed(*read as f64 / (*creation).max(1) as f64)
            }
            _ => InstructionMeasurement::unknown(
                "provider cache creation/read token observations are incomplete",
            ),
        };

        let creation_cost = sum_observed_f64(
            records,
            |record| &record.cache_creation_cost_usd,
            "cache creation cost",
        );
        let read_cost = sum_observed_f64(
            records,
            |record| &record.cache_read_cost_usd,
            "cache read cost",
        );
        let effective_cache_cost_usd = match (creation_cost.value(), read_cost.value()) {
            (Some(creation), Some(read)) => InstructionMeasurement::observed(*creation + *read),
            _ => InstructionMeasurement::unknown(
                "provider cache creation/read cost observations are incomplete",
            ),
        };

        let compaction_records = records
            .iter()
            .filter(|record| record.scenario == InstructionExperimentScenario::Compaction)
            .collect::<Vec<_>>();
        let compaction_rewarm_tokens = if compaction_records.is_empty() {
            InstructionMeasurement::unknown("no compaction scenario was recorded")
        } else {
            sum_observed_u64(
                compaction_records.iter().copied(),
                |record| &record.compaction_rewarm_tokens,
                "compaction rewarm tokens",
            )
        };

        Self {
            churn_rate,
            reuse_multiple,
            effective_cache_cost_usd,
            compaction_rewarm_tokens,
        }
    }
}

/// Returns a canonical SHA-256 fingerprint for adaptive snapshot identity.
///
/// Only the bounded snapshot fields that influence adaptive rendering are
/// included. Object keys are sorted recursively and array order remains
/// meaningful. Unrelated snapshot counters, tool history, and decision state
/// cannot create apparent instruction-prefix drift.
///
/// # Errors
///
/// Returns an error if the snapshot cannot be represented as JSON.
pub fn instruction_snapshot_sha256(
    snapshot: &AgentSnapshotPayload,
) -> Result<String, serde_json::Error> {
    let mut value = serde_json::json!({
        "focus_paths": snapshot.focus_paths.iter().take(6).collect::<Vec<_>>(),
        "focus_symbols": snapshot.focus_symbols.iter().take(8).collect::<Vec<_>>(),
        "open_question_text": snapshot
            .open_questions
            .iter()
            .take(4)
            .map(|question| &question.text)
            .collect::<Vec<_>>(),
    });
    canonicalize_json(&mut value);
    let bytes = serde_json::to_vec(&value)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn canonicalize_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                canonicalize_json(value);
            }
        }
        serde_json::Value::Object(values) => {
            let mut entries = std::mem::take(values).into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, mut value) in entries {
                canonicalize_json(&mut value);
                values.insert(key, value);
            }
        }
        _ => {}
    }
}

fn sum_observed_u64<'a>(
    records: impl IntoIterator<Item = &'a InstructionCacheTelemetryV1>,
    select: impl Fn(&'a InstructionCacheTelemetryV1) -> &'a InstructionMeasurement<u64>,
    label: &str,
) -> InstructionMeasurement<u64> {
    let mut total = 0_u64;
    for record in records {
        let Some(value) = select(record).value() else {
            return InstructionMeasurement::unknown(format!("{label} are incomplete"));
        };
        total = total.saturating_add(*value);
    }
    InstructionMeasurement::observed(total)
}

fn sum_observed_f64<'a>(
    records: impl IntoIterator<Item = &'a InstructionCacheTelemetryV1>,
    select: impl Fn(&'a InstructionCacheTelemetryV1) -> &'a InstructionMeasurement<f64>,
    label: &str,
) -> InstructionMeasurement<f64> {
    let mut total = 0.0_f64;
    for record in records {
        let Some(value) = select(record).value() else {
            return InstructionMeasurement::unknown(format!("{label} is incomplete"));
        };
        total += *value;
    }
    InstructionMeasurement::observed(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        scenario: InstructionExperimentScenario,
        creation: u64,
        read: u64,
        creation_cost: f64,
        read_cost: f64,
        rewarm: u64,
    ) -> InstructionCacheTelemetryV1 {
        InstructionCacheTelemetryV1 {
            schema: INSTRUCTION_CACHE_TELEMETRY_SCHEMA_V1.to_string(),
            run_id: "run-1".to_string(),
            request_index: 0,
            scenario,
            mode: InstructionRenderMode::Stable,
            source_sha256: "source".to_string(),
            rendered_prefix_sha256: "prefix".to_string(),
            stable_config_sha256: "config".to_string(),
            snapshot_sha256: InstructionMeasurement::unknown(
                "stable mode has no adaptive snapshot",
            ),
            renderer_cache_eligible: true,
            renderer_cache_hit: false,
            local_prefix_bytes: 128,
            local_prefix_est_tokens: 32,
            provider: InstructionMeasurement::observed("fixture".to_string()),
            provider_version: InstructionMeasurement::observed("fixture-v1".to_string()),
            provider_prompt_order: InstructionMeasurement::observed(vec![
                "instructions".to_string(),
                "brief".to_string(),
            ]),
            provider_cache_boundary: InstructionMeasurement::observed(
                "after_instructions".to_string(),
            ),
            cache_creation_tokens: InstructionMeasurement::observed(creation),
            cache_read_tokens: InstructionMeasurement::observed(read),
            cache_creation_cost_usd: InstructionMeasurement::observed(creation_cost),
            cache_read_cost_usd: InstructionMeasurement::observed(read_cost),
            compaction_rewarm_tokens: InstructionMeasurement::observed(rewarm),
            instruction_adherence: InstructionMeasurement::observed(true),
            instruction_adherence_method: InstructionMeasurement::observed(
                "fixture_assertion".to_string(),
            ),
        }
    }

    #[test]
    fn instruction_mode_defaults_to_passthrough() {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Request {
            mode: InstructionRenderMode,
        }

        assert_eq!(
            serde_json::from_str::<Request>("{}").unwrap().mode,
            InstructionRenderMode::Passthrough
        );
    }

    #[test]
    fn stable_config_fingerprint_ignores_term_order_and_case() {
        let left = InstructionStableConfig {
            focus_terms: vec!["Auth".to_string(), "cache".to_string()],
            ..InstructionStableConfig::default()
        };
        let right = InstructionStableConfig {
            focus_terms: vec![
                "CACHE".to_string(),
                " auth ".to_string(),
                "auth".to_string(),
            ],
            ..InstructionStableConfig::default()
        };

        assert_eq!(left.fingerprint_sha256(), right.fingerprint_sha256());
    }

    #[test]
    fn stable_config_normalization_bounds_focus_terms() {
        let config = InstructionStableConfig {
            max_focus_terms: 2,
            focus_terms: vec![
                "gamma".to_string(),
                "alpha".to_string(),
                "beta".to_string(),
                "ALPHA".to_string(),
            ],
            ..InstructionStableConfig::default()
        };

        let normalized = config.normalized();
        assert_eq!(normalized.max_focus_terms, 2);
        assert_eq!(normalized.focus_terms, ["alpha", "beta"]);
    }

    #[test]
    fn stable_config_normalizes_unsupported_version_label() {
        let unsupported = InstructionStableConfig {
            renderer_version: 99,
            ..InstructionStableConfig::default()
        };
        assert!(!unsupported.has_supported_renderer_version());
        assert_eq!(
            unsupported.normalized().renderer_version,
            INSTRUCTION_RENDERER_VERSION
        );
    }

    #[test]
    fn snapshot_fingerprint_changes_when_adaptive_state_changes() {
        let first = AgentSnapshotPayload {
            task_id: "task-a".to_string(),
            focus_paths: vec!["src/a.rs".to_string()],
            ..AgentSnapshotPayload::default()
        };
        let second = AgentSnapshotPayload {
            focus_paths: vec!["src/b.rs".to_string()],
            ..first.clone()
        };

        assert_ne!(
            instruction_snapshot_sha256(&first).unwrap(),
            instruction_snapshot_sha256(&second).unwrap()
        );
    }

    #[test]
    fn snapshot_fingerprint_ignores_state_that_cannot_affect_rendering() {
        let first = AgentSnapshotPayload {
            task_id: "task-a".to_string(),
            focus_paths: vec!["src/a.rs".to_string()],
            event_count: 1,
            ..AgentSnapshotPayload::default()
        };
        let second = AgentSnapshotPayload {
            event_count: 99,
            files_read: vec!["README.md".to_string()],
            ..first.clone()
        };

        assert_eq!(
            instruction_snapshot_sha256(&first).unwrap(),
            instruction_snapshot_sha256(&second).unwrap()
        );
    }

    #[test]
    fn telemetry_json_preserves_explicit_unknown_reason() {
        let value = serde_json::to_value(record(
            InstructionExperimentScenario::ColdStart,
            10,
            0,
            0.01,
            0.0,
            0,
        ))
        .unwrap();

        assert_eq!(
            value["snapshot_sha256"]["reason"],
            "stable mode has no adaptive snapshot"
        );
    }

    #[test]
    fn experiment_metrics_use_creation_and_read_values_separately() {
        let records = vec![
            record(
                InstructionExperimentScenario::ColdStart,
                100,
                0,
                0.02,
                0.0,
                0,
            ),
            record(
                InstructionExperimentScenario::Compaction,
                20,
                180,
                0.004,
                0.006,
                20,
            ),
        ];

        assert_eq!(
            InstructionCacheMetricsV1::from_records(&records),
            InstructionCacheMetricsV1 {
                churn_rate: InstructionMeasurement::observed(0.4),
                reuse_multiple: InstructionMeasurement::observed(1.5),
                effective_cache_cost_usd: InstructionMeasurement::observed(0.03),
                compaction_rewarm_tokens: InstructionMeasurement::observed(20),
            }
        );
    }

    #[test]
    fn experiment_metrics_remain_unknown_without_provider_observations() {
        let mut unknown = record(InstructionExperimentScenario::ColdStart, 0, 0, 0.0, 0.0, 0);
        unknown.cache_creation_tokens = InstructionMeasurement::unknown("local renderer run only");

        assert!(matches!(
            InstructionCacheMetricsV1::from_records(&[unknown]).churn_rate,
            InstructionMeasurement::Unknown { .. }
        ));
    }

    #[test]
    fn experiment_metrics_preserve_observed_zeroes() {
        let metrics = InstructionCacheMetricsV1::from_records(&[record(
            InstructionExperimentScenario::Compaction,
            0,
            0,
            0.0,
            0.0,
            0,
        )]);

        assert!(matches!(
            metrics.churn_rate,
            InstructionMeasurement::Unknown { .. }
        ));
        assert_eq!(
            metrics.reuse_multiple,
            InstructionMeasurement::observed(0.0)
        );
        assert_eq!(
            metrics.effective_cache_cost_usd,
            InstructionMeasurement::observed(0.0)
        );
        assert_eq!(
            metrics.compaction_rewarm_tokens,
            InstructionMeasurement::observed(0)
        );
    }

    #[test]
    fn empty_experiment_does_not_report_observed_zero_metrics() {
        let metrics = InstructionCacheMetricsV1::from_records(&[]);

        assert!(matches!(
            metrics.churn_rate,
            InstructionMeasurement::Unknown { .. }
        ));
        assert!(matches!(
            metrics.reuse_multiple,
            InstructionMeasurement::Unknown { .. }
        ));
        assert!(matches!(
            metrics.effective_cache_cost_usd,
            InstructionMeasurement::Unknown { .. }
        ));
        assert!(matches!(
            metrics.compaction_rewarm_tokens,
            InstructionMeasurement::Unknown { .. }
        ));
    }
}
