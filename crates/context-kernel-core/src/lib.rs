//! Compatibility facade for the Packet28 context kernel.
//!
//! Existing `0.2` callers keep using this crate and its root API unchanged.
//! New custom compositions can depend on `context-kernel-mechanism`; the
//! supported Packet28 catalog is owned by `context-kernel-builtins`.

pub use context_kernel_builtins::{
    build_diff_analyze_envelope, build_diff_pipeline_request, build_test_impact_envelope, execute,
    execute_sequence, load_packet_file, normalize_sequence_request, register_v1_reducers,
    render_instruction, BudgetMetric, BudgetStage, BudgetUsage, CacheRuntimeMetrics,
    DiffAnalyzeKernelInput, DiffAnalyzeKernelOutput, ExecutionBudget, ExecutionContext,
    GovernanceAudit, ImpactKernelInput, ImpactKernelOutput, InstructionSummaryPayload,
    InstructionSummaryRequest, Kernel, KernelAudit, KernelError, KernelFailure, KernelPacket,
    KernelRequest, KernelResponse, KernelSequenceRequest, KernelSequenceResponse,
    KernelStepReactiveConfig, KernelStepRequest, KernelStepResponse, NoopSequenceObserver,
    PersistConfig, ReactiveReplanMode, ReactiveSequenceConfig, ReducerExecutionAudit,
    ReducerResult, RenderedInstructionSummary, SequenceObserver, SerializableFileDiff,
    DEFAULT_INSTRUCTION_SUMMARY_BUDGET_TOKENS, INSTRUCTION_SUMMARY_SCHEMA_VERSION,
};
