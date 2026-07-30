use super::*;
use crate::watch::{run_recovered_replan_for_task, run_sequence_for_task_with_executor};
use packet28_daemon_core::storage::{
    append_next_task_event, append_task_event, load_task_registry,
    load_task_registry_with_event_tails, save_task_registry, save_watch_registry,
};
use std::fs::{self, OpenOptions};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Barrier;

fn registry_with_task(task_id: &str, last_event_seq: u64) -> TaskRegistry {
    let mut registry = TaskRegistry::default();
    registry.tasks.insert(
        task_id.to_string(),
        TaskRecord {
            task_id: task_id.to_string(),
            last_event_seq,
            ..TaskRecord::default()
        },
    );
    registry
}

fn event(kind: &str) -> DaemonEvent {
    DaemonEvent {
        kind: kind.to_string(),
        occurred_at_unix: 1,
        data: json!({"source": "reconciliation-test"}),
    }
}

#[test]
fn startup_reconciliation_advances_a_registry_lagging_the_durable_event_log() {
    let root = tempfile::tempdir().unwrap();
    ensure_daemon_dir(root.path()).unwrap();
    save_task_registry(root.path(), &registry_with_task("replay", 0)).unwrap();
    let frame = append_next_task_event(root.path(), "replay", &event("durable")).unwrap();
    assert_eq!(frame.seq, 1);

    let (mut loaded, tails) = load_task_registry_with_event_tails(root.path()).unwrap();
    assert!(reconcile_task_event_high_waters(&mut loaded, &tails).unwrap());
    assert_eq!(loaded.tasks["replay"].last_event_seq, 1);
}

#[test]
fn startup_reconciliation_rejects_a_registry_ahead_of_its_event_log() {
    let root = tempfile::tempdir().unwrap();
    ensure_daemon_dir(root.path()).unwrap();
    save_task_registry(root.path(), &registry_with_task("ahead", 0)).unwrap();
    append_next_task_event(root.path(), "ahead", &event("only-event")).unwrap();
    save_task_registry(root.path(), &registry_with_task("ahead", 2)).unwrap();

    let (mut loaded, tails) = load_task_registry_with_event_tails(root.path()).unwrap();
    let error = reconcile_task_event_high_waters(&mut loaded, &tails).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("high-water 2 for 'ahead' is ahead of durable event sequence 1"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn startup_reconciliation_rejects_nonzero_high_water_without_an_event_log() {
    let root = tempfile::tempdir().unwrap();
    ensure_daemon_dir(root.path()).unwrap();
    save_task_registry(root.path(), &registry_with_task("missing-log", 1)).unwrap();

    let (mut loaded, tails) = load_task_registry_with_event_tails(root.path()).unwrap();
    let error = reconcile_task_event_high_waters(&mut loaded, &tails).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("high-water 1 for 'missing-log' is ahead of its missing event log"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn startup_reconciliation_preserves_terminal_and_completes_cancellation_idempotently() {
    const RECOVERED_AT_UNIX: u64 = 31;
    let mut tasks = TaskRegistry::default();
    let mut watches = WatchRegistry::default();
    for (task_id, lifecycle) in [
        ("queued", TaskLifecycle::ReplanPending),
        ("recovered-running", TaskLifecycle::RunningRecoveredReplan),
        ("running", TaskLifecycle::Running),
        ("running-queued", TaskLifecycle::RunningReplanPending),
        (
            "cancelling-idle",
            TaskLifecycle::Cancelling { was_running: false },
        ),
        (
            "cancelling-running",
            TaskLifecycle::Cancelling { was_running: true },
        ),
    ] {
        let watch_ids = if task_id.starts_with("cancelling") {
            vec![format!("watch-{task_id}")]
        } else {
            Vec::new()
        };
        if let Some(watch_id) = watch_ids.first() {
            watches.watches.push(WatchRegistration {
                watch_id: watch_id.clone(),
                spec: WatchSpec {
                    task_id: task_id.to_string(),
                    ..WatchSpec::default()
                },
                active: true,
                ..WatchRegistration::default()
            });
        }
        tasks.tasks.insert(
            task_id.to_string(),
            TaskRecord {
                task_id: task_id.to_string(),
                lifecycle,
                watch_ids,
                last_error: task_id
                    .starts_with("cancelling")
                    .then(|| "existing cancellation detail".to_string()),
                ..TaskRecord::default()
            },
        );
    }
    tasks.tasks.insert(
        "idle".to_string(),
        TaskRecord {
            task_id: "idle".to_string(),
            last_error: Some("existing failure".to_string()),
            ..TaskRecord::default()
        },
    );
    tasks.tasks.insert(
        "cancelled".to_string(),
        TaskRecord {
            task_id: "cancelled".to_string(),
            lifecycle: TaskLifecycle::Cancelled,
            last_completed_at_unix: Some(17),
            last_error: Some("durable cancellation history".to_string()),
            ..TaskRecord::default()
        },
    );

    let reconciliation =
        reconcile_interrupted_task_lifecycles(&mut tasks, &mut watches, RECOVERED_AT_UNIX).unwrap();
    assert_eq!(reconciliation.changed_tasks, 5);
    assert_eq!(
        reconciliation.replan_task_ids,
        vec![
            "queued".to_string(),
            "recovered-running".to_string(),
            "running-queued".to_string()
        ]
    );
    assert!(watches.watches.is_empty());
    for (task_id, task) in &tasks.tasks {
        match task_id.as_str() {
            "idle" => {
                assert_eq!(task.lifecycle, TaskLifecycle::Idle);
                assert_eq!(task.last_error.as_deref(), Some("existing failure"));
            }
            "cancelled" => {
                assert_eq!(task.lifecycle, TaskLifecycle::Cancelled);
                assert_eq!(task.last_completed_at_unix, Some(17));
                assert_eq!(
                    task.last_error.as_deref(),
                    Some("durable cancellation history")
                );
            }
            "cancelling-idle" | "cancelling-running" => {
                assert_eq!(task.lifecycle, TaskLifecycle::Cancelled);
                assert_eq!(task.last_completed_at_unix, Some(RECOVERED_AT_UNIX));
                assert!(task.watch_ids.is_empty());
                assert!(
                    task.last_error.as_deref().is_some_and(|error| {
                        error.starts_with("existing cancellation detail; ")
                            && error.contains("task cancellation completed by packet28d restart")
                    }),
                    "task {task_id:?} did not retain cancellation evidence"
                );
            }
            "queued" => {
                assert_eq!(task.lifecycle, TaskLifecycle::ReplanPending);
                assert_eq!(task.last_error, None);
            }
            "running-queued" => {
                assert_eq!(task.lifecycle, TaskLifecycle::ReplanPending);
                assert!(
                    task.last_error.as_deref().is_some_and(|error| {
                        error.starts_with("task interrupted by packet28d restart")
                            && error.contains("durable replan remains queued")
                    }),
                    "task {task_id:?} did not retain its queued replan"
                );
            }
            "recovered-running" => {
                assert_eq!(task.lifecycle, TaskLifecycle::ReplanPending);
                assert!(
                    task.last_error.as_deref().is_some_and(|error| {
                        error.contains("interrupted after its durable claim")
                            && error.contains("durable replan remains queued")
                    }),
                    "task {task_id:?} did not retain its recovered replan"
                );
            }
            "running" => {
                assert_eq!(task.lifecycle, TaskLifecycle::Idle);
                assert!(
                    task.last_error.as_deref().is_some_and(
                        |error| error.starts_with("task interrupted by packet28d restart")
                    ),
                    "task {task_id:?} did not retain interruption evidence"
                );
            }
            _ => unreachable!("unexpected task {task_id:?}"),
        }
    }
    let reconciled = serde_json::to_value(&tasks).unwrap();

    let repeated =
        reconcile_interrupted_task_lifecycles(&mut tasks, &mut watches, RECOVERED_AT_UNIX + 1)
            .unwrap();
    assert_eq!(repeated.changed_tasks, 0);
    assert_eq!(
        repeated.replan_task_ids,
        vec![
            "queued".to_string(),
            "recovered-running".to_string(),
            "running-queued".to_string()
        ]
    );
    assert_eq!(serde_json::to_value(&tasks).unwrap(), reconciled);
}

#[test]
fn recovered_replan_claim_is_pending_only_generation_fenced_and_shutdown_safe() {
    let state = super::support::daemon_test_state();
    let root = super::support::daemon_test_root(&state);
    let calls = Arc::new(AtomicUsize::new(0));
    let reducer_calls = calls.clone();
    let durable_root = root.clone();
    let mut kernel = Kernel::new();
    kernel.register_reducer("recovery.count", move |_ctx, _packets| {
        assert_eq!(
            load_task_registry(&durable_root).unwrap().tasks["recovered-claim"].lifecycle,
            TaskLifecycle::RunningRecoveredReplan,
            "the recovered claim must be durable before reducer entry"
        );
        reducer_calls.fetch_add(1, Ordering::SeqCst);
        Ok(context_kernel_core::ReducerResult::default())
    });
    state.lock().unwrap().kernel = Arc::new(kernel);
    let sequence = context_kernel_core::KernelSequenceRequest {
        steps: vec![KernelStepRequest {
            id: "count".to_string(),
            target: "recovery.count".to_string(),
            ..KernelStepRequest::default()
        }],
        ..context_kernel_core::KernelSequenceRequest::default()
    };
    super::support::insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: "recovered-claim".to_string(),
            lifecycle: TaskLifecycle::ReplanPending,
            sequence_present: true,
            sequence: Some(sequence.clone()),
            ..TaskRecord::default()
        },
    );
    let original_generation = state
        .lock()
        .unwrap()
        .task_generations
        .create("recovered-claim")
        .unwrap()
        .id();

    assert!(
        run_recovered_replan_for_task(state.clone(), "recovered-claim", original_generation)
            .unwrap()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.lock().unwrap().tasks.tasks["recovered-claim"].lifecycle,
        TaskLifecycle::Idle
    );
    assert!(
        !run_recovered_replan_for_task(state.clone(), "recovered-claim", original_generation)
            .unwrap(),
        "a duplicate recovery command must not claim an idle task"
    );

    let (watch_tx, _watch_rx) = WatchIngress::new(1);
    register_task_and_watches(
        state.clone(),
        watch_tx,
        TaskSubmitSpec {
            task_id: "recovered-claim".to_string(),
            sequence,
            ..TaskSubmitSpec::default()
        },
    )
    .unwrap();
    let replacement_generation = state
        .lock()
        .unwrap()
        .task_generations
        .current("recovered-claim")
        .unwrap()
        .id();
    assert_ne!(replacement_generation, original_generation);
    assert!(
        !run_recovered_replan_for_task(state.clone(), "recovered-claim", original_generation)
            .unwrap(),
        "a stale recovery command must not cross same-id replacement"
    );

    {
        let mut guard = state.lock().unwrap();
        assert!(guard
            .tasks
            .tasks
            .get_mut("recovered-claim")
            .unwrap()
            .lifecycle
            .request_replan()
            .unwrap());
        guard.shutting_down = true;
    }
    assert!(
        !run_recovered_replan_for_task(state.clone(), "recovered-claim", replacement_generation)
            .unwrap(),
        "shutdown must prevent late recovery admission"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        load_task_events(&root, "recovered-claim")
            .unwrap()
            .iter()
            .filter(|frame| frame.event.kind == "task_started")
            .count(),
        1
    );
}

#[test]
fn durable_replan_claim_keeps_ownership_when_another_replan_arrives_during_its_barrier() {
    let state = super::support::daemon_test_state();
    let task_id = "recovered-claim-race";
    let calls = Arc::new(AtomicUsize::new(0));
    super::support::insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: task_id.to_string(),
            lifecycle: TaskLifecycle::ReplanPending,
            sequence_present: true,
            sequence: Some(context_kernel_core::KernelSequenceRequest {
                steps: vec![KernelStepRequest {
                    id: "claim-race".to_string(),
                    target: "recovery.claim-race".to_string(),
                    ..KernelStepRequest::default()
                }],
                ..context_kernel_core::KernelSequenceRequest::default()
            }),
            ..TaskRecord::default()
        },
    );
    flush_persistence(&state).unwrap();
    let (claim_reached, release_claim) = state
        .lock()
        .unwrap()
        .persistence
        .gate_checkpoint_for_test(2);

    let run_state = state.clone();
    let executor_calls = calls.clone();
    let run = thread::spawn(move || {
        run_sequence_for_task_with_executor(
            run_state,
            task_id,
            None,
            move |_kernel, _sequence, _observer| {
                let ordinal = executor_calls.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(context_kernel_core::KernelSequenceResponse {
                    request_id: u64::try_from(ordinal).unwrap(),
                    scheduled: Vec::new(),
                    skipped: Vec::new(),
                    budget_exhausted: false,
                    step_results: Vec::new(),
                    metadata: json!({}),
                })
            },
        )
    });

    claim_reached
        .recv_timeout(Duration::from_secs(2))
        .expect("durable claim did not reach its persistence barrier");
    assert_eq!(
        state.lock().unwrap().tasks.tasks[task_id].lifecycle,
        TaskLifecycle::RunningRecoveredReplan
    );
    {
        let mut guard = state.lock().unwrap();
        assert!(!guard
            .tasks
            .tasks
            .get_mut(task_id)
            .unwrap()
            .lifecycle
            .request_replan()
            .unwrap());
        assert_eq!(
            guard.tasks.tasks[task_id].lifecycle,
            TaskLifecycle::RunningReplanPending
        );
        persist_state(&guard).unwrap();
    }
    release_claim.send(()).unwrap();

    let response = run.join().unwrap().unwrap().unwrap();
    assert_eq!(response.request_id, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        state.lock().unwrap().tasks.tasks[task_id].lifecycle,
        TaskLifecycle::Idle
    );
}

#[test]
fn restart_preflight_rejects_malformed_work_without_mutating_earlier_tasks() {
    let tasks = TaskRegistry {
        tasks: BTreeMap::from([
            (
                "a-running".to_string(),
                TaskRecord {
                    task_id: "a-running".to_string(),
                    lifecycle: TaskLifecycle::Running,
                    ..TaskRecord::default()
                },
            ),
            (
                "z-malformed".to_string(),
                TaskRecord {
                    task_id: "z-malformed".to_string(),
                    lifecycle: TaskLifecycle::ReplanPending,
                    sequence_present: true,
                    sequence: None,
                    ..TaskRecord::default()
                },
            ),
        ]),
    };
    let before = serde_json::to_value(&tasks).unwrap();

    let error = preflight_restart_recovery(&tasks).unwrap_err();

    assert!(error
        .to_string()
        .contains("startup replan task 'z-malformed' has no stored sequence"));
    assert_eq!(serde_json::to_value(&tasks).unwrap(), before);
}

#[test]
fn recovered_process_group_probe_rejects_unsafe_identifiers() {
    let zero_error = crate::launch::recovered_agent_process_group_exists(0).unwrap_err();
    assert!(zero_error.to_string().contains("greater than zero"));

    let current_group = crate::launch::current_process_group();
    let current_group = u32::try_from(current_group).unwrap();
    let group_error =
        crate::launch::recovered_agent_process_group_exists(current_group).unwrap_err();
    assert!(group_error.to_string().contains("owns the current daemon"));
}

#[test]
fn successful_run_postprocessing_failure_does_not_abandon_owned_rerun() {
    let state = super::support::daemon_test_state();
    let root = super::support::daemon_test_root(&state);
    let task_id = "postprocess-rerun";
    let event_path = task_event_log_path(&root, &task_storage_id(task_id).unwrap());
    let saved_event_path = event_path.with_extension("jsonl.saved");
    let calls = Arc::new(AtomicUsize::new(0));
    let executor_calls = calls.clone();
    let executor_state = state.clone();
    super::support::insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: task_id.to_string(),
            sequence_present: true,
            sequence: Some(context_kernel_core::KernelSequenceRequest {
                steps: vec![KernelStepRequest {
                    id: "succeed-twice".to_string(),
                    target: "replan.succeed-twice".to_string(),
                    ..KernelStepRequest::default()
                }],
                ..context_kernel_core::KernelSequenceRequest::default()
            }),
            ..TaskRecord::default()
        },
    );

    let result = run_sequence_for_task_with_executor(
        state.clone(),
        task_id,
        None,
        move |_kernel, _sequence, _observer| {
            let ordinal = executor_calls.fetch_add(1, Ordering::SeqCst);
            if ordinal == 0 {
                {
                    let mut guard = executor_state.lock().unwrap();
                    assert!(!guard
                        .tasks
                        .tasks
                        .get_mut(task_id)
                        .unwrap()
                        .lifecycle
                        .request_replan()
                        .unwrap());
                    persist_state(&guard).unwrap();
                }
                flush_persistence(&executor_state).unwrap();
                fs::rename(&event_path, &saved_event_path).unwrap();
                fs::create_dir(&event_path).unwrap();
                return Ok(context_kernel_core::KernelSequenceResponse {
                    request_id: 1,
                    scheduled: Vec::new(),
                    skipped: Vec::new(),
                    budget_exhausted: false,
                    step_results: Vec::new(),
                    metadata: json!({}),
                });
            }
            fs::remove_dir(&event_path).unwrap();
            fs::rename(&saved_event_path, &event_path).unwrap();
            Ok(context_kernel_core::KernelSequenceResponse {
                request_id: 2,
                scheduled: Vec::new(),
                skipped: Vec::new(),
                budget_exhausted: false,
                step_results: Vec::new(),
                metadata: json!({}),
            })
        },
    )
    .unwrap()
    .unwrap();

    assert_eq!(result.request_id, 2);
    flush_persistence(&state).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        state.lock().unwrap().tasks.tasks[task_id].lifecycle,
        TaskLifecycle::Idle
    );
    let events = load_task_events(&root, task_id).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|frame| frame.event.kind == "task_completed")
            .count(),
        1
    );
}

#[test]
fn completed_rerun_checkpoint_failure_requests_process_recovery() {
    let state = super::support::daemon_test_state();
    let task_id = "checkpoint-failure-rerun";
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    super::support::insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: task_id.to_string(),
            sequence_present: true,
            sequence: Some(context_kernel_core::KernelSequenceRequest {
                steps: vec![KernelStepRequest {
                    id: "checkpoint-failure".to_string(),
                    target: "recovery.checkpoint-failure".to_string(),
                    ..KernelStepRequest::default()
                }],
                ..context_kernel_core::KernelSequenceRequest::default()
            }),
            ..TaskRecord::default()
        },
    );

    let run_state = state.clone();
    let run = thread::spawn(move || {
        run_sequence_for_task_with_executor(
            run_state,
            task_id,
            None,
            move |_kernel, _sequence, _observer| {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(context_kernel_core::KernelSequenceResponse {
                    request_id: 1,
                    scheduled: Vec::new(),
                    skipped: Vec::new(),
                    budget_exhausted: false,
                    step_results: Vec::new(),
                    metadata: json!({}),
                })
            },
        )
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let persistence = {
        let mut guard = state.lock().unwrap();
        assert!(!guard
            .tasks
            .tasks
            .get_mut(task_id)
            .unwrap()
            .lifecycle
            .request_replan()
            .unwrap());
        persist_state(&guard).unwrap();
        guard.persistence.clone()
    };
    persistence.flush().unwrap();
    persistence.exhaust_revisions_for_test();
    release_tx.send(()).unwrap();

    let error = run.join().unwrap().unwrap_err();
    assert!(error
        .to_string()
        .contains("failed to persist completed task lifecycle"));
    let guard = state.lock().unwrap();
    assert_eq!(
        guard.tasks.tasks[task_id].lifecycle,
        TaskLifecycle::ReplanPending
    );
    assert!(guard.shutdown.is_requested());
}

#[test]
fn durable_event_io_does_not_hold_the_daemon_state_mutex() {
    let state = super::support::daemon_test_state();
    super::support::insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: "lock-split".to_string(),
            ..TaskRecord::default()
        },
    );
    emit_task_event(state.clone(), "lock-split", "first", json!({"ordinal": 1})).unwrap();
    flush_persistence(&state).unwrap();

    let root = super::support::daemon_test_root(&state);
    let event_path = task_event_log_path(&root, &task_storage_id("lock-split").unwrap());
    let event_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&event_path)
        .unwrap();
    fs2::FileExt::lock_exclusive(&event_file).unwrap();
    let started_before = state.lock().unwrap().persistence.metrics().events_started;
    let event_state = state.clone();
    let event_thread = thread::spawn(move || {
        emit_task_event(
            event_state,
            "lock-split",
            "blocked-on-event-file",
            json!({"ordinal": 2}),
        )
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let started = state.lock().unwrap().persistence.metrics().events_started;
        if started > started_before {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "persistence worker did not reach the blocked event append"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        state.try_lock().is_ok(),
        "daemon state mutex remained held while durable event I/O was blocked"
    );
    fs2::FileExt::unlock(&event_file).unwrap();
    event_thread.join().unwrap().unwrap();
    flush_persistence(&state).unwrap();
    super::support::shutdown_test_persistence(&state);
}

#[test]
fn concurrent_event_publication_is_contiguous_and_registry_checkpoints_coalesce() {
    let state = super::support::daemon_test_state();
    super::support::insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: "publication-order".to_string(),
            ..TaskRecord::default()
        },
    );
    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
    state
        .lock()
        .unwrap()
        .subscribers
        .entry("publication-order".to_string())
        .or_default()
        .push(crate::state::TaskSubscriber { id: 1, sender });
    let start = Arc::new(Barrier::new(17));
    let mut workers = Vec::new();
    for ordinal in 0..16 {
        let state = state.clone();
        let start = start.clone();
        workers.push(thread::spawn(move || {
            start.wait();
            emit_task_event(
                state,
                "publication-order",
                "concurrent",
                json!({"ordinal": ordinal}),
            )
        }));
    }
    start.wait();
    for worker in workers {
        worker.join().unwrap().unwrap();
    }
    let frames = (0..16)
        .map(|_| receiver.blocking_recv().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        frames.iter().map(|frame| frame.seq).collect::<Vec<_>>(),
        (1..=16).collect::<Vec<_>>()
    );
    flush_persistence(&state).unwrap();
    let metrics = state.lock().unwrap().persistence.metrics();
    assert!(
        metrics.checkpoints_written < metrics.events_appended,
        "event burst wrote {} checkpoints for {} appended events",
        metrics.checkpoints_written,
        metrics.events_appended
    );
    let root = super::support::daemon_test_root(&state);
    assert_eq!(
        load_task_registry(&root).unwrap().tasks["publication-order"].last_event_seq,
        16
    );
    super::support::shutdown_test_persistence(&state);
}

#[test]
fn packet28d_persistence_io_has_one_source_owner() {
    fn visit(source_root: &Path, path: &Path, violations: &mut Vec<String>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "tests") {
                    continue;
                }
                visit(source_root, &path, violations);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs")
                || path
                    .file_name()
                    .is_some_and(|name| name == "persistence.rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for forbidden in [
                "save_task_registry(",
                "save_watch_registry(",
                "append_next_task_event(",
                "append_task_event(",
            ] {
                if source.contains(forbidden) {
                    violations.push(format!(
                        "{} contains {forbidden}",
                        path.strip_prefix(source_root).unwrap().display()
                    ));
                }
            }
        }
    }

    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    visit(&source_root, &source_root, &mut violations);
    assert!(
        violations.is_empty(),
        "persistence I/O escaped its owner:\n{}",
        violations.join("\n")
    );
}

const BENCHMARK_TASKS: usize = 300;
const BENCHMARK_EVENTS: u64 = 32;
const BENCHMARK_REPEATS: usize = 3;
const BENCHMARK_TARGET_REGISTRY_BYTES: usize = 1_848_193;

fn benchmark_registry_with_padding(padding: usize) -> TaskRegistry {
    let marker = "x".repeat(padding);
    let tasks = (0..BENCHMARK_TASKS)
        .map(|ordinal| {
            let task_id = if ordinal == 0 {
                "benchmark-target".to_string()
            } else {
                format!("benchmark-{ordinal:03}")
            };
            (
                task_id.clone(),
                TaskRecord {
                    task_id,
                    last_error: Some(marker.clone()),
                    ..TaskRecord::default()
                },
            )
        })
        .collect();
    TaskRegistry { tasks }
}

fn benchmark_registry() -> (TaskRegistry, usize) {
    let mut low = 0_usize;
    let mut high = 16 * 1024;
    let mut best = None::<(usize, usize)>;
    while low <= high {
        let padding = low + (high - low) / 2;
        let registry = benchmark_registry_with_padding(padding);
        let bytes = serde_json::to_vec_pretty(&registry).unwrap().len();
        if best.as_ref().is_none_or(|(_, best_bytes)| {
            bytes.abs_diff(BENCHMARK_TARGET_REGISTRY_BYTES)
                < best_bytes.abs_diff(BENCHMARK_TARGET_REGISTRY_BYTES)
        }) {
            best = Some((padding, bytes));
        }
        if bytes < BENCHMARK_TARGET_REGISTRY_BYTES {
            low = padding.saturating_add(1);
        } else if padding == 0 {
            break;
        } else {
            high = padding - 1;
        }
    }
    let (padding, _) = best.unwrap();
    let registry = benchmark_registry_with_padding(padding);
    let bytes = serde_json::to_vec_pretty(&registry).unwrap().len();
    (registry, bytes)
}

fn benchmark_event(sequence: u64) -> DaemonEventFrame {
    DaemonEventFrame {
        seq: sequence,
        task_id: "benchmark-target".to_string(),
        event: DaemonEvent {
            kind: "benchmark".to_string(),
            occurred_at_unix: sequence,
            data: json!({"sequence": sequence}),
        },
    }
}

fn median(values: &mut [u64]) -> u64 {
    values.sort_unstable();
    values[values.len() / 2]
}

struct PersistenceBenchmarkSample {
    lock_nanos: Vec<u64>,
    elapsed_micros: u64,
    published_bytes: u64,
    checkpoints: u64,
    registry: serde_json::Value,
    event_count: usize,
}

fn run_legacy_persistence_sample(fixture: &TaskRegistry) -> PersistenceBenchmarkSample {
    let root = tempfile::tempdir().unwrap();
    ensure_daemon_dir(root.path()).unwrap();
    let watches = WatchRegistry::default();
    let mut tasks = fixture.clone();
    save_watch_registry(root.path(), &watches).unwrap();
    save_task_registry(root.path(), &tasks).unwrap();
    append_task_event(root.path(), &benchmark_event(1)).unwrap();
    tasks
        .tasks
        .get_mut("benchmark-target")
        .unwrap()
        .last_event_seq = 1;
    save_task_registry(root.path(), &tasks).unwrap();

    let state = Mutex::new((tasks, watches));
    let event_path =
        task_event_log_path(root.path(), &task_storage_id("benchmark-target").unwrap());
    let started = Instant::now();
    let mut lock_nanos = Vec::new();
    let mut published_bytes = 0_u64;
    for sequence in 2..=BENCHMARK_EVENTS + 1 {
        let event_len_before = std::fs::metadata(&event_path).unwrap().len();
        let mut guard = state.lock().unwrap();
        let lock_acquired = Instant::now();
        guard
            .0
            .tasks
            .get_mut("benchmark-target")
            .unwrap()
            .last_event_seq = sequence;
        append_task_event(root.path(), &benchmark_event(sequence)).unwrap();
        save_watch_registry(root.path(), &guard.1).unwrap();
        save_task_registry(root.path(), &guard.0).unwrap();
        lock_nanos.push(u64::try_from(lock_acquired.elapsed().as_nanos()).unwrap_or(u64::MAX));
        drop(guard);
        published_bytes = published_bytes
            .saturating_add(
                std::fs::metadata(&event_path)
                    .unwrap()
                    .len()
                    .saturating_sub(event_len_before),
            )
            .saturating_add(
                std::fs::metadata(packet28_daemon_protocol::paths::task_registry_path(
                    root.path(),
                ))
                .unwrap()
                .len(),
            )
            .saturating_add(
                std::fs::metadata(packet28_daemon_protocol::paths::watch_registry_path(
                    root.path(),
                ))
                .unwrap()
                .len(),
            );
    }
    let elapsed_micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let recovered = load_task_registry(root.path()).unwrap();
    let events = load_task_events(root.path(), "benchmark-target").unwrap();
    PersistenceBenchmarkSample {
        lock_nanos,
        elapsed_micros,
        published_bytes,
        checkpoints: BENCHMARK_EVENTS,
        registry: serde_json::to_value(recovered).unwrap(),
        event_count: events.len(),
    }
}

fn run_owned_persistence_sample(fixture: &TaskRegistry) -> PersistenceBenchmarkSample {
    let state = super::support::daemon_test_state();
    {
        let mut guard = state.lock().unwrap();
        guard.tasks = fixture.clone();
        persist_state(&guard).unwrap();
    }
    flush_persistence(&state).unwrap();
    emit_task_event(
        state.clone(),
        "benchmark-target",
        "benchmark",
        json!({"sequence": 1}),
    )
    .unwrap();
    flush_persistence(&state).unwrap();
    let before = state.lock().unwrap().persistence.metrics();

    let started = Instant::now();
    let mut lock_nanos = Vec::new();
    for sequence in 2..=BENCHMARK_EVENTS + 1 {
        let lock_before = state
            .lock()
            .unwrap()
            .persistence
            .metrics()
            .event_state_lock_nanos;
        emit_task_event(
            state.clone(),
            "benchmark-target",
            "benchmark",
            json!({"sequence": sequence}),
        )
        .unwrap();
        let lock_after = state
            .lock()
            .unwrap()
            .persistence
            .metrics()
            .event_state_lock_nanos;
        lock_nanos.push(lock_after.saturating_sub(lock_before));
    }
    flush_persistence(&state).unwrap();
    let elapsed_micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let after = state.lock().unwrap().persistence.metrics();
    let root = super::support::daemon_test_root(&state);
    let recovered = load_task_registry(&root).unwrap();
    let events = load_task_events(&root, "benchmark-target").unwrap();
    super::support::shutdown_test_persistence(&state);
    PersistenceBenchmarkSample {
        lock_nanos,
        elapsed_micros,
        published_bytes: after
            .checkpoint_bytes_written
            .saturating_sub(before.checkpoint_bytes_written)
            .saturating_add(
                after
                    .event_bytes_appended
                    .saturating_sub(before.event_bytes_appended),
            ),
        checkpoints: after
            .checkpoints_written
            .saturating_sub(before.checkpoints_written),
        registry: serde_json::to_value(recovered).unwrap(),
        event_count: events.len(),
    }
}

#[test]
#[ignore = "release-only PER-01 persistence-owner benchmark; run explicitly with --ignored --nocapture"]
fn benchmark_task_persistence_owner() {
    let (fixture, fixture_registry_bytes) = benchmark_registry();
    assert!(fixture_registry_bytes.abs_diff(BENCHMARK_TARGET_REGISTRY_BYTES) < 1024);

    let mut legacy_lock_nanos = Vec::new();
    let mut legacy_elapsed_micros = Vec::new();
    let mut legacy_published_bytes = Vec::new();
    let mut owned_lock_nanos = Vec::new();
    let mut owned_elapsed_micros = Vec::new();
    let mut owned_published_bytes = Vec::new();
    let mut owned_checkpoints = Vec::new();
    for _ in 0..BENCHMARK_REPEATS {
        let legacy = run_legacy_persistence_sample(&fixture);
        let owned = run_owned_persistence_sample(&fixture);
        assert_eq!(legacy.registry, owned.registry);
        assert_eq!(legacy.event_count, owned.event_count);
        assert_eq!(legacy.event_count, (BENCHMARK_EVENTS + 1) as usize);
        legacy_lock_nanos.push(median(&mut legacy.lock_nanos.clone()));
        legacy_elapsed_micros.push(legacy.elapsed_micros);
        legacy_published_bytes.push(legacy.published_bytes);
        owned_lock_nanos.push(median(&mut owned.lock_nanos.clone()));
        owned_elapsed_micros.push(owned.elapsed_micros);
        owned_published_bytes.push(owned.published_bytes);
        owned_checkpoints.push(owned.checkpoints);
    }

    let result = json!({
        "schema_version": 1,
        "fixture_tasks": BENCHMARK_TASKS,
        "fixture_registry_bytes": fixture_registry_bytes,
        "measured_events": BENCHMARK_EVENTS,
        "repeats": BENCHMARK_REPEATS,
        "legacy_full_checkpoint_under_lock": {
            "median_event_lock_ns": median(&mut legacy_lock_nanos),
            "median_elapsed_us": median(&mut legacy_elapsed_micros),
            "median_published_bytes": median(&mut legacy_published_bytes),
            "median_checkpoints": BENCHMARK_EVENTS,
        },
        "owned_coalesced_checkpoint_after_lock": {
            "median_event_lock_ns": median(&mut owned_lock_nanos),
            "median_elapsed_us": median(&mut owned_elapsed_micros),
            "median_published_bytes": median(&mut owned_published_bytes),
            "median_checkpoints": median(&mut owned_checkpoints),
        },
        "parity_event_count": BENCHMARK_EVENTS + 1,
    });
    println!("PER01_RESULT={result}");
}
