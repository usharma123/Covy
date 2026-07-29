# Supported public surface

`context-kernel-mechanism` is the target-neutral kernel API. Its public surface
has three operational families: custom composition and execution, sequence
scheduling with cooperative cancellation, and cache persistence. Shared wire
DTOs are re-exported for those families; they are not separate operational
entrypoints.

## Reviewed root inventory

The stable root inventory contains exactly 35 names, grouped by role:

- Execution: `KernelMechanism`, `ExecutionContext`, `KernelPacket`,
  `KernelRequest`, `KernelResponse`, `ReducerResult`, `KernelError`, and
  `KernelFailure`.
- Sequences: `normalize_sequence_request`, `KernelSequenceRequest`,
  `KernelSequenceResponse`, `KernelStepRequest`, `KernelStepResponse`,
  `KernelStepReactiveConfig`, `ReactiveSequenceConfig`, `ReactiveReplanMode`,
  `SequenceObserver`, and `NoopSequenceObserver`.
- Budgets, audit, and cache reporting: `ExecutionBudget`, `BudgetMetric`,
  `BudgetStage`, `BudgetUsage`, `KernelAudit`, `GovernanceAudit`,
  `ReducerExecutionAudit`, and `CacheRuntimeMetrics`.
- Composition SPI: `KernelServices`, `ExecutionPolicy`, `ExecutionPolicyRun`,
  `ReactivePlanner`, `ReactivePlanRequest`, `ReactivePlan`, and
  `KernelPlanMutation`.
- Persistence and packet loading: `PersistConfig` and `load_packet_file`.

<!-- public-surface:mechanism-execution -->
## Custom composition and execution

Register synchronous reducers on [`KernelMechanism`]. [`KernelServices`] lets a
composition inject request policy and reactive planning without teaching the
mechanism any concrete target names.

```rust
use context_kernel_mechanism::{
    KernelMechanism, KernelPacket, KernelRequest, ReducerResult,
};
use serde_json::json;

let mut kernel = KernelMechanism::new();
kernel.register_reducer("example.echo", |_context, packets| {
    Ok(ReducerResult {
        output_packets: packets.to_vec(),
        ..ReducerResult::default()
    })
});

let response = kernel.execute(KernelRequest {
    target: "example.echo".to_string(),
    input_packets: vec![KernelPacket::from_value(json!({"value": 7}), None)],
    policy_context: json!({"disable_cache": true}),
    ..KernelRequest::default()
})?;

assert_eq!(response.target, "example.echo");
assert_eq!(response.output_packets[0].body, json!({"value": 7}));
# Ok::<(), context_kernel_mechanism::KernelError>(())
```

The reducer, policy, planner, cache hook, and observer callbacks are trusted
in-process extensions. Their panics are not caught by the kernel.

<!-- public-surface:mechanism-sequence -->
## Sequence scheduling and cancellation

[`KernelMechanism::execute_sequence`] normalizes step identifiers, validates
dependencies, and schedules ready work under the sequence budget.

```rust
use context_kernel_mechanism::{
    KernelMechanism, KernelSequenceRequest, KernelStepRequest, ReducerResult,
};

let mut kernel = KernelMechanism::new();
kernel.register_reducer("example.first", |_context, _packets| {
    Ok(ReducerResult::default())
});
kernel.register_reducer("example.second", |_context, _packets| {
    Ok(ReducerResult::default())
});

let response = kernel.execute_sequence(KernelSequenceRequest {
    steps: vec![
        KernelStepRequest {
            id: "second".to_string(),
            target: "example.second".to_string(),
            depends_on: vec!["first".to_string()],
            ..KernelStepRequest::default()
        },
        KernelStepRequest {
            id: "first".to_string(),
            target: "example.first".to_string(),
            ..KernelStepRequest::default()
        },
    ],
    ..KernelSequenceRequest::default()
})?;

assert_eq!(response.scheduled, ["first", "second"]);
# Ok::<(), context_kernel_mechanism::KernelError>(())
```

Normalization rejects empty targets, duplicate identifiers, and empty, self,
or unknown dependencies. Scheduler and mutation failures become
[`KernelError::SchedulerFailed`]. Budget exhaustion is a successful response
with skipped steps. A reducer failure is recorded on that step; dependent work
is skipped while independent work may continue.

[`SequenceObserver::should_cancel`] is checked at cooperative step and callback
boundaries. It cannot interrupt a synchronous reducer already running.
Cancellation returns [`KernelError::SequenceCancelled`] rather than a partial
sequence response.

<!-- public-surface:mechanism-persistence -->
## Cache persistence lifecycle

[`KernelMechanism::with_persistence`] keeps construction infallible: if the
persistence owner cannot open, execution remains available with an in-memory
cache and the failure is visible through
[`KernelMechanism::cache_runtime_metrics`].

```rust
use std::time::Duration;

use context_kernel_mechanism::{
    KernelMechanism, KernelPacket, KernelRequest, PersistConfig, ReducerResult,
};
use serde_json::json;

let directory = tempfile::tempdir()?;
let mut kernel =
    KernelMechanism::with_persistence(PersistConfig::new(directory.path().to_path_buf()));
kernel.register_reducer("example.persist", |_context, _packets| {
    Ok(ReducerResult {
        output_packets: vec![KernelPacket::from_value(json!({"saved": true}), None)],
        ..ReducerResult::default()
    })
});

kernel.execute(KernelRequest {
    target: "example.persist".to_string(),
    ..KernelRequest::default()
})?;
let metrics = kernel.shutdown_cache_persistence(Duration::from_secs(2))?;

assert!(metrics.persisted_deltas >= 1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Cache mutations become visible in memory before asynchronous durability.
`flush_cache_persistence` waits for queued deltas to reach the WAL;
`shutdown_cache_persistence` is the explicit bounded flush, checkpoint, and
join contract for the final root owner. Callers must not rely on drop timing.
Timeouts and lower-level failures are reported as
[`KernelError::CachePersistence`]. Pruning reserves persistence capacity before
mutating live state, then records tombstones and flushes; a later persistence
failure is reported but is not a transactional rollback.

## Errors

[`KernelError::structured`] maps the stable categories to:

`empty_target`, `unknown_target`, `invalid_request`, `budget_exceeded`,
`packet_read_failed`, `packet_parse_failed`, `reducer_failed`,
`scheduler_failed`, `cache_lock_failed`, `cache_persistence_failed`,
`policy_violation`, and `sequence_cancelled`.

Fallible execution, sequence, file-loading, cache inspection, recall, pruning,
flush, and shutdown entrypoints return [`KernelError`]. There is no public
unsafe kernel API.

## Reviewed exclusions

- Wire request, response, budget, audit, and failure DTOs support the three
  operational families and are intentionally excluded from separate doctests.
- `ExecutionPolicy`, `ExecutionPolicyRun`, `ReactivePlanner`, and the reactive
  plan types are the composition SPI, covered by the composition family.
- Context-store inspection and file loading are utilities inside the
  persistence and execution families, not additional lifecycle owners.
- `context_scheduler_core` and context-memory implementation types are not
  kernel root APIs; only [`PersistConfig`] is re-exported.
- Built-in targets, policies, and adapters belong to
  `context-kernel-builtins`; daemon runtime APIs are outside this crate.
