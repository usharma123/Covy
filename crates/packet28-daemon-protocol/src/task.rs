//! Task lifecycle, launch, watch, and registry payloads.

use super::*;

/// Runtime task state represented on the legacy wire as three boolean fields.
///
/// The enum prevents callers from constructing contradictory combinations such
/// as a task that is both cancelled and pending a replan. Serialization keeps
/// the original `running`, `cancel_requested`, and `pending_replan` fields so
/// existing task registries and clients remain compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskLifecycle {
    /// The task is registered but has no work in flight.
    #[default]
    Idle,
    /// Work is queued because a watch requested a replan.
    ReplanPending,
    /// The stored sequence is currently executing.
    Running,
    /// The sequence is executing and must run again after it completes.
    RunningReplanPending,
    /// Cancellation owns the task and no new work may start.
    Cancelling {
        /// Whether sequence work was active when cancellation began.
        was_running: bool,
    },
}

impl TaskLifecycle {
    /// Returns whether sequence work is currently active.
    pub const fn is_running(self) -> bool {
        matches!(
            self,
            Self::Running | Self::RunningReplanPending | Self::Cancelling { was_running: true }
        )
    }

    /// Returns whether cancellation has been requested.
    pub const fn is_cancelling(self) -> bool {
        matches!(self, Self::Cancelling { .. })
    }

    /// Returns whether another sequence run is pending.
    pub const fn has_pending_replan(self) -> bool {
        matches!(self, Self::ReplanPending | Self::RunningReplanPending)
    }

    /// Starts one sequence generation.
    ///
    /// # Errors
    ///
    /// Returns [`TaskLifecycleTransitionError`] when work is already running or
    /// cancellation owns the task.
    pub fn start(&mut self) -> Result<(), TaskLifecycleTransitionError> {
        match *self {
            Self::Idle | Self::ReplanPending => {
                *self = Self::Running;
                Ok(())
            }
            from => Err(TaskLifecycleTransitionError {
                from,
                action: TaskLifecycleAction::Start,
            }),
        }
    }

    /// Requests a replan without starting duplicate work.
    ///
    /// # Errors
    ///
    /// Returns [`TaskLifecycleTransitionError`] when cancellation owns the
    /// task.
    pub fn request_replan(&mut self) -> Result<bool, TaskLifecycleTransitionError> {
        let should_start = match *self {
            Self::Idle => {
                *self = Self::ReplanPending;
                true
            }
            Self::Running => {
                *self = Self::RunningReplanPending;
                false
            }
            Self::ReplanPending | Self::RunningReplanPending => false,
            Self::Cancelling { .. } => {
                return Err(TaskLifecycleTransitionError {
                    from: *self,
                    action: TaskLifecycleAction::RequestReplan,
                });
            }
        };
        Ok(should_start)
    }

    /// Completes the active generation and reports whether a rerun is pending.
    ///
    /// # Errors
    ///
    /// Returns [`TaskLifecycleTransitionError`] when no generation is running.
    pub fn finish_run(&mut self) -> Result<bool, TaskLifecycleTransitionError> {
        match *self {
            Self::Running => {
                *self = Self::Idle;
                Ok(false)
            }
            Self::RunningReplanPending => {
                *self = Self::ReplanPending;
                Ok(true)
            }
            from => Err(TaskLifecycleTransitionError {
                from,
                action: TaskLifecycleAction::FinishRun,
            }),
        }
    }

    /// Moves the task into its terminal cancellation transition.
    ///
    /// Returns `true` only for the first cancellation request.
    pub fn request_cancel(&mut self) -> bool {
        if self.is_cancelling() {
            return false;
        }
        *self = Self::Cancelling {
            was_running: self.is_running(),
        };
        true
    }

    const fn legacy_flags(self) -> TaskLifecycleWire {
        TaskLifecycleWire {
            running: self.is_running(),
            cancel_requested: self.is_cancelling(),
            pending_replan: self.has_pending_replan(),
        }
    }

    const fn from_legacy_flags(flags: TaskLifecycleWire) -> Self {
        match (flags.running, flags.cancel_requested, flags.pending_replan) {
            (running, true, _) => Self::Cancelling {
                was_running: running,
            },
            (true, false, true) => Self::RunningReplanPending,
            (true, false, false) => Self::Running,
            (false, false, true) => Self::ReplanPending,
            (false, false, false) => Self::Idle,
        }
    }
}

impl Serialize for TaskLifecycle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let flags = self.legacy_flags();
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("running", &flags.running)?;
        map.serialize_entry("cancel_requested", &flags.cancel_requested)?;
        map.serialize_entry("pending_replan", &flags.pending_replan)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for TaskLifecycle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        TaskLifecycleWire::deserialize(deserializer).map(Self::from_legacy_flags)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(default)]
struct TaskLifecycleWire {
    running: bool,
    cancel_requested: bool,
    pending_replan: bool,
}

/// Task lifecycle operation rejected by the current runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cannot {action} while task lifecycle is {from:?}")]
pub struct TaskLifecycleTransitionError {
    /// State in which the operation was attempted.
    pub from: TaskLifecycle,
    /// Operation that was rejected.
    pub action: TaskLifecycleAction,
}

/// Operations that can change a [`TaskLifecycle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskLifecycleAction {
    /// Start one sequence generation.
    Start,
    /// Queue a replan.
    RequestReplan,
    /// Complete the active sequence generation.
    FinishRun,
}

impl std::fmt::Display for TaskLifecycleAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Start => "start task work",
            Self::RequestReplan => "request a task replan",
            Self::FinishRun => "finish task work",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TaskAwaitHandoffRequest {
    pub task_id: String,
    pub timeout_ms: Option<u64>,
    pub poll_ms: Option<u64>,
    pub after_context_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TaskAwaitHandoffResponse {
    pub task_status: BrokerTaskStatusResponse,
    pub waited_ms: u64,
    pub polls: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TaskMarkHandoffConsumedRequest {
    pub task_id: String,
    pub handoff_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TaskMarkHandoffConsumedResponse {
    pub handoff: Option<BrokerHandoffDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TaskLaunchAgentRequest {
    pub task_id: String,
    pub task: Option<String>,
    pub wait_for_handoff: bool,
    pub handoff_timeout_ms: Option<u64>,
    pub handoff_poll_ms: Option<u64>,
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TaskLaunchAgentResponse {
    pub task_id: String,
    pub pid: u32,
    pub bootstrap_mode: String,
    pub bootstrap_path: String,
    pub log_path: String,
    pub handoff_id: Option<String>,
    pub handoff_artifact_id: Option<String>,
    pub handoff_checkpoint_id: Option<String>,
    pub started_at_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TaskRecord {
    pub task_id: String,
    #[serde(flatten)]
    pub lifecycle: TaskLifecycle,
    pub last_request_id: Option<u64>,
    pub last_started_at_unix: Option<u64>,
    pub last_completed_at_unix: Option<u64>,
    pub last_replan_at_unix: Option<u64>,
    pub last_error: Option<String>,
    pub watch_ids: Vec<String>,
    pub sequence_present: bool,
    pub sequence: Option<KernelSequenceRequest>,
    pub last_sequence_metadata: Option<Value>,
    pub last_event_seq: u64,
    pub last_context_refresh_at_unix: Option<u64>,
    pub working_set_est_tokens: u64,
    pub evictable_est_tokens: u64,
    pub changed_since_checkpoint_paths: usize,
    pub changed_since_checkpoint_symbols: usize,
    pub latest_context_version: Option<String>,
    pub latest_brief_path: Option<String>,
    pub latest_brief_hash: Option<String>,
    pub latest_brief_generated_at_unix: Option<u64>,
    pub latest_context_reason: Option<String>,
    pub latest_handoff_id: Option<String>,
    pub latest_handoff_artifact_id: Option<String>,
    pub latest_handoff_generated_at_unix: Option<u64>,
    pub latest_handoff_checkpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handoffs: Vec<BrokerHandoffDescriptor>,
    pub latest_agent_pid: Option<u32>,
    pub latest_agent_bootstrap_mode: Option<String>,
    pub latest_agent_log_path: Option<String>,
    pub latest_agent_started_at_unix: Option<u64>,
    pub latest_agent_completed_at_unix: Option<u64>,
    pub latest_agent_exit_code: Option<i32>,
    pub latest_agent_context_version: Option<String>,
    pub latest_agent_handoff_artifact_id: Option<String>,
    pub latest_agent_handoff_checkpoint_id: Option<String>,
    pub latest_hook_session_id: Option<String>,
    pub latest_hook_event_at_unix: Option<u64>,
    pub latest_hook_boundary_at_unix: Option<u64>,
    pub latest_hook_boundary_kind: Option<String>,
    pub latest_hook_bootstrap_context_version: Option<String>,
    pub latest_hook_bootstrap_at_unix: Option<u64>,
    pub hook_window_est_tokens: u64,
    pub hook_window_est_bytes: u64,
    pub hook_soft_threshold_tokens: u64,
    pub hook_threshold_exceeded: bool,
    pub latest_hook_handoff_reason: Option<String>,
    pub latest_hook_command_id: Option<String>,
    pub latest_hook_command_kind: Option<String>,
    pub latest_hook_progress_at_unix: Option<u64>,
    pub hook_git_epoch: u64,
    pub hook_fs_epoch: u64,
    pub hook_rust_epoch: u64,
    pub hook_reducer_cache: BTreeMap<String, HookReducerCacheEntry>,
    pub latest_broker_request: Option<BrokerGetContextRequest>,
    pub linked_decisions: BTreeMap<String, String>,
    pub resolved_questions: BTreeMap<String, String>,
    pub question_texts: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WatchRegistration {
    pub watch_id: String,
    pub spec: WatchSpec,
    pub active: bool,
    pub last_event_at_unix: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WatchRegistry {
    pub watches: Vec<WatchRegistration>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TaskRegistry {
    pub tasks: BTreeMap<String, TaskRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_accepts_only_idle_or_queued_work() {
        for (initial, expected) in [
            (TaskLifecycle::Idle, Ok(TaskLifecycle::Running)),
            (TaskLifecycle::ReplanPending, Ok(TaskLifecycle::Running)),
            (TaskLifecycle::Running, Err(TaskLifecycle::Running)),
            (
                TaskLifecycle::RunningReplanPending,
                Err(TaskLifecycle::RunningReplanPending),
            ),
            (
                TaskLifecycle::Cancelling { was_running: false },
                Err(TaskLifecycle::Cancelling { was_running: false }),
            ),
        ] {
            let mut lifecycle = initial;
            let result = lifecycle.start();
            match expected {
                Ok(expected) => {
                    assert!(result.is_ok(), "start failed from {initial:?}: {result:?}");
                    assert_eq!(lifecycle, expected);
                }
                Err(expected_from) => {
                    assert_eq!(
                        result.unwrap_err(),
                        TaskLifecycleTransitionError {
                            from: expected_from,
                            action: TaskLifecycleAction::Start,
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn request_replan_queues_exactly_one_sequence_owner() {
        for (initial, expected, should_start) in [
            (TaskLifecycle::Idle, TaskLifecycle::ReplanPending, true),
            (
                TaskLifecycle::ReplanPending,
                TaskLifecycle::ReplanPending,
                false,
            ),
            (
                TaskLifecycle::Running,
                TaskLifecycle::RunningReplanPending,
                false,
            ),
            (
                TaskLifecycle::RunningReplanPending,
                TaskLifecycle::RunningReplanPending,
                false,
            ),
        ] {
            let mut lifecycle = initial;
            assert_eq!(lifecycle.request_replan().unwrap(), should_start);
            assert_eq!(lifecycle, expected);
        }
    }

    #[test]
    fn request_replan_rejects_cancelling_task() {
        let mut lifecycle = TaskLifecycle::Cancelling { was_running: true };

        assert_eq!(
            lifecycle.request_replan().unwrap_err(),
            TaskLifecycleTransitionError {
                from: TaskLifecycle::Cancelling { was_running: true },
                action: TaskLifecycleAction::RequestReplan,
            }
        );
    }

    #[test]
    fn finish_run_preserves_only_a_pending_rerun() {
        for (initial, expected, rerun) in [
            (TaskLifecycle::Running, TaskLifecycle::Idle, false),
            (
                TaskLifecycle::RunningReplanPending,
                TaskLifecycle::ReplanPending,
                true,
            ),
        ] {
            let mut lifecycle = initial;
            assert_eq!(lifecycle.finish_run().unwrap(), rerun);
            assert_eq!(lifecycle, expected);
        }
    }

    #[test]
    fn finish_run_rejects_states_without_active_work() {
        for initial in [
            TaskLifecycle::Idle,
            TaskLifecycle::ReplanPending,
            TaskLifecycle::Cancelling { was_running: false },
            TaskLifecycle::Cancelling { was_running: true },
        ] {
            let mut lifecycle = initial;
            assert_eq!(
                lifecycle.finish_run().unwrap_err(),
                TaskLifecycleTransitionError {
                    from: initial,
                    action: TaskLifecycleAction::FinishRun,
                }
            );
        }
    }

    #[test]
    fn request_cancel_is_idempotent_and_removes_pending_replan() {
        let mut lifecycle = TaskLifecycle::RunningReplanPending;

        assert!(lifecycle.request_cancel());
        assert_eq!(lifecycle, TaskLifecycle::Cancelling { was_running: true });
        assert!(!lifecycle.request_cancel());
    }

    #[test]
    fn task_record_preserves_legacy_lifecycle_wire_fields() {
        let legacy = serde_json::json!({
            "task_id": "task-1",
            "running": true,
            "cancel_requested": false,
            "pending_replan": true
        });
        let record: TaskRecord = serde_json::from_value(legacy).unwrap();

        assert_eq!(record.lifecycle, TaskLifecycle::RunningReplanPending);
        let encoded = serde_json::to_value(record).unwrap();
        assert_eq!(encoded["running"], serde_json::json!(true));
        assert_eq!(encoded["cancel_requested"], serde_json::json!(false));
        assert_eq!(encoded["pending_replan"], serde_json::json!(true));
        assert!(encoded.get("lifecycle").is_none());
    }

    #[test]
    fn every_typed_lifecycle_roundtrips_through_legacy_flags() {
        for lifecycle in [
            TaskLifecycle::Idle,
            TaskLifecycle::ReplanPending,
            TaskLifecycle::Running,
            TaskLifecycle::RunningReplanPending,
            TaskLifecycle::Cancelling { was_running: false },
            TaskLifecycle::Cancelling { was_running: true },
        ] {
            let record = TaskRecord {
                task_id: "task-1".to_string(),
                lifecycle,
                ..TaskRecord::default()
            };
            let encoded = serde_json::to_vec(&record).unwrap();
            let decoded: TaskRecord = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(decoded.lifecycle, lifecycle);
        }
    }

    #[test]
    fn contradictory_legacy_cancel_flags_are_canonicalized() {
        let record: TaskRecord = serde_json::from_value(serde_json::json!({
            "task_id": "task-1",
            "running": false,
            "cancel_requested": true,
            "pending_replan": true
        }))
        .unwrap();

        assert_eq!(
            record.lifecycle,
            TaskLifecycle::Cancelling { was_running: false }
        );
        assert!(!record.lifecycle.has_pending_replan());
    }
}
