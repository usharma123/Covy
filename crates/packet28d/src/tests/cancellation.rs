use super::support::{daemon_test_root, daemon_test_state};
use super::*;
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicUsize, Ordering};

fn insert_task_generation(state: &Arc<Mutex<DaemonState>>, task_id: &str) -> TaskGenerationToken {
    let mut guard = state.lock().unwrap();
    guard.tasks.tasks.insert(
        task_id.to_string(),
        TaskRecord {
            task_id: task_id.to_string(),
            ..TaskRecord::default()
        },
    );
    guard.task_generations.create(task_id).unwrap()
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
fn cancel_before_start_is_idempotent_and_stale_events_do_not_resurrect_task() {
    let state = daemon_test_state();
    let generation = insert_task_generation(&state, "task-cancel-before-start");
    let (subscriber, receiver) = mpsc::channel();
    state
        .lock()
        .unwrap()
        .subscribers
        .insert("task-cancel-before-start".to_string(), vec![subscriber]);

    let (removed, watch_ids) = cancel_task(state.clone(), "task-cancel-before-start").unwrap();
    assert!(removed.is_some());
    assert!(watch_ids.is_empty());
    assert!(generation.is_cancelled());

    let (removed_again, watch_ids_again) =
        cancel_task(state.clone(), "task-cancel-before-start").unwrap();
    assert!(removed_again.is_none());
    assert!(watch_ids_again.is_empty());
    assert!(!emit_task_event_for_generation(
        state.clone(),
        "task-cancel-before-start",
        generation.id(),
        "task_completed",
        json!({"stale": true}),
    )
    .unwrap());
    assert!(!state
        .lock()
        .unwrap()
        .tasks
        .tasks
        .contains_key("task-cancel-before-start"));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(10)),
        Err(RecvTimeoutError::Disconnected)
    ));
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
    let (removed, _) = cancel_thread.join().unwrap().unwrap();
    assert!(removed.is_some());

    let new_generation = insert_task_generation(&state, "task-generation-race");
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
    assert_eq!(task.last_event_seq, 1);
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
    let (removed, _) = cancel_thread.join().unwrap().unwrap();
    assert!(removed.is_some());
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
    let (removed, _) = cancel_task(state.clone(), "task-child-cancel").unwrap();
    assert!(removed.is_some());
    assert!(
        cancel_started.elapsed() < Duration::from_secs(10),
        "child cancellation exceeded the bounded reap window"
    );
    assert!(!state
        .lock()
        .unwrap()
        .tasks
        .tasks
        .contains_key("task-child-cancel"));
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
