use std::collections::BTreeSet;
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

const MECHANISM_ROOT_EXPORTS: &[&str] = &[
    "BudgetMetric",
    "BudgetStage",
    "BudgetUsage",
    "CacheRuntimeMetrics",
    "ExecutionBudget",
    "ExecutionContext",
    "ExecutionPolicy",
    "ExecutionPolicyRun",
    "GovernanceAudit",
    "KernelAudit",
    "KernelError",
    "KernelFailure",
    "KernelMechanism",
    "KernelPacket",
    "KernelPlanMutation",
    "KernelRequest",
    "KernelResponse",
    "KernelSequenceRequest",
    "KernelSequenceResponse",
    "KernelServices",
    "KernelStepReactiveConfig",
    "KernelStepRequest",
    "KernelStepResponse",
    "NoopSequenceObserver",
    "PersistConfig",
    "ReactivePlan",
    "ReactivePlanRequest",
    "ReactivePlanner",
    "ReactiveReplanMode",
    "ReactiveSequenceConfig",
    "ReducerExecutionAudit",
    "ReducerResult",
    "SequenceObserver",
    "load_packet_file",
    "normalize_sequence_request",
];
const BUILTINS_OWNED_EXPORTS: &[&str] = &[
    "DEFAULT_INSTRUCTION_SUMMARY_BUDGET_TOKENS",
    "DiffAnalyzeKernelInput",
    "DiffAnalyzeKernelOutput",
    "INSTRUCTION_SUMMARY_SCHEMA_VERSION",
    "ImpactKernelInput",
    "ImpactKernelOutput",
    "InstructionSummaryPayload",
    "InstructionSummaryRequest",
    "Kernel",
    "RenderedInstructionSummary",
    "SerializableFileDiff",
    "build_diff_analyze_envelope",
    "build_diff_pipeline_request",
    "build_test_impact_envelope",
    "execute",
    "execute_sequence",
    "register_v1_reducers",
    "render_instruction",
];
const CORE_COMPATIBILITY_EXCLUSIONS: &[&str] = &[
    "ExecutionPolicy",
    "ExecutionPolicyRun",
    "KernelMechanism",
    "KernelPlanMutation",
    "KernelServices",
    "ReactivePlan",
    "ReactivePlanRequest",
    "ReactivePlanner",
];
const DOCTEST_FAMILIES: &[(&str, &str)] = &[
    (
        include_str!("../../context-kernel-mechanism/PUBLIC_API.md"),
        "mechanism-execution",
    ),
    (
        include_str!("../../context-kernel-mechanism/PUBLIC_API.md"),
        "mechanism-sequence",
    ),
    (
        include_str!("../../context-kernel-mechanism/PUBLIC_API.md"),
        "mechanism-persistence",
    ),
    (
        include_str!("../../context-kernel-builtins/PUBLIC_API.md"),
        "builtins-catalog",
    ),
    (include_str!("../PUBLIC_API.md"), "kernel-compatibility"),
];

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn split_crate_root_exports_match_the_reviewed_inventories() {
    let mechanism = root_public_names(include_str!("../../context-kernel-mechanism/src/lib.rs"));
    let expected_mechanism = name_set(MECHANISM_ROOT_EXPORTS);
    assert_eq!(
        mechanism, expected_mechanism,
        "mechanism root changed without updating its reviewed inventory"
    );

    let builtins = root_public_names(include_str!("../../context-kernel-builtins/src/lib.rs"));
    let expected_builtins = expected_mechanism
        .union(&name_set(BUILTINS_OWNED_EXPORTS))
        .cloned()
        .collect();
    assert_eq!(
        builtins, expected_builtins,
        "builtins root changed without updating its reviewed inventory"
    );

    let core = root_public_names(include_str!("../src/lib.rs"));
    let exclusions = name_set(CORE_COMPATIBILITY_EXCLUSIONS);
    let expected_core = expected_builtins.difference(&exclusions).cloned().collect();
    assert_eq!(
        core, expected_core,
        "compatibility root changed without updating its reviewed exclusions"
    );
}

#[test]
fn every_supported_entrypoint_family_has_one_doctest_marker() {
    for (docs, family) in DOCTEST_FAMILIES {
        let marker = format!("<!-- public-surface:{family} -->");
        assert_eq!(
            docs.match_indices(&marker).count(),
            1,
            "entrypoint family '{family}' must have exactly one reviewed doctest section"
        );
    }
}

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
        (KernelError::EmptyTarget, "empty_target"),
        (
            KernelError::UnknownTarget {
                target: "target".to_string(),
                registered: vec!["registered".to_string()],
            },
            "unknown_target",
        ),
        (
            KernelError::InvalidRequest {
                detail: "detail".to_string(),
            },
            "invalid_request",
        ),
        (
            KernelError::BudgetExceeded {
                target: "compat.noop".to_string(),
                stage: BudgetStage::Total,
                metric: BudgetMetric::Bytes,
                used: 2,
                cap: 1,
            },
            "budget_exceeded",
        ),
        (
            KernelError::PacketRead {
                path: "packet.json".to_string(),
                detail: "detail".to_string(),
            },
            "packet_read_failed",
        ),
        (
            KernelError::PacketParse {
                path: "packet.json".to_string(),
                detail: "detail".to_string(),
            },
            "packet_parse_failed",
        ),
        (
            KernelError::ReducerFailed {
                target: "compat.noop".to_string(),
                detail: "detail".to_string(),
            },
            "reducer_failed",
        ),
        (
            KernelError::SchedulerFailed {
                detail: "detail".to_string(),
            },
            "scheduler_failed",
        ),
        (
            KernelError::CacheLock {
                detail: "detail".to_string(),
            },
            "cache_lock_failed",
        ),
        (
            KernelError::CachePersistence {
                detail: "detail".to_string(),
            },
            "cache_persistence_failed",
        ),
        (
            KernelError::PolicyViolation {
                target: "compat.noop".to_string(),
                detail: "detail".to_string(),
            },
            "policy_violation",
        ),
        (
            KernelError::SequenceCancelled {
                task_id: Some("task".to_string()),
            },
            "sequence_cancelled",
        ),
    ];

    for (error, expected_code) in errors {
        assert_eq!(error.structured().code, expected_code);
    }
}

fn name_set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn root_public_names(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut statement = None::<String>;
    let mut brace_depth = 0_i32;

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(active) = statement.as_mut() {
            active.push_str(trimmed);
            if trimmed.ends_with(';') {
                add_reexport_names(
                    &mut names,
                    &statement.take().expect("active public use statement"),
                );
            }
        } else if brace_depth == 0 {
            if let Some(tail) = trimmed.strip_prefix("pub use ") {
                if tail.ends_with(';') {
                    add_reexport_names(&mut names, tail);
                } else {
                    statement = Some(tail.to_string());
                }
            } else {
                for prefix in [
                    "pub const ",
                    "pub enum ",
                    "pub fn ",
                    "pub mod ",
                    "pub static ",
                    "pub struct ",
                    "pub trait ",
                    "pub type ",
                ] {
                    if let Some(tail) = trimmed.strip_prefix(prefix) {
                        let name = tail
                            .split(|character: char| {
                                !(character.is_alphanumeric() || character == '_')
                            })
                            .next()
                            .expect("public item name");
                        names.insert(name.to_string());
                    }
                }
            }
        }

        brace_depth += trimmed.matches('{').count() as i32;
        brace_depth -= trimmed.matches('}').count() as i32;
    }

    assert!(
        statement.is_none(),
        "unterminated root public use statement"
    );
    assert_eq!(brace_depth, 0, "unbalanced source braces");
    names
}

fn add_reexport_names(names: &mut BTreeSet<String>, statement: &str) {
    assert!(
        !statement.contains(" as "),
        "aliased public uses need an explicit inventory parser update"
    );
    let compact = statement
        .trim_end_matches(';')
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert!(
        !compact.contains('*'),
        "reviewed roots must use explicit re-exports"
    );

    if let Some(open) = compact.find('{') {
        let close = compact.rfind('}').expect("closed public use group");
        for name in compact[open + 1..close]
            .split(',')
            .filter(|name| !name.is_empty())
        {
            assert!(
                !name.contains('{') && !name.contains('}'),
                "nested public use groups need an explicit inventory parser update"
            );
            names.insert(exported_name(name));
        }
    } else {
        names.insert(exported_name(&compact));
    }
}

fn exported_name(path: &str) -> String {
    path.rsplit("::")
        .next()
        .expect("public re-export name")
        .to_string()
}
