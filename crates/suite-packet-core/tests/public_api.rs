use serde_json::{json, Value};
use suite_packet_core as packet;

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
    assert!(
        source.split("pub use ").skip(1).all(|tail| !tail
            .split(';')
            .next()
            .unwrap_or(tail)
            .contains('*')),
        "root glob re-exports can leak new implementation details without review"
    );
}
