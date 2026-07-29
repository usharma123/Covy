use super::*;

pub(crate) struct BuiltinReactivePlanner;

impl ReactivePlanner for BuiltinReactivePlanner {
    fn plan(&self, request: ReactivePlanRequest<'_>) -> Result<ReactivePlan, KernelError> {
        let snapshot = derive_agent_snapshot(request.cache_entries, request.task_id);
        Ok(ReactivePlan {
            event_count: snapshot.event_count,
            mutations: build_reactive_kernel_mutations(
                request.remaining,
                request.original_steps,
                &snapshot,
                request.completed_success,
                request.mode,
                request.append_focused_map,
                request.anchor_step_id,
            ),
        })
    }
}

fn merge_focus_into_map_step(
    step: &KernelStepRequest,
    snapshot: &suite_packet_core::AgentSnapshotPayload,
) -> Option<KernelStepRequest> {
    if step.target != "mapy.repo"
        || (snapshot.focus_paths.is_empty() && snapshot.focus_symbols.is_empty())
    {
        return None;
    }

    let mut request: mapy_core::RepoMapRequest =
        serde_json::from_value(step.reducer_input.clone()).ok()?;
    let mut changed = false;
    for path in &snapshot.focus_paths {
        if !request.focus_paths.iter().any(|existing| existing == path) {
            request.focus_paths.push(path.clone());
            changed = true;
        }
    }
    for symbol in &snapshot.focus_symbols {
        if !request
            .focus_symbols
            .iter()
            .any(|existing| existing == symbol)
        {
            request.focus_symbols.push(symbol.clone());
            changed = true;
        }
    }
    if !changed {
        return None;
    }

    let mut replaced = step.clone();
    replaced.reducer_input = serde_json::to_value(request).ok()?;
    Some(replaced)
}

pub(crate) fn build_reactive_kernel_mutations(
    remaining: &[KernelStepRequest],
    original_steps: &[KernelStepRequest],
    snapshot: &suite_packet_core::AgentSnapshotPayload,
    completed_success: &BTreeSet<String>,
    mode: ReactiveReplanMode,
    append_focused_map: bool,
    anchor_step_id: Option<&str>,
) -> Vec<KernelPlanMutation> {
    let mut mutations = Vec::new();

    for step in remaining {
        if snapshot
            .completed_steps
            .iter()
            .any(|completed| completed == &step.id)
        {
            mutations.push(KernelPlanMutation::Cancel {
                step_id: step.id.clone(),
                reason: "completed_step".to_string(),
            });
            continue;
        }
        if mode == ReactiveReplanMode::TaskAware
            && (!snapshot.changed_paths_since_checkpoint.is_empty()
                || !snapshot.changed_symbols_since_checkpoint.is_empty())
            && !step_affected_by_snapshot(step, snapshot)
        {
            mutations.push(KernelPlanMutation::Cancel {
                step_id: step.id.clone(),
                reason: "inputs_unchanged".to_string(),
            });
            continue;
        }
        if let Some(replaced) = merge_focus_into_map_step(step, snapshot) {
            mutations.push(KernelPlanMutation::Replace {
                step: replaced,
                reason: "focus_narrowed".to_string(),
            });
        }
    }

    let has_uncancelled_map = remaining.iter().any(|step| {
        step.target == "mapy.repo"
            && !mutations.iter().any(|mutation| {
                matches!(
                    mutation,
                    KernelPlanMutation::Cancel { step_id, .. } if step_id == &step.id
                )
            })
    });

    if append_focused_map
        && (!snapshot.focus_paths.is_empty() || !snapshot.focus_symbols.is_empty())
        && !has_uncancelled_map
    {
        if let Some(template) = original_steps
            .iter()
            .find(|step| step.target == "mapy.repo")
        {
            let appended_id = format!("{}__reactive_focus", template.id);
            if !remaining.iter().any(|step| step.id == appended_id)
                && !completed_success.contains(&appended_id)
                && !snapshot
                    .completed_steps
                    .iter()
                    .any(|step_id| step_id == &appended_id)
            {
                let mut appended = template.clone();
                appended.id = appended_id;
                appended.depends_on.retain(|dep| {
                    !completed_success.contains(dep)
                        && !snapshot.completed_steps.iter().any(|done| done == dep)
                });
                if let Some(anchor) = anchor_step_id {
                    if !appended.depends_on.iter().any(|dep| dep == anchor) {
                        appended.depends_on.push(anchor.to_string());
                    }
                }
                if let Some(replaced) = merge_focus_into_map_step(&appended, snapshot) {
                    mutations.push(KernelPlanMutation::Append {
                        step: replaced,
                        reason: "focus_followup".to_string(),
                    });
                }
            }
        }
    }

    mutations
}

fn step_affected_by_snapshot(
    step: &KernelStepRequest,
    snapshot: &suite_packet_core::AgentSnapshotPayload,
) -> bool {
    let changed_paths = &snapshot.changed_paths_since_checkpoint;
    let changed_symbols = &snapshot.changed_symbols_since_checkpoint;
    let focus_changed = snapshot.focus_paths != snapshot.checkpoint_focus_paths
        || snapshot.focus_symbols != snapshot.checkpoint_focus_symbols;

    if let Some(reactive) = step.reactive.as_ref() {
        if reactive.rerun_on_focus_change && focus_changed {
            return true;
        }
        if !reactive.path_globs.is_empty() {
            let matched = changed_paths.iter().any(|path| {
                reactive
                    .path_globs
                    .iter()
                    .any(|glob| match glob::Pattern::new(glob) {
                        Ok(pattern) => pattern.matches(path),
                        Err(_) => true,
                    })
            });
            if reactive.skip_if_inputs_unchanged {
                return matched;
            }
            if matched {
                return true;
            }
        }
    }

    match step.target.as_str() {
        "mapy.repo" => {
            focus_changed
                || changed_paths.iter().any(|path| {
                    !(path.ends_with(".info")
                        || path.ends_with(".lcov")
                        || path.ends_with(".xml")
                        || path.contains("coverage")
                        || path.contains("report"))
                })
                || !changed_symbols.is_empty()
        }
        "diffy.analyze" | "testy.impact" => changed_paths.iter().any(|path| {
            !(path.ends_with(".info")
                || path.ends_with(".lcov")
                || path.ends_with(".xml")
                || path.contains("coverage")
                || path.contains("report")
                || path.ends_with(".log"))
        }),
        "contextq.correlate" | "contextq.assemble" | "contextq.manage" => {
            !changed_paths.is_empty() || !changed_symbols.is_empty() || focus_changed
        }
        "stacky.slice" | "buildy.reduce" => changed_paths.iter().any(|path| {
            path.ends_with(".log")
                || path.ends_with(".txt")
                || path.contains("report")
                || path.contains("diagnostic")
        }),
        target if target.contains("cover") || target.contains("guard") => !changed_paths.is_empty(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appended_focus_map_prunes_satisfied_dependencies() {
        let template = KernelStepRequest {
            id: "map".to_string(),
            target: "mapy.repo".to_string(),
            depends_on: vec!["done".to_string(), "pending".to_string()],
            reducer_input: serde_json::to_value(mapy_core::RepoMapRequest::default()).unwrap(),
            ..KernelStepRequest::default()
        };
        let snapshot = suite_packet_core::AgentSnapshotPayload {
            focus_paths: vec!["src/main.rs".to_string()],
            completed_steps: vec!["done".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        };
        let completed_success = BTreeSet::from(["done".to_string()]);

        let mutations = build_reactive_kernel_mutations(
            &[],
            &[template],
            &snapshot,
            &completed_success,
            ReactiveReplanMode::TaskAware,
            true,
            Some("anchor"),
        );

        let KernelPlanMutation::Append { step, .. } = &mutations[0] else {
            panic!("expected appended map mutation");
        };
        assert_eq!(
            step.depends_on,
            vec!["pending".to_string(), "anchor".to_string()]
        );
    }

    #[test]
    fn unchanged_checkpoint_focus_does_not_trigger_focus_rerun() {
        let step = KernelStepRequest {
            id: "ctx".to_string(),
            target: "contextq.manage".to_string(),
            reactive: Some(KernelStepReactiveConfig {
                rerun_on_focus_change: true,
                ..KernelStepReactiveConfig::default()
            }),
            ..KernelStepRequest::default()
        };
        let snapshot = suite_packet_core::AgentSnapshotPayload {
            focus_paths: vec!["src/main.rs".to_string()],
            checkpoint_focus_paths: vec!["src/main.rs".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        };

        assert!(!step_affected_by_snapshot(&step, &snapshot));
    }

    #[test]
    fn invalid_glob_forces_rerun_instead_of_skipping() {
        let step = KernelStepRequest {
            id: "ctx".to_string(),
            target: "contextq.manage".to_string(),
            reactive: Some(KernelStepReactiveConfig {
                path_globs: vec!["[".to_string()],
                skip_if_inputs_unchanged: true,
                ..KernelStepReactiveConfig::default()
            }),
            ..KernelStepRequest::default()
        };
        let snapshot = suite_packet_core::AgentSnapshotPayload {
            changed_paths_since_checkpoint: vec!["src/main.rs".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        };

        assert!(step_affected_by_snapshot(&step, &snapshot));
    }

    #[test]
    fn canceled_map_step_does_not_block_focus_followup_append() {
        let remaining = vec![KernelStepRequest {
            id: "map".to_string(),
            target: "mapy.repo".to_string(),
            reducer_input: serde_json::to_value(mapy_core::RepoMapRequest::default()).unwrap(),
            ..KernelStepRequest::default()
        }];
        let snapshot = suite_packet_core::AgentSnapshotPayload {
            focus_paths: vec!["src/main.rs".to_string()],
            completed_steps: vec!["map".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        };

        let mutations = build_reactive_kernel_mutations(
            &remaining,
            &remaining,
            &snapshot,
            &BTreeSet::new(),
            ReactiveReplanMode::TaskAware,
            true,
            None,
        );

        assert!(mutations.iter().any(|mutation| {
            matches!(
                mutation,
                KernelPlanMutation::Append { step, .. } if step.id == "map__reactive_focus"
            )
        }));
    }
}
