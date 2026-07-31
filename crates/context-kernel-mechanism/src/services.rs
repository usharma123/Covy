use std::collections::BTreeSet;
use std::sync::Arc;

use context_memory_core::PacketCacheEntry;
use serde_json::Value;

use super::{
    GovernanceAudit, KernelError, KernelPacket, KernelRequest, KernelStepRequest,
    ReactiveReplanMode,
};

/// Per-request policy state used by the kernel execution mechanism.
pub trait ExecutionPolicyRun: Send {
    /// Returns whether this request may use the packet cache.
    fn cache_enabled(&self) -> bool;

    /// Returns the canonical cache identity for this request.
    fn cache_input(&self) -> &Value;

    /// Audits reducer output and updates the reported governance state.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] when output violates the configured policy.
    fn audit_output(&mut self, packets: &[KernelPacket]) -> Result<(), KernelError>;

    /// Returns the current serializable governance audit.
    fn governance_audit(&self) -> GovernanceAudit;
}

/// Starts request-scoped governance and cache policy.
pub trait ExecutionPolicy: Send + Sync {
    /// Validates a request and creates the state used for output auditing.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] when request policy is invalid or rejects the
    /// reducer or its input packets.
    fn begin(
        &self,
        target: &str,
        request: &KernelRequest,
    ) -> Result<Box<dyn ExecutionPolicyRun>, KernelError>;
}

/// A target-neutral mutation proposed by a reactive sequence planner.
#[derive(Debug, Clone)]
pub enum KernelPlanMutation {
    /// Removes a pending step.
    Cancel {
        /// Pending step identifier.
        step_id: String,
        /// Stable diagnostic reason.
        reason: String,
    },
    /// Replaces a pending step with the same identifier.
    Replace {
        /// Replacement request.
        step: KernelStepRequest,
        /// Stable diagnostic reason.
        reason: String,
    },
    /// Appends a new pending step.
    Append {
        /// Appended request.
        step: KernelStepRequest,
        /// Stable diagnostic reason.
        reason: String,
    },
}

/// Read-only inputs supplied to a reactive sequence planner.
pub struct ReactivePlanRequest<'a> {
    /// Sequence task identity.
    pub task_id: &'a str,
    /// Steps that have not yet completed.
    pub remaining: &'a [KernelStepRequest],
    /// Normalized sequence plan before reactive mutations.
    pub original_steps: &'a [KernelStepRequest],
    /// Step identifiers completed successfully in this run.
    pub completed_success: &'a BTreeSet<String>,
    /// Requested replanning mode.
    pub mode: ReactiveReplanMode,
    /// Whether a focused map follow-up may be appended.
    pub append_focused_map: bool,
    /// Step that triggered this replan, if any.
    pub anchor_step_id: Option<&'a str>,
    /// Snapshot of cache entries taken by the mechanism.
    pub cache_entries: &'a [PacketCacheEntry],
}

/// A reactive plan result applied by the scheduling mechanism.
#[derive(Debug, Clone, Default)]
pub struct ReactivePlan {
    /// Monotonic task-state event count observed by the planner.
    pub event_count: usize,
    /// Mutations to apply to the pending plan.
    pub mutations: Vec<KernelPlanMutation>,
}

/// Supplies composition-specific reactive sequence decisions.
pub trait ReactivePlanner: Send + Sync {
    /// Builds a replan from the current task state and pending steps.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] when task state cannot be interpreted.
    fn plan(&self, request: ReactivePlanRequest<'_>) -> Result<ReactivePlan, KernelError>;
}

/// Execution and replanning services injected into [`KernelMechanism`].
///
/// [`KernelMechanism`]: crate::KernelMechanism
#[derive(Clone)]
pub struct KernelServices {
    pub(crate) execution_policy: Arc<dyn ExecutionPolicy>,
    pub(crate) reactive_planner: Arc<dyn ReactivePlanner>,
}

impl KernelServices {
    /// Creates a service set from explicit policy and replanning providers.
    pub fn new(
        execution_policy: Arc<dyn ExecutionPolicy>,
        reactive_planner: Arc<dyn ReactivePlanner>,
    ) -> Self {
        Self {
            execution_policy,
            reactive_planner,
        }
    }
}

impl Default for KernelServices {
    fn default() -> Self {
        Self::new(
            Arc::new(DefaultExecutionPolicy),
            Arc::new(NoopReactivePlanner),
        )
    }
}

#[derive(Debug, Default)]
struct DefaultExecutionPolicy;

impl ExecutionPolicy for DefaultExecutionPolicy {
    fn begin(
        &self,
        target: &str,
        request: &KernelRequest,
    ) -> Result<Box<dyn ExecutionPolicyRun>, KernelError> {
        Ok(Box::new(DefaultExecutionPolicyRun {
            cache_enabled: !request
                .policy_context
                .get("disable_cache")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cache_input: super::default_cache_input(target, request),
        }))
    }
}

struct DefaultExecutionPolicyRun {
    cache_enabled: bool,
    cache_input: Value,
}

impl ExecutionPolicyRun for DefaultExecutionPolicyRun {
    fn cache_enabled(&self) -> bool {
        self.cache_enabled
    }

    fn cache_input(&self) -> &Value {
        &self.cache_input
    }

    fn audit_output(&mut self, _packets: &[KernelPacket]) -> Result<(), KernelError> {
        Ok(())
    }

    fn governance_audit(&self) -> GovernanceAudit {
        GovernanceAudit::default()
    }
}

#[derive(Debug, Default)]
struct NoopReactivePlanner;

impl ReactivePlanner for NoopReactivePlanner {
    fn plan(&self, _request: ReactivePlanRequest<'_>) -> Result<ReactivePlan, KernelError> {
        Ok(ReactivePlan::default())
    }
}
