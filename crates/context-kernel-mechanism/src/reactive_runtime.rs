use super::*;

pub(crate) fn policy_context_with_task_id(
    mut policy_context: Value,
    task_id: Option<&str>,
) -> Value {
    let Some(task_id) = task_id.filter(|task_id| !task_id.trim().is_empty()) else {
        return policy_context;
    };
    match &mut policy_context {
        Value::Object(map) => {
            map.entry("task_id".to_string())
                .or_insert_with(|| Value::String(task_id.to_string()));
            policy_context
        }
        Value::Null => json!({ "task_id": task_id }),
        other => json!({
            "task_id": task_id,
            "sequence_policy_context": other.clone(),
        }),
    }
}

pub(crate) fn kernel_step_estimate(
    step: &KernelStepRequest,
) -> context_scheduler_core::StepEstimate {
    let usage = usage_for_packets(&step.input_packets);
    context_scheduler_core::StepEstimate {
        tokens: usage.tokens,
        bytes: usage.bytes,
        runtime_ms: usage.runtime_ms,
    }
}

pub(crate) fn schedule_step_from_kernel(
    step: &KernelStepRequest,
) -> context_scheduler_core::ScheduleStep {
    context_scheduler_core::ScheduleStep {
        id: step.id.clone(),
        target: step.target.clone(),
        depends_on: step.depends_on.clone(),
        estimate: kernel_step_estimate(step),
    }
}

pub(crate) fn schedule_budget_remaining(
    budget: ExecutionBudget,
    consumed: context_scheduler_core::StepEstimate,
) -> context_scheduler_core::ScheduleBudget {
    context_scheduler_core::ScheduleBudget {
        token_cap: budget
            .token_cap
            .map(|cap| cap.saturating_sub(consumed.tokens)),
        byte_cap: budget
            .byte_cap
            .map(|cap| cap.saturating_sub(consumed.bytes)),
        runtime_ms_cap: budget
            .runtime_ms_cap
            .map(|cap| cap.saturating_sub(consumed.runtime_ms)),
    }
}

pub(crate) fn to_schedule_mutations(
    mutations: &[KernelPlanMutation],
) -> Vec<context_scheduler_core::ScheduleMutation> {
    mutations
        .iter()
        .map(|mutation| match mutation {
            KernelPlanMutation::Cancel { step_id, reason } => {
                context_scheduler_core::ScheduleMutation::Cancel {
                    step_id: step_id.clone(),
                    reason: reason.clone(),
                }
            }
            KernelPlanMutation::Replace { step, reason } => {
                context_scheduler_core::ScheduleMutation::Replace {
                    step: schedule_step_from_kernel(step),
                    reason: reason.clone(),
                }
            }
            KernelPlanMutation::Append { step, reason } => {
                context_scheduler_core::ScheduleMutation::Append {
                    step: schedule_step_from_kernel(step),
                    reason: reason.clone(),
                }
            }
        })
        .collect()
}

pub(crate) fn apply_kernel_mutations(
    steps: &[KernelStepRequest],
    mutations: &[KernelPlanMutation],
) -> Vec<KernelStepRequest> {
    let mut by_id = steps
        .iter()
        .cloned()
        .map(|step| (step.id.clone(), step))
        .collect::<HashMap<_, _>>();
    let mut order = steps.iter().map(|step| step.id.clone()).collect::<Vec<_>>();

    for mutation in mutations {
        match mutation {
            KernelPlanMutation::Cancel { step_id, .. } => {
                if by_id.remove(step_id).is_some() {
                    order.retain(|id| id != step_id);
                    for step in by_id.values_mut() {
                        step.depends_on.retain(|dep| dep != step_id);
                    }
                }
            }
            KernelPlanMutation::Replace { step, .. } => {
                if by_id.contains_key(&step.id) {
                    by_id.insert(step.id.clone(), step.clone());
                }
            }
            KernelPlanMutation::Append { step, .. } => {
                if !by_id.contains_key(&step.id) {
                    order.push(step.id.clone());
                    by_id.insert(step.id.clone(), step.clone());
                }
            }
        }
    }

    order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect()
}

pub(crate) fn remove_satisfied_dependency(remaining: &mut [KernelStepRequest], completed_id: &str) {
    for step in remaining {
        step.depends_on.retain(|dep| dep != completed_id);
    }
}

pub(crate) fn remove_failed_dependents(
    remaining: &mut Vec<KernelStepRequest>,
    failed_id: &str,
) -> Vec<KernelStepRequest> {
    let mut removed = Vec::new();
    let mut failed = vec![failed_id.to_string()];
    while let Some(dep_id) = failed.pop() {
        let (mut newly_removed, kept): (Vec<_>, Vec<_>) = remaining
            .drain(..)
            .partition(|step| step.depends_on.iter().any(|dep| dep == &dep_id));
        for step in &newly_removed {
            failed.push(step.id.clone());
        }
        removed.append(&mut newly_removed);
        *remaining = kept;
    }
    removed
}

pub(crate) fn resolve_sequence_task_id(req: &KernelSequenceRequest) -> Option<String> {
    if let Some(task_id) = req
        .reactive
        .task_id
        .as_ref()
        .filter(|task_id| !task_id.trim().is_empty())
    {
        return Some(task_id.clone());
    }

    req.steps.iter().find_map(|step| {
        step.policy_context
            .get("task_id")
            .and_then(Value::as_str)
            .filter(|task_id| !task_id.trim().is_empty())
            .map(ToOwned::to_owned)
    })
}

pub(crate) fn record_replan_cancellations(
    steps: &[KernelStepRequest],
    applied: &[context_scheduler_core::AppliedMutation],
    skipped: &mut Vec<String>,
    step_results: &mut Vec<KernelStepResponse>,
) {
    for mutation in applied.iter().filter(|mutation| mutation.kind == "cancel") {
        if let Some(step) = steps.iter().find(|step| step.id == mutation.step_id) {
            skipped.push(step.id.clone());
            step_results.push(KernelStepResponse {
                id: step.id.clone(),
                target: step.target.clone(),
                status: "skipped".to_string(),
                response: None,
                failure: Some(KernelFailure {
                    code: mutation.reason.clone(),
                    message: format!("step skipped by replanning: {}", mutation.reason),
                    target: Some(step.target.clone()),
                }),
            });
        }
    }
}
