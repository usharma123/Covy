use super::support::{daemon_test_root, daemon_test_state, insert_admitted_task_record};
use super::*;
use crate::watch::install_watch;
use packet28_daemon_core::storage::{load_task_registry, load_watch_registry};
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

fn insert_task_generation(state: &Arc<Mutex<DaemonState>>, task_id: &str) -> TaskGenerationToken {
    insert_admitted_task_record(
        state,
        TaskRecord {
            task_id: task_id.to_string(),
            ..TaskRecord::default()
        },
    );
    state
        .lock()
        .unwrap()
        .task_generations
        .create(task_id)
        .unwrap()
}

fn wait_until_cancelled(generation: &TaskGenerationToken) {
    let started = Instant::now();
    while !generation.is_cancelled() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cancellation did not become visible"
        );
        thread::yield_now();
    }
}

#[test]
fn cancel_before_start_persists_terminal_history_and_rejects_stale_events() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    let generation = insert_task_generation(&state, "task-cancel-before-start");
    let (subscriber, mut receiver) = tokio::sync::mpsc::channel(1);
    state.lock().unwrap().subscribers.insert(
        "task-cancel-before-start".to_string(),
        vec![crate::state::TaskSubscriber {
            id: 1,
            sender: subscriber,
        }],
    );

    let (terminal, watch_ids) = cancel_task(state.clone(), "task-cancel-before-start").unwrap();
    let terminal = terminal.unwrap();
    assert_eq!(terminal.lifecycle, TaskLifecycle::Cancelled);
    assert_eq!(terminal.last_event_seq, 1);
    assert!(watch_ids.is_empty());
    assert!(generation.is_cancelled());

    let (terminal_again, watch_ids_again) =
        cancel_task(state.clone(), "task-cancel-before-start").unwrap();
    assert_eq!(terminal_again.unwrap().lifecycle, TaskLifecycle::Cancelled);
    assert!(watch_ids_again.is_empty());
    assert!(!emit_task_event_for_generation(
        state.clone(),
        "task-cancel-before-start",
        generation.id(),
        "task_completed",
        json!({"stale": true}),
    )
    .unwrap());
    assert_eq!(
        state.lock().unwrap().tasks.tasks["task-cancel-before-start"].lifecycle,
        TaskLifecycle::Cancelled
    );
    let cancellation = receiver.try_recv().unwrap();
    assert_eq!(cancellation.seq, 1);
    assert_eq!(cancellation.event.kind, "task_cancelled");
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
    flush_persistence(&state).unwrap();
    let persisted = load_task_registry(&root).unwrap();
    assert_eq!(
        persisted.tasks["task-cancel-before-start"].lifecycle,
        TaskLifecycle::Cancelled
    );
    let events = load_task_events(&root, "task-cancel-before-start").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.kind, "task_cancelled");

    let error = run_sequence_for_task(state.clone(), "task-cancel-before-start").unwrap_err();
    assert!(error.to_string().contains("cannot start task work"));
    assert!(state
        .lock()
        .unwrap()
        .task_generations
        .current("task-cancel-before-start")
        .is_none());
}

#[test]
fn failed_admission_rollback_quiesces_and_removes_runtime_and_durable_state() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    let task_id = "task-failed-admission";
    let watch_id = "watch-failed-admission";
    insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: task_id.to_string(),
            watch_ids: vec![watch_id.to_string()],
            ..TaskRecord::default()
        },
    );
    let generation = state
        .lock()
        .unwrap()
        .task_generations
        .create(task_id)
        .unwrap();
    let operation = generation
        .acquire_operation()
        .expect("failed admission generation should accept initial work");
    let (subscriber, mut receiver) = tokio::sync::mpsc::channel(1);
    {
        let mut guard = state.lock().unwrap();
        guard.watches.watches.push(WatchRegistration {
            watch_id: watch_id.to_string(),
            spec: WatchSpec {
                task_id: task_id.to_string(),
                ..WatchSpec::default()
            },
            ..WatchRegistration::default()
        });
        guard.subscribers.insert(
            task_id.to_string(),
            vec![crate::state::TaskSubscriber {
                id: 1,
                sender: subscriber,
            }],
        );
        persist_state_for_test(&guard).unwrap();
    }
    let (watch_tx, _watch_rx) = WatchIngress::new(1);
    install_watch(state.clone(), watch_tx, watch_id.to_string()).unwrap();
    assert!(state.lock().unwrap().watcher_handles.contains_key(watch_id));
    flush_persistence(&state).unwrap();

    let rollback_state = state.clone();
    let generation_id = generation.id();
    let rollback = thread::spawn(move || {
        rollback_failed_task_admission(rollback_state, task_id, generation_id, None)
    });
    wait_until_cancelled(&generation);
    assert!(
        !rollback.is_finished(),
        "rollback completed before the admitted operation quiesced"
    );
    drop(operation);
    rollback.join().unwrap().unwrap();

    let guard = state.lock().unwrap();
    assert!(!guard.tasks.tasks.contains_key(task_id));
    assert!(guard
        .watches
        .watches
        .iter()
        .all(|watch| watch.spec.task_id != task_id));
    assert!(!guard.subscribers.contains_key(task_id));
    assert!(guard.task_generations.current(task_id).is_none());
    assert!(!guard.watcher_handles.contains_key(watch_id));
    drop(guard);
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));

    let persisted_tasks = load_task_registry(&root).unwrap();
    assert!(!persisted_tasks.tasks.contains_key(task_id));
    let persisted_watches = load_watch_registry(&root).unwrap();
    assert!(persisted_watches
        .watches
        .iter()
        .all(|watch| watch.spec.task_id != task_id));
}

#[test]
fn watch_installation_rejects_cancelled_generation_without_publishing_handle() {
    let state = daemon_test_state();
    let task_id = "task-cancelled-watch-install";
    let watch_id = "watch-cancelled-install";
    insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: task_id.to_string(),
            watch_ids: vec![watch_id.to_string()],
            ..TaskRecord::default()
        },
    );
    let generation = state
        .lock()
        .unwrap()
        .task_generations
        .create(task_id)
        .unwrap();
    {
        let mut guard = state.lock().unwrap();
        guard.watches.watches.push(WatchRegistration {
            watch_id: watch_id.to_string(),
            spec: WatchSpec {
                task_id: task_id.to_string(),
                ..WatchSpec::default()
            },
            ..WatchRegistration::default()
        });
    }
    generation.request_cancel();
    let (watch_tx, _watch_rx) = WatchIngress::new(1);

    let error = install_watch(state.clone(), watch_tx, watch_id.to_string()).unwrap_err();

    assert!(error.to_string().contains("generation changed"));
    assert!(!state.lock().unwrap().watcher_handles.contains_key(watch_id));
}

#[test]
fn failed_admission_rollback_preserves_concurrent_terminal_cancellation() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    let task_id = "task-failed-admission-cancelled";
    let generation = insert_task_generation(&state, task_id);
    let generation_id = generation.id();

    let (terminal, _) = cancel_task(state.clone(), task_id).unwrap();
    assert_eq!(terminal.unwrap().lifecycle, TaskLifecycle::Cancelled);
    rollback_failed_task_admission(state.clone(), task_id, generation_id, None).unwrap();
    flush_persistence(&state).unwrap();

    assert_eq!(
        state.lock().unwrap().tasks.tasks[task_id].lifecycle,
        TaskLifecycle::Cancelled
    );
    assert_eq!(
        load_task_registry(&root).unwrap().tasks[task_id].lifecycle,
        TaskLifecycle::Cancelled
    );
}

#[test]
fn failed_admission_rollback_restores_displaced_terminal_task() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    let task_id = "task-failed-admission-replacement";
    let old_generation = insert_task_generation(&state, task_id);
    let old_generation_id = old_generation.id();
    let (terminal, _) = cancel_task(state.clone(), task_id).unwrap();
    let terminal = terminal.unwrap();
    assert_eq!(terminal.lifecycle, TaskLifecycle::Cancelled);
    flush_persistence(&state).unwrap();

    let (watch_tx, _watch_rx) = WatchIngress::new(1);
    let admission = register_task_and_watches(
        state.clone(),
        watch_tx,
        TaskSubmitSpec {
            task_id: task_id.to_string(),
            sequence: context_kernel_core::KernelSequenceRequest {
                steps: vec![KernelStepRequest {
                    id: "replacement".to_string(),
                    target: "replacement.noop".to_string(),
                    ..KernelStepRequest::default()
                }],
                ..context_kernel_core::KernelSequenceRequest::default()
            },
            ..TaskSubmitSpec::default()
        },
    )
    .unwrap();
    rollback_failed_task_admission(state.clone(), task_id, old_generation_id, None).unwrap();
    assert!(state
        .lock()
        .unwrap()
        .task_generations
        .matches(task_id, admission.generation));
    assert!(emit_task_event_for_generation(
        state.clone(),
        task_id,
        admission.generation,
        "task_started",
        json!({"generation": "failed-replacement"}),
    )
    .unwrap());

    rollback_failed_task_admission(
        state.clone(),
        task_id,
        admission.generation,
        admission.replaced_task,
    )
    .unwrap();

    let restored = state.lock().unwrap().tasks.tasks[task_id].clone();
    assert_eq!(restored.lifecycle, TaskLifecycle::Cancelled);
    assert_eq!(restored.last_event_seq, terminal.last_event_seq + 1);
    assert!(state
        .lock()
        .unwrap()
        .task_generations
        .current(task_id)
        .is_none());
    let persisted = load_task_registry(&root).unwrap();
    assert_eq!(persisted.tasks[task_id].lifecycle, TaskLifecycle::Cancelled);
    assert_eq!(
        persisted.tasks[task_id].last_event_seq,
        restored.last_event_seq
    );
}

#[test]
fn initial_sequence_execution_rejects_stale_admission_generation() {
    let state = daemon_test_state();
    let task_id = "task-stale-initial-admission";
    let calls = Arc::new(AtomicUsize::new(0));
    let reducer_calls = calls.clone();
    let mut kernel = Kernel::new();
    kernel.register_reducer("initial.fenced", move |_context, _packets| {
        reducer_calls.fetch_add(1, Ordering::SeqCst);
        Ok(context_kernel_core::ReducerResult::default())
    });
    state.lock().unwrap().kernel = Arc::new(kernel);
    let spec = || TaskSubmitSpec {
        task_id: task_id.to_string(),
        sequence: context_kernel_core::KernelSequenceRequest {
            steps: vec![KernelStepRequest {
                id: "initial".to_string(),
                target: "initial.fenced".to_string(),
                ..KernelStepRequest::default()
            }],
            ..context_kernel_core::KernelSequenceRequest::default()
        },
        ..TaskSubmitSpec::default()
    };
    let (watch_tx, _watch_rx) = WatchIngress::new(1);
    let first = register_task_and_watches(state.clone(), watch_tx, spec()).unwrap();
    let (replacement_watch_tx, _replacement_watch_rx) = WatchIngress::new(1);
    let replacement =
        register_task_and_watches(state.clone(), replacement_watch_tx, spec()).unwrap();

    let stale_error =
        run_initial_sequence_for_task(state.clone(), task_id, first.generation).unwrap_err();
    assert!(stale_error.to_string().contains("cancelled"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    run_initial_sequence_for_task(state, task_id, replacement.generation).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn cancel_waits_for_inflight_completion_and_same_id_reuse_rejects_old_generation() {
    let state = daemon_test_state();
    let old_generation = insert_task_generation(&state, "task-generation-race");
    let sequence_lease = old_generation
        .acquire_operation()
        .expect("active generation should accept work");
    let state_for_cancel = state.clone();
    let cancel_thread =
        thread::spawn(move || cancel_task(state_for_cancel, "task-generation-race"));

    wait_until_cancelled(&old_generation);
    assert!(!emit_task_event_for_generation(
        state.clone(),
        "task-generation-race",
        old_generation.id(),
        "task_completed",
        json!({"stale": true}),
    )
    .unwrap());
    drop(sequence_lease);
    let (terminal, _) = cancel_thread.join().unwrap().unwrap();
    assert_eq!(terminal.unwrap().lifecycle, TaskLifecycle::Cancelled);

    let (watch_tx, _watch_rx) = WatchIngress::new(4);
    let admission = register_task_and_watches(
        state.clone(),
        watch_tx,
        TaskSubmitSpec {
            task_id: "task-generation-race".to_string(),
            sequence: context_kernel_core::KernelSequenceRequest {
                steps: vec![KernelStepRequest {
                    id: "replacement".to_string(),
                    target: "replacement.noop".to_string(),
                    ..KernelStepRequest::default()
                }],
                ..context_kernel_core::KernelSequenceRequest::default()
            },
            ..TaskSubmitSpec::default()
        },
    )
    .unwrap();
    assert_eq!(admission.task.lifecycle, TaskLifecycle::Idle);
    assert!(admission.watches.is_empty());
    let new_generation = state
        .lock()
        .unwrap()
        .task_generations
        .current("task-generation-race")
        .unwrap();
    assert_ne!(old_generation.id(), new_generation.id());
    assert!(!emit_task_event_for_generation(
        state.clone(),
        "task-generation-race",
        old_generation.id(),
        "task_completed",
        json!({"generation": "old"}),
    )
    .unwrap());
    assert!(emit_task_event_for_generation(
        state.clone(),
        "task-generation-race",
        new_generation.id(),
        "task_started",
        json!({"generation": "new"}),
    )
    .unwrap());
    let guard = state.lock().unwrap();
    let task = guard.tasks.tasks.get("task-generation-race").unwrap();
    assert_eq!(task.last_event_seq, 2);
}

#[test]
fn cancel_between_steps_stops_the_next_reducer_and_suppresses_completion() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    let generation = insert_task_generation(&state, "task-between-steps");
    let calls = Arc::new(AtomicUsize::new(0));
    let first_calls = calls.clone();
    let second_calls = calls.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let release_for_reducer = release_rx.clone();
    let mut kernel = Kernel::new();
    kernel.register_reducer("step.blocking", move |_ctx, _packets| {
        first_calls.fetch_add(1, Ordering::SeqCst);
        started_tx.send(()).unwrap();
        release_for_reducer
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        Ok(context_kernel_core::ReducerResult::default())
    });
    kernel.register_reducer("step.after", move |_ctx, _packets| {
        second_calls.fetch_add(1, Ordering::SeqCst);
        Ok(context_kernel_core::ReducerResult::default())
    });
    {
        let mut guard = state.lock().unwrap();
        guard.kernel = Arc::new(kernel);
        let task = guard.tasks.tasks.get_mut("task-between-steps").unwrap();
        task.sequence_present = true;
        task.sequence = Some(context_kernel_core::KernelSequenceRequest {
            budget: context_kernel_core::ExecutionBudget::default(),
            reactive: context_kernel_core::ReactiveSequenceConfig::default(),
            steps: vec![
                KernelStepRequest {
                    id: "blocking".to_string(),
                    target: "step.blocking".to_string(),
                    ..KernelStepRequest::default()
                },
                KernelStepRequest {
                    id: "after".to_string(),
                    target: "step.after".to_string(),
                    depends_on: vec!["blocking".to_string()],
                    ..KernelStepRequest::default()
                },
            ],
        });
    }

    let state_for_run = state.clone();
    let run_thread =
        thread::spawn(move || run_sequence_for_task(state_for_run, "task-between-steps"));
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let state_for_cancel = state.clone();
    let cancel_thread = thread::spawn(move || cancel_task(state_for_cancel, "task-between-steps"));
    wait_until_cancelled(&generation);
    release_tx.send(()).unwrap();

    let run_error = run_thread.join().unwrap().unwrap_err();
    assert!(run_error.to_string().contains("cancelled"));
    let (terminal, _) = cancel_thread.join().unwrap().unwrap();
    assert_eq!(terminal.unwrap().lifecycle, TaskLifecycle::Cancelled);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(load_task_events(&root, "task-between-steps")
        .unwrap()
        .iter()
        .all(|frame| {
            !matches!(
                frame.event.kind.as_str(),
                "step_completed" | "task_completed"
            )
        }));
}

#[test]
fn cancel_terminates_process_group_and_waits_for_child_reap() {
    let state = daemon_test_state();
    let generation = insert_task_generation(&state, "task-child-cancel");
    let root = daemon_test_root(&state);
    let ready_path = root.join("child-ready");
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("trap '' TERM; sleep 30 & printf ready > \"$1\"; wait")
        .arg("packet28-child-test")
        .arg(&ready_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0);
    let child = command.spawn().unwrap();
    let pid = child.id();
    let process_group = i32::try_from(pid).unwrap();
    assert!(generation.register_child(OwnedChildProcess { pid, process_group }));
    {
        let mut guard = state.lock().unwrap();
        guard
            .tasks
            .tasks
            .get_mut("task-child-cancel")
            .unwrap()
            .latest_agent_pid = Some(pid);
    }
    crate::launch::spawn_owned_child_waiter(
        state.clone(),
        "task-child-cancel".to_string(),
        generation,
        OwnedChildProcess { pid, process_group },
        child,
    )
    .unwrap();

    let ready_started = Instant::now();
    while !ready_path.exists() {
        assert!(
            ready_started.elapsed() < Duration::from_secs(2),
            "child process did not become ready"
        );
        thread::yield_now();
    }

    let cancel_started = Instant::now();
    let (terminal, _) = cancel_task(state.clone(), "task-child-cancel").unwrap();
    assert_eq!(terminal.unwrap().lifecycle, TaskLifecycle::Cancelled);
    assert!(
        cancel_started.elapsed() < Duration::from_secs(10),
        "child cancellation exceeded the bounded reap window"
    );
    assert_eq!(
        state.lock().unwrap().tasks.tasks["task-child-cancel"].lifecycle,
        TaskLifecycle::Cancelled
    );
    assert!(load_task_events(&root, "task-child-cancel")
        .unwrap()
        .iter()
        .all(|frame| frame.event.kind != "task.agent_launch_completed"));

    // SAFETY: signal 0 only probes the process group created by this test.
    let probe_result = unsafe { libc::kill(-process_group, 0) };
    assert_eq!(probe_result, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}
