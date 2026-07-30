use super::*;
use crate::watch::run_sequence_for_task_with_executor;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

fn successful_response(request_id: u64) -> context_kernel_core::KernelSequenceResponse {
    context_kernel_core::KernelSequenceResponse {
        request_id,
        scheduled: Vec::new(),
        skipped: Vec::new(),
        budget_exhausted: false,
        step_results: Vec::new(),
        metadata: json!({}),
    }
}

#[test]
fn failed_run_retains_ownership_and_executes_one_durable_rerun() {
    let state = super::support::daemon_test_state();
    let root = super::support::daemon_test_root(&state);
    let calls = Arc::new(AtomicUsize::new(0));
    let reducer_calls = calls.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    super::support::insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: "failed-rerun".to_string(),
            sequence_present: true,
            sequence: Some(context_kernel_core::KernelSequenceRequest {
                steps: vec![KernelStepRequest {
                    id: "fail-once".to_string(),
                    target: "replan.fail-once".to_string(),
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
            "failed-rerun",
            None,
            move |_kernel, _sequence, _observer| {
                let ordinal = reducer_calls.fetch_add(1, Ordering::SeqCst);
                if ordinal == 0 {
                    started_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap();
                    return Err(context_kernel_core::KernelError::ReducerFailed {
                        target: "replan.fail-once".to_string(),
                        detail: "injected first-attempt failure".to_string(),
                    });
                }
                Ok(successful_response(2))
            },
        )
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    {
        let mut guard = state.lock().unwrap();
        let should_start = guard
            .tasks
            .tasks
            .get_mut("failed-rerun")
            .unwrap()
            .lifecycle
            .request_replan()
            .unwrap();
        assert!(
            !should_start,
            "active run must retain ownership of its rerun"
        );
        assert_eq!(
            guard.tasks.tasks["failed-rerun"].lifecycle,
            TaskLifecycle::RunningReplanPending
        );
        persist_state_for_test(&guard).unwrap();
    }
    flush_persistence(&state).unwrap();
    release_tx.send(()).unwrap();

    run.join().unwrap().unwrap().unwrap();
    flush_persistence(&state).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        state.lock().unwrap().tasks.tasks["failed-rerun"].lifecycle,
        TaskLifecycle::Idle
    );
    let events = load_task_events(&root, "failed-rerun").unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|frame| frame.event.kind == "task_started")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|frame| frame.event.kind == "task_failed")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|frame| frame.event.kind == "task_completed")
            .count(),
        1
    );
}

#[test]
fn failed_event_append_does_not_abandon_owned_rerun() {
    let state = super::support::daemon_test_state();
    let root = super::support::daemon_test_root(&state);
    let task_id = "failed-event-rerun";
    let event_path = task_event_log_path(&root, &task_storage_id(task_id).unwrap());
    let saved_event_path = event_path.with_extension("jsonl.saved");
    let calls = Arc::new(AtomicUsize::new(0));
    let executor_calls = calls.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    super::support::insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: task_id.to_string(),
            sequence_present: true,
            sequence: Some(context_kernel_core::KernelSequenceRequest {
                steps: vec![KernelStepRequest {
                    id: "fail-event-once".to_string(),
                    target: "replan.fail-event-once".to_string(),
                    ..KernelStepRequest::default()
                }],
                ..context_kernel_core::KernelSequenceRequest::default()
            }),
            ..TaskRecord::default()
        },
    );

    let run_state = state.clone();
    let executor_event_path = event_path.clone();
    let executor_saved_event_path = saved_event_path.clone();
    let run = thread::spawn(move || {
        run_sequence_for_task_with_executor(
            run_state,
            task_id,
            None,
            move |_kernel, _sequence, _observer| {
                let ordinal = executor_calls.fetch_add(1, Ordering::SeqCst);
                if ordinal == 0 {
                    started_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap();
                    return Err(context_kernel_core::KernelError::ReducerFailed {
                        target: "replan.fail-event-once".to_string(),
                        detail: "injected first-attempt failure".to_string(),
                    });
                }
                fs::remove_dir(&executor_event_path).unwrap();
                fs::rename(&executor_saved_event_path, &executor_event_path).unwrap();
                Ok(successful_response(2))
            },
        )
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
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
        persist_state_for_test(&guard).unwrap();
    }
    flush_persistence(&state).unwrap();
    fs::rename(&event_path, &saved_event_path).unwrap();
    fs::create_dir(&event_path).unwrap();
    release_tx.send(()).unwrap();

    run.join().unwrap().unwrap().unwrap();
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
            .filter(|frame| frame.event.kind == "task_failed")
            .count(),
        0
    );
    assert_eq!(
        events
            .iter()
            .filter(|frame| frame.event.kind == "task_completed")
            .count(),
        1
    );
}
