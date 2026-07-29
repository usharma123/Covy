use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use context_kernel_mechanism::{
    ExecutionBudget, KernelError, KernelMechanism, KernelPacket, KernelResponse,
    KernelSequenceRequest, KernelStepRequest, ReactiveSequenceConfig, ReducerResult,
    SequenceObserver,
};

#[derive(Default)]
struct CancellingObserver {
    cancelled: bool,
    completed_steps: usize,
    cancel_after_completed_steps: Option<usize>,
}

impl SequenceObserver for CancellingObserver {
    fn should_cancel(&self) -> bool {
        self.cancelled
    }

    fn on_step_completed(
        &mut self,
        _position: usize,
        _step: &KernelStepRequest,
        _response: &KernelResponse,
    ) {
        self.completed_steps += 1;
        if self.cancel_after_completed_steps == Some(self.completed_steps) {
            self.cancelled = true;
        }
    }
}

fn two_step_sequence() -> KernelSequenceRequest {
    KernelSequenceRequest {
        budget: ExecutionBudget::default(),
        reactive: ReactiveSequenceConfig::default(),
        steps: vec![
            KernelStepRequest {
                id: "first".to_string(),
                target: "step.noop".to_string(),
                ..KernelStepRequest::default()
            },
            KernelStepRequest {
                id: "second".to_string(),
                target: "step.noop".to_string(),
                depends_on: vec!["first".to_string()],
                ..KernelStepRequest::default()
            },
        ],
    }
}

#[test]
fn cancellation_before_sequence_start_executes_no_reducer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_reducer = calls.clone();
    let mut kernel = KernelMechanism::new();
    kernel.register_reducer("step.noop", move |_ctx, _packets: &[KernelPacket]| {
        calls_for_reducer.fetch_add(1, Ordering::SeqCst);
        Ok(ReducerResult::default())
    });
    let mut observer = CancellingObserver {
        cancelled: true,
        ..CancellingObserver::default()
    };

    let error = kernel
        .execute_sequence_with_observer(two_step_sequence(), &mut observer)
        .expect_err("pre-cancelled sequence must stop");

    assert!(matches!(
        &error,
        KernelError::SequenceCancelled { task_id: None }
    ));
    assert_eq!(error.structured().code, "sequence_cancelled");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn cancellation_after_a_completed_step_stops_before_the_next_reducer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_reducer = calls.clone();
    let mut kernel = KernelMechanism::new();
    kernel.register_reducer("step.noop", move |_ctx, _packets: &[KernelPacket]| {
        calls_for_reducer.fetch_add(1, Ordering::SeqCst);
        Ok(ReducerResult::default())
    });
    let mut observer = CancellingObserver {
        cancel_after_completed_steps: Some(1),
        ..CancellingObserver::default()
    };

    let error = kernel
        .execute_sequence_with_observer(two_step_sequence(), &mut observer)
        .expect_err("sequence must stop at the completed-step boundary");

    assert!(matches!(
        &error,
        KernelError::SequenceCancelled { task_id: None }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(observer.completed_steps, 1);
}
