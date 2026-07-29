use std::collections::BTreeSet;

use serde_json::{json, Value};
use suite_packet_core as packet;

const OPERATIONAL_DOCTEST_FAMILIES: &[&str] =
    &["envelope-hash", "machine-artifact", "schema-registry"];
const OPERATIONAL_MODULES: &[&str] = &["envelope", "machine", "registry"];
const INTENTIONALLY_EXCLUDED_MODULES: &[(&str, &str)] = &[
    ("agent", "serde and agent packet fixtures"),
    ("context", "correlation and context-management fixtures"),
    ("coverage", "coverage and diff packet fixtures"),
    (
        "diagnostics",
        "data contract without an operational entrypoint",
    ),
    ("error", "error taxonomy exercised by fallible entrypoints"),
    ("gate", "coverage and test-impact fixtures"),
    ("governance", "governance packet fixtures"),
    (
        "instruction",
        "telemetry data contract outside the packet registry",
    ),
    (
        "kernel",
        "implementation-free wire request and response DTOs",
    ),
    ("memory", "implementation-free memory wire DTOs"),
    (
        "merge",
        "summary data contract without an operational entrypoint",
    ),
    ("search", "implementation-free search wire DTOs"),
    ("shard", "task and shard DTOs with separate schema versions"),
    ("testmap", "test-map persistence and index DTOs"),
];
const COMPATIBILITY_ONLY_MODULES: &[(&str, &str)] =
    &[("diff", "legacy namespace for coverage diff types")];
const ROOT_COMPATIBILITY_EXPORTS: &[&str] = &[
    "agent::{AgentDecision,AgentIntention,AgentQuestion,AgentSnapshotPayload,AgentStateEventData,AgentStateEventKind,AgentStateEventPayload,SearchQuerySummary,ToolFailureSummary,ToolInvocationSummary,ToolKindSuccess,ToolOperationKind,ToolPathSummary}",
    "context::{ContextCorrelationFinding,ContextCorrelationPayload,ContextManageBudgetSummary,ContextManagePacketRef,ContextManagePayload,ContextManageRecommendedAction,CorrelationEvidenceRef,MemoryKind,MemorySourceTier}",
    "coverage::{CoverageData,CoverageFormat,DiffStatus,FileCoverage,FileDiff,IssueGateCounts,QualityGateResult,RepoSnapshot}",
    "diagnostics::{DiagnosticsData,DiagnosticsFormat,Issue,Severity}",
    "envelope::{canonical_hash_json,envelope_json_bytes,estimate_tokens_from_bytes,BudgetCost,EnvelopeV1,FileRef,Provenance,RiskLevel,SymbolRef}",
    "error::CovyError",
    "gate::{ImpactPlan,ImpactResult,PlannedTest,UncoveredBlock}",
    "instruction::{instruction_snapshot_sha256,InstructionCacheMetricsV1,InstructionCacheTelemetryV1,InstructionExperimentScenario,InstructionMeasurement,InstructionRenderMode,InstructionStableConfig,INSTRUCTION_CACHE_TELEMETRY_SCHEMA_V1,INSTRUCTION_RENDERER_VERSION}",
    "machine::{artifact_path,artifact_store_root,read_packet_artifact,wrap_envelope,write_packet_artifact,ArtifactHandle,JsonProfile,PacketWrapperV1,ARTIFACT_DIR,MACHINE_SCHEMA_VERSION}",
    "merge::MergeSummary",
    "registry::{packet_contract,packet_contracts,packet_type_schema_snapshot,wrapper_schema_snapshot,PacketTypeContract,PACKET_TYPE_AGENT_SNAPSHOT,PACKET_TYPE_AGENT_STATE,PACKET_TYPE_BUILD_REDUCE,PACKET_TYPE_CONTEXT_ASSEMBLE,PACKET_TYPE_CONTEXT_CORRELATE,PACKET_TYPE_CONTEXT_MANAGE,PACKET_TYPE_COVER_CHECK,PACKET_TYPE_DIFF_ANALYZE,PACKET_TYPE_GUARD_CHECK,PACKET_TYPE_MAP_QUERY,PACKET_TYPE_MAP_REPO,PACKET_TYPE_PROXY_RUN,PACKET_TYPE_STACK_SLICE,PACKET_TYPE_TEST_IMPACT}",
    "shard::{PlannedShard,PlannedTask,Shard,ShardPlan,Task,TaskSet,UniversalShardPlan,SHARD_PLAN_SCHEMA_VERSION,TASK_SCHEMA_VERSION}",
    "testmap::{SparseFileCoverage,SparseTestCoverageRow,TestMapIndex,TestMapMetadata,TestTimingHistory}",
];

fn assert_public_type<T>() {}

#[test]
fn compatibility_root_exports_remain_available() {
    assert_public_type::<packet::ToolOperationKind>();
    assert_public_type::<packet::ToolInvocationSummary>();
    assert_public_type::<packet::ToolFailureSummary>();
    assert_public_type::<packet::ToolPathSummary>();
    assert_public_type::<packet::SearchQuerySummary>();
    assert_public_type::<packet::AgentIntention>();
    assert_public_type::<packet::ToolKindSuccess>();
    assert_public_type::<packet::AgentStateEventKind>();
    assert_public_type::<packet::AgentStateEventData>();
    assert_public_type::<packet::AgentStateEventPayload>();
    assert_public_type::<packet::AgentDecision>();
    assert_public_type::<packet::AgentQuestion>();
    assert_public_type::<packet::AgentSnapshotPayload>();

    assert_public_type::<packet::MemorySourceTier>();
    assert_public_type::<packet::MemoryKind>();
    assert_public_type::<packet::CorrelationEvidenceRef>();
    assert_public_type::<packet::ContextCorrelationFinding>();
    assert_public_type::<packet::ContextCorrelationPayload>();
    assert_public_type::<packet::ContextManagePacketRef>();
    assert_public_type::<packet::ContextManageBudgetSummary>();
    assert_public_type::<packet::ContextManageRecommendedAction>();
    assert_public_type::<packet::ContextManagePayload>();

    assert_public_type::<packet::CoverageFormat>();
    assert_public_type::<packet::FileCoverage>();
    assert_public_type::<packet::CoverageData>();
    assert_public_type::<packet::DiffStatus>();
    assert_public_type::<packet::FileDiff>();
    assert_public_type::<packet::IssueGateCounts>();
    assert_public_type::<packet::QualityGateResult>();
    assert_public_type::<packet::RepoSnapshot>();
    assert_public_type::<packet::Severity>();
    assert_public_type::<packet::Issue>();
    assert_public_type::<packet::DiagnosticsFormat>();
    assert_public_type::<packet::DiagnosticsData>();

    assert_public_type::<packet::RiskLevel>();
    assert_public_type::<packet::FileRef>();
    assert_public_type::<packet::SymbolRef>();
    assert_public_type::<packet::BudgetCost>();
    assert_public_type::<packet::Provenance>();
    assert_public_type::<packet::EnvelopeV1<Value>>();
    assert_public_type::<packet::CovyError>();
    assert_public_type::<packet::ImpactResult>();
    assert_public_type::<packet::PlannedTest>();
    assert_public_type::<packet::UncoveredBlock>();
    assert_public_type::<packet::ImpactPlan>();

    assert_public_type::<packet::InstructionRenderMode>();
    assert_public_type::<packet::InstructionStableConfig>();
    assert_public_type::<packet::InstructionExperimentScenario>();
    assert_public_type::<packet::InstructionMeasurement<Value>>();
    assert_public_type::<packet::InstructionCacheTelemetryV1>();
    assert_public_type::<packet::InstructionCacheMetricsV1>();

    assert_public_type::<packet::JsonProfile>();
    assert_public_type::<packet::PacketWrapperV1<Value>>();
    assert_public_type::<packet::ArtifactHandle>();
    assert_public_type::<packet::MergeSummary>();
    assert_public_type::<packet::PacketTypeContract>();

    assert_public_type::<packet::Task>();
    assert_public_type::<packet::TaskSet>();
    assert_public_type::<packet::PlannedTask>();
    assert_public_type::<packet::PlannedShard>();
    assert_public_type::<packet::UniversalShardPlan>();
    assert_public_type::<packet::Shard>();
    assert_public_type::<packet::ShardPlan>();

    assert_public_type::<packet::TestMapMetadata>();
    assert_public_type::<packet::SparseFileCoverage>();
    assert_public_type::<packet::SparseTestCoverageRow>();
    assert_public_type::<packet::TestMapIndex>();
    assert_public_type::<packet::TestTimingHistory>();

    let value = json!({"value": 1});
    assert_eq!(packet::envelope_json_bytes(&value), 11);
    assert_eq!(packet::estimate_tokens_from_bytes(11), 2);
    assert_eq!(packet::canonical_hash_json(&value).len(), 64);
    let snapshot = packet::AgentSnapshotPayload::default();
    assert_eq!(
        packet::instruction_snapshot_sha256(&snapshot).unwrap(),
        packet::instruction_snapshot_sha256(&snapshot).unwrap()
    );

    let envelope = packet::EnvelopeV1 {
        payload: value,
        ..packet::EnvelopeV1::default()
    };
    let wrapper = packet::wrap_envelope("example.packet.v1", envelope);
    assert_eq!(wrapper.schema_version, packet::MACHINE_SCHEMA_VERSION);
    assert_eq!(
        packet::artifact_store_root(std::path::Path::new(".")),
        std::path::Path::new(".").join(packet::ARTIFACT_DIR)
    );
    assert_eq!(
        packet::artifact_path(std::path::Path::new("."), "handle"),
        std::path::Path::new(".")
            .join(packet::ARTIFACT_DIR)
            .join("handle.json")
    );
    let _write = packet::write_packet_artifact::<Value>;
    let _read = packet::read_packet_artifact;

    assert!(!packet::packet_contracts().is_empty());
    assert!(packet::packet_contract(packet::PACKET_TYPE_COVER_CHECK).is_some());
    assert!(packet::wrapper_schema_snapshot().is_object());
    assert!(
        packet::packet_type_schema_snapshot(packet::PACKET_TYPE_COVER_CHECK)
            .expect("registered packet type")
            .is_object()
    );

    let packet_types = [
        packet::PACKET_TYPE_COVER_CHECK,
        packet::PACKET_TYPE_DIFF_ANALYZE,
        packet::PACKET_TYPE_TEST_IMPACT,
        packet::PACKET_TYPE_AGENT_STATE,
        packet::PACKET_TYPE_AGENT_SNAPSHOT,
        packet::PACKET_TYPE_CONTEXT_CORRELATE,
        packet::PACKET_TYPE_CONTEXT_MANAGE,
        packet::PACKET_TYPE_STACK_SLICE,
        packet::PACKET_TYPE_BUILD_REDUCE,
        packet::PACKET_TYPE_MAP_REPO,
        packet::PACKET_TYPE_MAP_QUERY,
        packet::PACKET_TYPE_PROXY_RUN,
        packet::PACKET_TYPE_CONTEXT_ASSEMBLE,
        packet::PACKET_TYPE_GUARD_CHECK,
    ];
    assert_eq!(packet_types.len(), 14);
    assert_eq!(packet::TASK_SCHEMA_VERSION, 1);
    assert_eq!(packet::SHARD_PLAN_SCHEMA_VERSION, 1);
    assert_eq!(
        packet::INSTRUCTION_CACHE_TELEMETRY_SCHEMA_V1,
        "packet28.instruction_cache_experiment.v1"
    );
    assert_eq!(packet::INSTRUCTION_RENDERER_VERSION, 1);
}

#[test]
fn compatibility_root_uses_explicit_allowlists() {
    let source = include_str!("../src/lib.rs");
    let actual = root_reexports(source);
    let expected = ROOT_COMPATIBILITY_EXPORTS
        .iter()
        .map(|export| (*export).to_string())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        actual, expected,
        "root compatibility exports changed without updating the reviewed inventory"
    );
}

#[test]
fn every_public_module_has_a_reviewed_classification() {
    let source = include_str!("../src/lib.rs");
    let actual = source
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("pub mod ")
                .and_then(|tail| tail.strip_suffix(';'))
                .map(ToOwned::to_owned)
        })
        .collect::<BTreeSet<_>>();
    let expected = OPERATIONAL_MODULES
        .iter()
        .copied()
        .chain(
            INTENTIONALLY_EXCLUDED_MODULES
                .iter()
                .map(|(module, _)| *module),
        )
        .chain(COMPATIBILITY_ONLY_MODULES.iter().map(|(module, _)| *module))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();

    for (module, reason) in INTENTIONALLY_EXCLUDED_MODULES
        .iter()
        .chain(COMPATIBILITY_ONLY_MODULES)
    {
        assert!(
            !reason.trim().is_empty(),
            "excluded module '{module}' needs a review reason"
        );
    }
    assert_eq!(
        actual, expected,
        "classify every public module as operational, data-contract, or compatibility-only"
    );
}

#[test]
fn every_supported_operational_family_has_one_doctest_marker() {
    let docs = include_str!("../PUBLIC_API.md");

    for family in OPERATIONAL_DOCTEST_FAMILIES {
        let marker = format!("<!-- public-surface:{family} -->");
        assert_eq!(
            docs.match_indices(&marker).count(),
            1,
            "operational family '{family}' must have exactly one reviewed doctest section"
        );
    }
}

fn root_reexports(source: &str) -> BTreeSet<String> {
    let mut exports = BTreeSet::new();
    let mut current = None::<String>;

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(statement) = current.as_mut() {
            statement.push_str(trimmed);
            if trimmed.ends_with(';') {
                let completed = current.take().expect("active public use statement");
                exports.insert(normalize_export(&completed));
            }
        } else if let Some(tail) = trimmed.strip_prefix("pub use ") {
            if tail.ends_with(';') {
                exports.insert(normalize_export(tail));
            } else {
                current = Some(tail.to_string());
            }
        }
    }

    assert!(current.is_none(), "unterminated root public use statement");
    exports
}

fn normalize_export(statement: &str) -> String {
    let normalized = statement
        .trim_end_matches(';')
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    normalized.replace(",}", "}")
}
