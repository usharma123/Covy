use super::*;
use packet28_daemon_core::storage::{
    append_next_task_event, append_task_event, load_task_registry,
    load_task_registry_with_event_tails, save_task_registry, save_watch_registry,
};
use std::fs::OpenOptions;
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
