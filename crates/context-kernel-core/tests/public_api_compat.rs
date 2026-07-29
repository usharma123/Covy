use std::path::Path;
use std::time::Duration;

use context_kernel_core::{
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
use context_memory_core::{
    ContextStoreListFilter, ContextStorePaging, ContextStorePruneRequest, NoopDeltaReuseHooks,
    RecallOptions,
};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn legacy_root_types_and_functions_remain_nameable() {
    assert_send_sync::<Kernel>();
    assert_send_sync::<ExecutionContext>();
    assert_send_sync::<KernelError>();

    let _constants = (
        DEFAULT_INSTRUCTION_SUMMARY_BUDGET_TOKENS,
        INSTRUCTION_SUMMARY_SCHEMA_VERSION,
    );
    let _wire_defaults = (
        BudgetUsage::default(),
        ExecutionBudget::default(),
        GovernanceAudit::default(),
        KernelPacket::default(),
        KernelRequest::default(),
        KernelSequenceRequest::default(),
        KernelStepReactiveConfig::default(),
        KernelStepRequest::default(),
        ReactiveSequenceConfig::default(),
        ReducerExecutionAudit::default(),
        ReducerResult::default(),
    );
    let _kernel_audit: Option<KernelAudit> = None;
    let _kernel_failure: Option<KernelFailure> = None;
    let _kernel_response: Option<KernelResponse> = None;
    let _sequence_response: Option<KernelSequenceResponse> = None;
    let _step_response: Option<KernelStepResponse> = None;
    let _local_defaults = (
        CacheRuntimeMetrics::default(),
        DiffAnalyzeKernelOutput::default(),
        ImpactKernelOutput::default(),
        InstructionSummaryPayload::default(),
        InstructionSummaryRequest::default(),
        NoopSequenceObserver,
    );
    let _local_names: (Option<DiffAnalyzeKernelInput>, Option<ImpactKernelInput>) = (None, None);
    let _enums = (
        BudgetStage::Input,
        BudgetMetric::Tokens,
        ReactiveReplanMode::Basic,
    );

    let _load: fn(&Path) -> Result<KernelPacket, KernelError> = load_packet_file;
    let _execute: fn(KernelRequest) -> Result<KernelResponse, KernelError> = execute;
    let _execute_sequence: fn(
        KernelSequenceRequest,
    ) -> Result<KernelSequenceResponse, KernelError> = execute_sequence;
    let _normalize: fn(KernelSequenceRequest) -> Result<KernelSequenceRequest, KernelError> =
        normalize_sequence_request;
    let _register: fn(&mut Kernel) = register_v1_reducers;
    let _diff_request = build_diff_pipeline_request;
    let _diff_envelope = build_diff_analyze_envelope;
    let _impact_envelope = build_test_impact_envelope;
    let _render = render_instruction;
    let _serialized: Option<SerializableFileDiff> = None;
    let _rendered: Option<RenderedInstructionSummary> = None;

    let direct: context_kernel_builtins::Kernel = Kernel::new();
    let legacy: Kernel = context_kernel_builtins::Kernel::new();
    drop((direct, legacy));
}

#[test]
fn all_four_legacy_constructors_and_instance_methods_remain_available() {
    let directory = tempfile::tempdir().unwrap();
    let persistence = PersistConfig::new(directory.path().to_path_buf());

    let mut empty = Kernel::new();
    Kernel::register_reducer(&mut empty, "compat.noop", |ctx, _packets| {
        ctx.set_shared("compat", serde_json::Value::Bool(true));
        let _ = ctx.shared_value("compat");
        let _ = ctx.shared_json();
        let _ = ctx.cache_entries()?;
        let _ = ctx.cache_recall("", &RecallOptions::default())?;
        let _ = ctx.cache_related_entries(None, &[], &[], &[])?;
        Ok(ReducerResult::default())
    });
    assert_eq!(Kernel::reducer_names(&empty), vec!["compat.noop"]);
    let _ = Kernel::execute(
        &empty,
        KernelRequest {
            target: "compat.noop".to_string(),
            ..KernelRequest::default()
        },
    );
    let mut hooks = NoopDeltaReuseHooks;
    let _ = Kernel::execute_with_hooks(
        &empty,
        KernelRequest {
            target: "compat.noop".to_string(),
            ..KernelRequest::default()
        },
        &mut hooks,
    );
    let _ = Kernel::execute_sequence(&empty, KernelSequenceRequest::default());
    let mut observer = NoopSequenceObserver;
    let _ = Kernel::execute_sequence_with_observer(
        &empty,
        KernelSequenceRequest::default(),
        &mut observer as &mut dyn SequenceObserver,
    );
    let _ = Kernel::cache_runtime_metrics(&empty);
    let _ = Kernel::context_store_list(
        &empty,
        &ContextStoreListFilter::default(),
        &ContextStorePaging::default(),
    );
    let _ = Kernel::context_store_get(&empty, "missing");
    let _ = Kernel::context_store_stats(&empty);
    let _ = Kernel::context_store_recall(&empty, "", &RecallOptions::default());

    let builtins = Kernel::with_v1_reducers();
    assert_eq!(Kernel::reducer_names(&builtins).len(), 16);

    let persistent = Kernel::with_persistence(persistence.clone());
    let _ = Kernel::flush_cache_persistence(&persistent, Duration::from_millis(1));
    let _ = Kernel::context_store_prune(
        &persistent,
        ContextStorePruneRequest::default(),
        Duration::from_secs(1),
    );

    let persistent_builtins = Kernel::with_v1_reducers_and_persistence(PersistConfig::new(
        directory.path().join("builtins"),
    ));
    assert_eq!(Kernel::reducer_names(&persistent_builtins).len(), 16);
    let _ = Kernel::shutdown_cache_persistence(&persistent_builtins, Duration::from_secs(2));
}

#[test]
fn legacy_error_variants_and_structured_mapping_remain_available() {
    let errors = [
        KernelError::EmptyTarget,
        KernelError::UnknownTarget {
            target: "target".to_string(),
            registered: vec!["registered".to_string()],
        },
        KernelError::InvalidRequest {
            detail: "detail".to_string(),
        },
        KernelError::BudgetExceeded {
            target: "compat.noop".to_string(),
            stage: BudgetStage::Total,
            metric: BudgetMetric::Bytes,
            used: 2,
            cap: 1,
        },
        KernelError::PacketRead {
            path: "packet.json".to_string(),
            detail: "detail".to_string(),
        },
        KernelError::PacketParse {
            path: "packet.json".to_string(),
            detail: "detail".to_string(),
        },
        KernelError::ReducerFailed {
            target: "compat.noop".to_string(),
            detail: "detail".to_string(),
        },
        KernelError::SchedulerFailed {
            detail: "detail".to_string(),
        },
        KernelError::CacheLock {
            detail: "detail".to_string(),
        },
        KernelError::CachePersistence {
            detail: "detail".to_string(),
        },
        KernelError::PolicyViolation {
            target: "compat.noop".to_string(),
            detail: "detail".to_string(),
        },
        KernelError::SequenceCancelled {
            task_id: Some("task".to_string()),
        },
    ];

    assert!(errors
        .into_iter()
        .all(|error| !error.structured().code.is_empty()));
}
