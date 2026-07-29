//! Versioned packet contracts shared by Packet28 producers and consumers.
//!
//! The crate keeps wire data independent of runtime implementations. Common
//! packet types remain available at the crate root for compatibility, while
//! subsystem-specific contracts such as kernel, memory, search, and governance
//! stay under their named modules.
//!
//! # Canonical envelopes
//!
//! ```
//! use serde_json::json;
//! use suite_packet_core::EnvelopeV1;
//!
//! let envelope = EnvelopeV1 {
//!     tool: "example".into(),
//!     kind: "example.result".into(),
//!     summary: "one deterministic result".into(),
//!     payload: json!({"count": 1}),
//!     ..EnvelopeV1::default()
//! }
//! .with_canonical_hash();
//!
//! assert_eq!(envelope.version, "1");
//! assert_eq!(envelope.hash.len(), 64);
//! ```
//!
//! # Namespaced contracts
//!
//! ```
//! use suite_packet_core::kernel::KernelRequest;
//! use suite_packet_core::search::SearchRequest;
//!
//! let kernel = KernelRequest::default();
//! let search = SearchRequest::default();
//! assert!(kernel.target.is_empty());
//! assert!(search.query.is_empty());
//! ```
//!
//! Subsystem-specific contracts are deliberately not added to the
//! compatibility root surface:
//!
//! ```compile_fail
//! use suite_packet_core::KernelRequest;
//! ```
#![doc = include_str!("../PUBLIC_API.md")]

extern crate packet28_binary_codec as wincode;

/// Agent activity and snapshot packet contracts.
pub mod agent;
/// Context correlation and context-management packet contracts.
pub mod context;
/// Coverage, diff, quality-gate, and repository snapshot contracts.
pub mod coverage;
/// Diagnostics and issue packet contracts.
pub mod diagnostics;
/// Compatibility namespace for diff packet types.
pub mod diff;
/// Versioned packet envelopes, references, provenance, and hashing.
pub mod envelope;
/// Errors shared by packet serialization and artifact operations.
pub mod error;
/// Test-impact and quality-gate packet contracts.
pub mod gate;
/// Governance audit packet contracts.
pub mod governance;
/// Instruction rendering and cache experiment contracts.
pub mod instruction;
/// Context-kernel wire requests, responses, budgets, and audit contracts.
pub mod kernel;
/// Machine-readable wrappers and packet artifact storage.
pub mod machine;
/// Context memory and packet-cache contracts.
pub mod memory;
/// Packet merge summaries.
pub mod merge;
/// Stable packet-type registry and schema snapshots.
pub mod registry;
/// Search request, result, and statistics contracts.
pub mod search;
/// Universal task and shard-planning contracts.
pub mod shard;
/// Sparse test-map and timing-history contracts.
pub mod testmap;

pub use agent::{
    AgentDecision, AgentIntention, AgentQuestion, AgentSnapshotPayload, AgentStateEventData,
    AgentStateEventKind, AgentStateEventPayload, SearchQuerySummary, ToolFailureSummary,
    ToolInvocationSummary, ToolKindSuccess, ToolOperationKind, ToolPathSummary,
};
pub use context::{
    ContextCorrelationFinding, ContextCorrelationPayload, ContextManageBudgetSummary,
    ContextManagePacketRef, ContextManagePayload, ContextManageRecommendedAction,
    CorrelationEvidenceRef, MemoryKind, MemorySourceTier,
};
pub use coverage::{
    CoverageData, CoverageFormat, DiffStatus, FileCoverage, FileDiff, IssueGateCounts,
    QualityGateResult, RepoSnapshot,
};
pub use diagnostics::{DiagnosticsData, DiagnosticsFormat, Issue, Severity};
pub use envelope::{
    canonical_hash_json, envelope_json_bytes, estimate_tokens_from_bytes, BudgetCost, EnvelopeV1,
    FileRef, Provenance, RiskLevel, SymbolRef,
};
pub use error::CovyError;
pub use gate::{ImpactPlan, ImpactResult, PlannedTest, UncoveredBlock};
pub use instruction::{
    instruction_snapshot_sha256, InstructionCacheMetricsV1, InstructionCacheTelemetryV1,
    InstructionExperimentScenario, InstructionMeasurement, InstructionRenderMode,
    InstructionStableConfig, INSTRUCTION_CACHE_TELEMETRY_SCHEMA_V1, INSTRUCTION_RENDERER_VERSION,
};
pub use machine::{
    artifact_path, artifact_store_root, read_packet_artifact, wrap_envelope, write_packet_artifact,
    ArtifactHandle, JsonProfile, PacketWrapperV1, ARTIFACT_DIR, MACHINE_SCHEMA_VERSION,
};
pub use merge::MergeSummary;
pub use registry::{
    packet_contract, packet_contracts, packet_type_schema_snapshot, wrapper_schema_snapshot,
    PacketTypeContract, PACKET_TYPE_AGENT_SNAPSHOT, PACKET_TYPE_AGENT_STATE,
    PACKET_TYPE_BUILD_REDUCE, PACKET_TYPE_CONTEXT_ASSEMBLE, PACKET_TYPE_CONTEXT_CORRELATE,
    PACKET_TYPE_CONTEXT_MANAGE, PACKET_TYPE_COVER_CHECK, PACKET_TYPE_DIFF_ANALYZE,
    PACKET_TYPE_GUARD_CHECK, PACKET_TYPE_MAP_QUERY, PACKET_TYPE_MAP_REPO, PACKET_TYPE_PROXY_RUN,
    PACKET_TYPE_STACK_SLICE, PACKET_TYPE_TEST_IMPACT,
};
pub use shard::{
    PlannedShard, PlannedTask, Shard, ShardPlan, Task, TaskSet, UniversalShardPlan,
    SHARD_PLAN_SCHEMA_VERSION, TASK_SCHEMA_VERSION,
};
pub use testmap::{
    SparseFileCoverage, SparseTestCoverageRow, TestMapIndex, TestMapMetadata, TestTimingHistory,
};
