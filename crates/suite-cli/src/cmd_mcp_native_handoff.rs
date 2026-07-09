use super::*;

pub(crate) fn handle_packet28_verify_handoff(
    root: &Path,
    args: Packet28VerifyHandoffArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.verify_handoff requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.verify_handoff requires artifact_id or context_version")
    })?;
    let path = task_version_json_path(root, task_id, &artifact_id);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "failed to read stored handoff context artifact '{}'",
            path.display()
        )
    })?;
    let payload: Value = serde_json::from_slice(&bytes)?;
    let mut missing = Vec::new();
    let brief = payload
        .get("brief")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !brief.contains("Task Objective") && !brief.contains("task_objective") {
        missing.push("objective".to_string());
    }
    let has_next_action = payload
        .get("next_action_summary")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        || payload
            .get("latest_intention")
            .is_some_and(|value| !value.is_null());
    if !has_next_action {
        missing.push("next_action".to_string());
    }
    let has_debt_signal = section_exists(&payload, "context_debt")
        || section_exists(&payload, "evidence_freshness")
        || payload
            .get("changed_paths_since_checkpoint")
            .and_then(Value::as_array)
            .is_some_and(|paths| !paths.is_empty())
        || payload
            .get("open_questions")
            .and_then(Value::as_array)
            .is_some_and(|questions| !questions.is_empty());
    if !has_debt_signal {
        missing.push("debt_signal".to_string());
    }
    let has_evidence_handle = payload
        .get("artifact_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        || payload
            .get("evidence_artifact_ids")
            .and_then(Value::as_array)
            .is_some_and(|ids| !ids.is_empty());
    if !has_evidence_handle {
        missing.push("evidence_handle".to_string());
    }
    let score = 100_u64.saturating_sub((missing.len() as u64).saturating_mul(25));
    let ready = missing.is_empty();
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ready": ready,
        "score": score,
        "missing": missing,
        "summary": if ready {
            "handoff_replay_ready"
        } else {
            "handoff_replay_incomplete"
        },
    }))
}

pub(crate) fn handle_packet28_prompt_pressure(
    root: &Path,
    args: Packet28PromptPressureArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.prompt_pressure requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.prompt_pressure requires artifact_id or context_version")
    })?;
    let path = task_version_json_path(root, task_id, &artifact_id);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "failed to read stored handoff context artifact '{}'",
            path.display()
        )
    })?;
    let payload: Value = serde_json::from_slice(&bytes)?;
    let budget_tokens = args.budget_tokens.unwrap_or(8_000).max(1);
    let next_prompt = args.next_prompt.unwrap_or_default();
    let context_tokens = estimate_tokens_for_value(&payload);
    let next_prompt_tokens = estimate_tokens_for_text(&next_prompt);
    let pointer_context = format!("Read `packet28://task/{task_id}/brief` for full context.");
    let pointer_context_tokens = estimate_tokens_for_text(&pointer_context);
    let pointer_total_tokens = pointer_context_tokens.saturating_add(next_prompt_tokens);
    let pointer_savings_tokens = context_tokens.saturating_sub(pointer_context_tokens);
    let pointer_savings_pct = if context_tokens == 0 {
        0.0
    } else {
        ((pointer_savings_tokens as f64 / context_tokens as f64) * 1000.0).round() / 10.0
    };
    let total_tokens = context_tokens.saturating_add(next_prompt_tokens);
    let remaining_tokens = budget_tokens as i64 - total_tokens as i64;
    let mut removable_sections = Vec::new();
    if let Some(sections) = payload.get("sections").and_then(Value::as_array) {
        for section in sections {
            let id = section
                .get("id")
                .or_else(|| section.get("title"))
                .and_then(Value::as_str)
                .unwrap_or("section");
            removable_sections.push((id.to_string(), estimate_tokens_for_value(section)));
        }
    }
    removable_sections.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let largest_removable_sections: Vec<Value> = removable_sections
        .into_iter()
        .take(3)
        .map(|(id, tokens)| {
            json!({
                "id": id,
                "tokens": tokens,
            })
        })
        .collect();
    let pressure = if total_tokens > budget_tokens {
        "over_budget"
    } else if total_tokens.saturating_mul(100) >= budget_tokens.saturating_mul(85) {
        "tight"
    } else {
        "ok"
    };
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "budget_tokens": budget_tokens,
        "context_tokens": context_tokens,
        "next_prompt_tokens": next_prompt_tokens,
        "total_tokens": total_tokens,
        "pointer_context_tokens": pointer_context_tokens,
        "pointer_total_tokens": pointer_total_tokens,
        "pointer_savings_tokens": pointer_savings_tokens,
        "pointer_savings_pct": pointer_savings_pct,
        "remaining_tokens": remaining_tokens,
        "pressure": pressure,
        "over_budget": total_tokens > budget_tokens,
        "largest_removable_sections": largest_removable_sections,
        "summary": format!("prompt_pressure={pressure} total_tokens={total_tokens} remaining_tokens={remaining_tokens} pointer_savings_tokens={pointer_savings_tokens}"),
    }))
}

pub(crate) fn handle_packet28_handoff_diff(
    root: &Path,
    args: Packet28HandoffDiffArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_diff requires task_id"));
    }
    let left_artifact_id = args
        .left_artifact_id
        .or(args.left_context_version)
        .ok_or_else(|| {
            anyhow!("packet28.handoff_diff requires left_artifact_id or left_context_version")
        })?;
    let right_artifact_id = args
        .right_artifact_id
        .or(args.right_context_version)
        .ok_or_else(|| {
            anyhow!("packet28.handoff_diff requires right_artifact_id or right_context_version")
        })?;
    let left = read_handoff_payload(root, task_id, &left_artifact_id, "handoff diff")?;
    let right = read_handoff_payload(root, task_id, &right_artifact_id, "handoff diff")?;
    let mut deltas = Vec::new();
    push_handoff_delta(
        &mut deltas,
        "next_action",
        handoff_next_action(&left),
        handoff_next_action(&right),
    );
    push_handoff_delta(
        &mut deltas,
        "objective",
        handoff_objective(&left),
        handoff_objective(&right),
    );
    push_handoff_delta(
        &mut deltas,
        "evidence_handles",
        handoff_evidence_handle_summary(&left),
        handoff_evidence_handle_summary(&right),
    );
    push_handoff_delta(
        &mut deltas,
        "debt_signal",
        handoff_debt_signal(&left).to_string(),
        handoff_debt_signal(&right).to_string(),
    );
    let top_delta = deltas
        .first()
        .and_then(|delta| delta.get("field"))
        .and_then(Value::as_str)
        .unwrap_or("none");
    Ok(json!({
        "task_id": task_id,
        "left_artifact_id": left_artifact_id,
        "right_artifact_id": right_artifact_id,
        "delta_count": deltas.len(),
        "top_delta": top_delta,
        "deltas": deltas,
        "summary": format!("handoff_diff delta_count={} top_delta={top_delta}", deltas.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_compress(
    root: &Path,
    args: Packet28HandoffCompressionArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_compress requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_compress requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff compression")?;
    let budget_tokens = args.budget_tokens.unwrap_or(8_000).max(1);
    let next_prompt = args.next_prompt.unwrap_or_default();
    let context_tokens = estimate_tokens_for_value(&payload);
    let total_tokens = context_tokens.saturating_add(estimate_tokens_for_text(&next_prompt));
    let mut needed_savings = total_tokens.saturating_sub(budget_tokens);
    let mut candidates = Vec::new();
    if let Some(sections) = payload.get("sections").and_then(Value::as_array) {
        for section in sections {
            let id = section_identifier(section);
            let tokens = estimate_tokens_for_value(section);
            let protected = is_replay_critical_section(section);
            if protected {
                continue;
            }
            candidates.push((id, tokens));
        }
    }
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut selected_tokens = 0_u64;
    let mut recommendations = Vec::new();
    for (id, tokens) in candidates.into_iter().take(4) {
        if needed_savings == 0 {
            break;
        }
        selected_tokens = selected_tokens.saturating_add(tokens);
        needed_savings = needed_savings.saturating_sub(tokens);
        recommendations.push(json!({
            "action": "drop_section",
            "id": id,
            "tokens": tokens,
            "reason": "non_replay_critical_section",
        }));
    }
    let projected_total_tokens = total_tokens.saturating_sub(selected_tokens);
    let projected_over_budget = projected_total_tokens > budget_tokens;
    let recommendation_count = recommendations.len();
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "budget_tokens": budget_tokens,
        "total_tokens": total_tokens,
        "needed_savings_tokens": total_tokens.saturating_sub(budget_tokens),
        "projected_total_tokens": projected_total_tokens,
        "projected_over_budget": projected_over_budget,
        "recommendations": recommendations,
        "summary": format!(
            "handoff_compress recommendations={} projected_over_budget={}",
            recommendation_count,
            projected_over_budget
        ),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_dependencies(
    root: &Path,
    args: Packet28HandoffDependencyLintArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!(
            "packet28.handoff_lint_dependencies requires task_id"
        ));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_dependencies requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff dependency lint")?;
    let available_artifacts = available_handoff_artifacts(&payload);
    let referenced_artifacts = referenced_handoff_artifacts(&payload);
    let mut issues = Vec::new();
    for reference in referenced_artifacts {
        if !available_artifacts
            .iter()
            .any(|available| available == &reference)
        {
            issues.push(json!({
                "kind": "missing_artifact",
                "reference": reference,
                "reason": "referenced artifact handle is absent from artifact_id and evidence_artifact_ids",
            }));
        }
    }
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": issues.is_empty(),
        "issue_count": issues.len(),
        "issues": issues,
        "summary": format!("handoff_dependency_lint issue_count={}", issues.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_paths(
    root: &Path,
    args: Packet28HandoffPathLintArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_lint_paths requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_paths requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff path lint")?;
    let changed_paths = available_handoff_paths(&payload);
    let referenced_paths = referenced_handoff_paths(&payload);
    let mut issues = Vec::new();
    for reference in referenced_paths {
        let exists_on_disk = root.join(&reference).exists();
        let listed_as_changed = changed_paths.iter().any(|path| path == &reference);
        if !exists_on_disk && !listed_as_changed {
            issues.push(json!({
                "kind": "missing_path",
                "reference": reference,
                "reason": "referenced path is absent on disk and not listed in changed_paths_since_checkpoint",
            }));
        }
    }
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": issues.is_empty(),
        "issue_count": issues.len(),
        "issues": issues,
        "summary": format!("handoff_path_lint issue_count={}", issues.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_tests(
    root: &Path,
    args: Packet28HandoffTestLintArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_lint_tests requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_tests requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff test lint")?;
    let text_blocks = handoff_text_blocks(&payload);
    let mut mentioned_tests = Vec::new();
    let mut command_backed_tests = Vec::new();
    for text in &text_blocks {
        collect_test_mentions(text, &mut mentioned_tests);
        collect_command_backed_tests(text, &mut command_backed_tests);
    }
    let mut issues = Vec::new();
    for test_name in mentioned_tests {
        if !command_backed_tests
            .iter()
            .any(|command_test| command_test == &test_name)
        {
            issues.push(json!({
                "kind": "missing_test_command",
                "reference": test_name,
                "reason": "test-like name is mentioned without a runnable test command in the same handoff",
            }));
        }
    }
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": issues.is_empty(),
        "issue_count": issues.len(),
        "issues": issues,
        "summary": format!("handoff_test_lint issue_count={}", issues.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_stale_commands(
    root: &Path,
    args: Packet28HandoffStaleCommandLintArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!(
            "packet28.handoff_lint_stale_commands requires task_id"
        ));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_stale_commands requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff stale-command lint")?;
    let command_refs = referenced_handoff_commands(&payload);
    let changed_paths = available_handoff_paths(&payload);
    let events = load_task_events(root, task_id).unwrap_or_default();
    let latest_edit_at = latest_relevant_edit_at(&events, &changed_paths);
    let mut issues = Vec::new();
    if let Some(latest_edit_at) = latest_edit_at {
        for command in command_refs {
            if let Some(command_at) = latest_command_event_at(&events, &command) {
                if command_at < latest_edit_at {
                    issues.push(json!({
                        "kind": "stale_command",
                        "reference": command,
                        "command_at_unix": command_at,
                        "latest_edit_at_unix": latest_edit_at,
                        "reason": "referenced command ran before the latest relevant edit",
                    }));
                }
            }
        }
    }
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": issues.is_empty(),
        "issue_count": issues.len(),
        "issues": issues,
        "summary": format!("handoff_stale_command_lint issue_count={}", issues.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_environment(
    root: &Path,
    args: Packet28HandoffEnvironmentLintArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!(
            "packet28.handoff_lint_environment requires task_id"
        ));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_environment requires artifact_id or context_version")
    })?;
    let payload = read_handoff_payload(root, task_id, &artifact_id, "handoff environment lint")?;
    let command_refs = referenced_handoff_commands(&payload);
    let mut issues = Vec::new();
    for command in command_refs {
        if let Some(executable) = command_executable(&command) {
            if !executable_exists(&executable) {
                issues.push(json!({
                    "kind": "missing_tool",
                    "reference": executable,
                    "command": command,
                    "reason": "command executable was not found on PATH",
                }));
            }
        }
        for env_var in command_env_vars(&command) {
            if std::env::var_os(&env_var).is_none() {
                issues.push(json!({
                    "kind": "missing_env",
                    "reference": env_var,
                    "command": command,
                    "reason": "command references an environment variable that is not set",
                }));
            }
        }
    }
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": issues.is_empty(),
        "issue_count": issues.len(),
        "issues": issues,
        "summary": format!("handoff_environment_lint issue_count={}", issues.len()),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_all(
    root: &Path,
    args: Packet28HandoffLintAllArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_lint_all requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_lint_all requires artifact_id or context_version")
    })?;
    let checks = vec![
        handoff_lint_check(
            "replay",
            handle_packet28_verify_handoff(
                root,
                Packet28VerifyHandoffArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
        handoff_lint_check(
            "dependencies",
            handle_packet28_handoff_lint_dependencies(
                root,
                Packet28HandoffDependencyLintArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
        handoff_lint_check(
            "paths",
            handle_packet28_handoff_lint_paths(
                root,
                Packet28HandoffPathLintArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
        handoff_lint_check(
            "tests",
            handle_packet28_handoff_lint_tests(
                root,
                Packet28HandoffTestLintArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
        handoff_lint_check(
            "stale_commands",
            handle_packet28_handoff_lint_stale_commands(
                root,
                Packet28HandoffStaleCommandLintArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
        handoff_lint_check(
            "environment",
            handle_packet28_handoff_lint_environment(
                root,
                Packet28HandoffEnvironmentLintArgs {
                    task_id: task_id.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    context_version: None,
                },
            )?,
        ),
    ];
    let failing_categories: Vec<String> = checks
        .iter()
        .filter(|check| !check.get("ok").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|check| {
            check
                .get("category")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let issue_count: u64 = checks
        .iter()
        .map(|check| {
            check
                .get("issue_count")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        })
        .sum();
    let ok = failing_categories.is_empty();
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "ok": ok,
        "status": if ok { "ready" } else { "blocked" },
        "issue_count": issue_count,
        "failing_categories": failing_categories,
        "checks": checks,
        "summary": format!("handoff_lint_all status={} issue_count={issue_count}", if ok { "ready" } else { "blocked" }),
    }))
}

pub(crate) fn handle_packet28_handoff_fix_plan(
    root: &Path,
    args: Packet28HandoffFixPlanArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_fix_plan requires task_id"));
    }
    let artifact_id = args.artifact_id.or(args.context_version).ok_or_else(|| {
        anyhow!("packet28.handoff_fix_plan requires artifact_id or context_version")
    })?;
    let lint = handle_packet28_handoff_lint_all(
        root,
        Packet28HandoffLintAllArgs {
            task_id: task_id.to_string(),
            artifact_id: Some(artifact_id.clone()),
            context_version: None,
        },
    )?;
    let actions = handoff_fix_actions_from_lint(&lint);
    let action_count = actions.len();
    Ok(json!({
        "task_id": task_id,
        "artifact_id": artifact_id,
        "status": if action_count == 0 { "ready" } else { "needs_fix" },
        "action_count": action_count,
        "actions": actions,
        "summary": format!("handoff_fix_plan action_count={action_count}"),
    }))
}

pub(crate) fn handle_packet28_handoff_repair_verify(
    root: &Path,
    args: Packet28HandoffRepairVerifyArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_repair_verify requires task_id"));
    }
    let before_artifact_id = args
        .before_artifact_id
        .or(args.before_context_version)
        .ok_or_else(|| {
            anyhow!("packet28.handoff_repair_verify requires before_artifact_id or before_context_version")
        })?;
    let after_artifact_id = args
        .after_artifact_id
        .or(args.after_context_version)
        .ok_or_else(|| {
            anyhow!("packet28.handoff_repair_verify requires after_artifact_id or after_context_version")
        })?;
    let before = handle_packet28_handoff_lint_all(
        root,
        Packet28HandoffLintAllArgs {
            task_id: task_id.to_string(),
            artifact_id: Some(before_artifact_id.clone()),
            context_version: None,
        },
    )?;
    let after = handle_packet28_handoff_lint_all(
        root,
        Packet28HandoffLintAllArgs {
            task_id: task_id.to_string(),
            artifact_id: Some(after_artifact_id.clone()),
            context_version: None,
        },
    )?;
    let before_categories = lint_failing_categories(&before);
    let after_categories = lint_failing_categories(&after);
    let cleared_categories: Vec<String> = before_categories
        .iter()
        .filter(|category| !after_categories.iter().any(|after| after == *category))
        .cloned()
        .collect();
    let regressed_categories: Vec<String> = after_categories
        .iter()
        .filter(|category| !before_categories.iter().any(|before| before == *category))
        .cloned()
        .collect();
    let verified = after_categories.is_empty();
    let cleared_count = cleared_categories.len();
    Ok(json!({
        "task_id": task_id,
        "before_artifact_id": before_artifact_id,
        "after_artifact_id": after_artifact_id,
        "verified": verified,
        "cleared_categories": cleared_categories,
        "remaining_categories": after_categories,
        "regressed_categories": regressed_categories,
        "summary": format!("handoff_repair_verify verified={verified} cleared={cleared_count}"),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_trends(
    root: &Path,
    args: Packet28HandoffLintTrendArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.handoff_lint_trends requires task_id"));
    }
    let max_artifacts = args.max_artifacts.unwrap_or(8).clamp(1, 24);
    let artifact_ids = if args.artifact_ids.is_empty() {
        discover_handoff_artifact_ids(root, task_id, max_artifacts)?
    } else {
        args.artifact_ids
            .into_iter()
            .filter(|id| !id.trim().is_empty())
            .take(max_artifacts)
            .collect()
    };
    let mut records = Vec::new();
    let mut category_counts = std::collections::BTreeMap::<String, u64>::new();
    let mut latest_categories = Vec::<String>::new();
    for artifact_id in &artifact_ids {
        let lint = handle_packet28_handoff_lint_all(
            root,
            Packet28HandoffLintAllArgs {
                task_id: task_id.to_string(),
                artifact_id: Some(artifact_id.clone()),
                context_version: None,
            },
        )?;
        let categories = lint_failing_categories(&lint);
        latest_categories = categories.clone();
        for category in &categories {
            *category_counts.entry(category.clone()).or_default() += 1;
        }
        records.push(json!({
            "artifact_id": artifact_id,
            "failing_categories": categories,
        }));
    }
    let recurring_categories: Vec<Value> = category_counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(category, count)| {
            json!({
                "category": category,
                "count": count,
            })
        })
        .collect();
    let cleared_categories: Vec<String> = category_counts
        .keys()
        .filter(|category| !latest_categories.iter().any(|latest| latest == *category))
        .cloned()
        .collect();
    Ok(json!({
        "task_id": task_id,
        "artifact_count": records.len(),
        "latest_artifact_id": artifact_ids.last().cloned().unwrap_or_default(),
        "latest_blocking_categories": latest_categories,
        "recurring_categories": recurring_categories,
        "cleared_categories": cleared_categories,
        "records": records,
        "summary": format!(
            "handoff_lint_trends artifacts={} recurring={} cleared={}",
            artifact_ids.len(),
            category_counts.values().filter(|count| **count > 1).count(),
            category_counts.keys().filter(|category| {
                !lint_latest_category_contains(&latest_categories, category)
            }).count()
        ),
    }))
}

pub(crate) fn handle_packet28_handoff_lint_regressions(
    root: &Path,
    args: Packet28HandoffLintRegressionArgs,
) -> Result<Value> {
    let task_id = args.task_id.trim();
    if task_id.is_empty() {
        return Err(anyhow!(
            "packet28.handoff_lint_regressions requires task_id"
        ));
    }
    let max_artifacts = args.max_artifacts.unwrap_or(8).clamp(1, 24);
    let artifact_ids = if args.artifact_ids.is_empty() {
        discover_handoff_artifact_ids(root, task_id, max_artifacts)?
    } else {
        args.artifact_ids
            .into_iter()
            .filter(|id| !id.trim().is_empty())
            .take(max_artifacts)
            .collect()
    };
    let mut snapshots = Vec::<(String, Vec<String>)>::new();
    for artifact_id in &artifact_ids {
        let lint = handle_packet28_handoff_lint_all(
            root,
            Packet28HandoffLintAllArgs {
                task_id: task_id.to_string(),
                artifact_id: Some(artifact_id.clone()),
                context_version: None,
            },
        )?;
        snapshots.push((artifact_id.clone(), lint_failing_categories(&lint)));
    }
    let latest_artifact_id = snapshots
        .last()
        .map(|(artifact_id, _)| artifact_id.clone())
        .unwrap_or_default();
    let latest_categories = snapshots
        .last()
        .map(|(_, categories)| categories.clone())
        .unwrap_or_default();
    let mut regressions = Vec::new();
    for category in &latest_categories {
        let mut seen_before = false;
        let mut cleared_before_latest = false;
        for (_, categories) in snapshots.iter().take(snapshots.len().saturating_sub(1)) {
            if categories.iter().any(|candidate| candidate == category) {
                seen_before = true;
            } else if seen_before {
                cleared_before_latest = true;
            }
        }
        if seen_before && cleared_before_latest {
            regressions.push(json!({
                "category": category,
                "latest_artifact_id": latest_artifact_id,
                "reason": "category was previously cleared and reappeared in the latest artifact",
            }));
        }
    }
    let regression_count = regressions.len();
    Ok(json!({
        "task_id": task_id,
        "artifact_count": snapshots.len(),
        "ok": regression_count == 0,
        "regression_count": regression_count,
        "regressions": regressions,
        "summary": format!("handoff_lint_regressions count={regression_count}"),
    }))
}

fn read_handoff_payload(
    root: &Path,
    task_id: &str,
    artifact_id: &str,
    label: &str,
) -> Result<Value> {
    let path = task_version_json_path(root, task_id, artifact_id);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "failed to read stored {label} context artifact '{}'",
            path.display()
        )
    })?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn available_handoff_artifacts(payload: &Value) -> Vec<String> {
    let mut artifacts = Vec::new();
    if let Some(artifact_id) = payload.get("artifact_id").and_then(Value::as_str) {
        append_unique(&mut artifacts, artifact_id.to_string());
    }
    if let Some(ids) = payload
        .get("evidence_artifact_ids")
        .and_then(Value::as_array)
    {
        for id in ids.iter().filter_map(Value::as_str) {
            append_unique(&mut artifacts, id.to_string());
        }
    }
    artifacts
}

fn available_handoff_paths(payload: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(changed_paths) = payload
        .get("changed_paths_since_checkpoint")
        .and_then(Value::as_array)
    {
        for path in changed_paths.iter().filter_map(Value::as_str) {
            append_unique(&mut paths, path.to_string());
        }
    }
    paths
}

fn handoff_text_blocks(payload: &Value) -> Vec<String> {
    let mut blocks = Vec::new();
    if let Some(brief) = payload.get("brief").and_then(Value::as_str) {
        blocks.push(brief.to_string());
    }
    if let Some(next_action) = payload.get("next_action_summary").and_then(Value::as_str) {
        blocks.push(next_action.to_string());
    }
    if let Some(sections) = payload.get("sections").and_then(Value::as_array) {
        for section in sections {
            if let Some(body) = section.get("body").and_then(Value::as_str) {
                blocks.push(body.to_string());
            }
        }
    }
    blocks
}

fn referenced_handoff_artifacts(payload: &Value) -> Vec<String> {
    let mut references = Vec::new();
    collect_artifact_references(
        payload
            .get("brief")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        &mut references,
    );
    if let Some(sections) = payload.get("sections").and_then(Value::as_array) {
        for section in sections {
            collect_artifact_references(
                section
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                &mut references,
            );
        }
    }
    references
}

fn referenced_handoff_paths(payload: &Value) -> Vec<String> {
    let mut references = Vec::new();
    collect_path_references(
        payload
            .get("brief")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        &mut references,
    );
    if let Some(sections) = payload.get("sections").and_then(Value::as_array) {
        for section in sections {
            collect_path_references(
                section
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                &mut references,
            );
        }
    }
    references
}

fn referenced_handoff_commands(payload: &Value) -> Vec<String> {
    let mut commands = Vec::new();
    for block in handoff_text_blocks(payload) {
        for line in block.lines() {
            if let Some(command) = extract_test_command_reference(line) {
                append_unique(&mut commands, command);
            }
        }
    }
    commands
}

fn collect_artifact_references(text: &str, references: &mut Vec<String>) {
    for token in text.split_whitespace() {
        let token = token.trim_matches(|ch: char| {
            matches!(
                ch,
                '`' | '\'' | '"' | ',' | '.' | ';' | ':' | ')' | '(' | '[' | ']'
            )
        });
        if token.starts_with("artifact-") || token.starts_with("raw-") {
            append_unique(references, token.to_string());
        }
    }
}

fn collect_test_mentions(text: &str, tests: &mut Vec<String>) {
    for token in text.split_whitespace() {
        let token = clean_reference_token(token);
        if is_test_name_reference(token) {
            append_unique(tests, token.to_string());
        }
    }
}

fn collect_command_backed_tests(text: &str, tests: &mut Vec<String>) {
    for line in text.lines() {
        if contains_test_command(line) {
            collect_test_mentions(line, tests);
        }
    }
}

fn contains_test_command(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    line.contains("cargo test")
        || line.contains("cargo nextest")
        || line.contains("npm test")
        || line.contains("pnpm test")
        || line.contains("yarn test")
        || line.contains("bun test")
        || line.contains("pytest")
        || line.contains("go test")
        || line.contains("mvn test")
        || line.contains("gradle test")
}

fn extract_test_command_reference(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let markers = [
        "cargo test",
        "cargo nextest",
        "npm test",
        "pnpm test",
        "yarn test",
        "bun test",
        "pytest",
        "go test",
        "mvn test",
        "gradle test",
    ];
    let start = markers
        .iter()
        .filter_map(|marker| lower.find(marker))
        .min()?;
    let command = clean_command_reference(&line[start..]);
    (!command.is_empty()).then_some(command)
}

fn clean_command_reference(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch: char| matches!(ch, '`' | '\'' | '"' | ',' | '.' | ';' | ')' | ']'))
        .to_string()
}

fn command_executable(command: &str) -> Option<String> {
    command
        .split_whitespace()
        .find(|part| !part.contains('='))
        .map(clean_command_token)
        .filter(|part| !part.is_empty())
}

fn clean_command_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| matches!(ch, '`' | '\'' | '"' | ',' | '.' | ';' | ')' | ']'))
        .to_string()
}

fn executable_exists(executable: &str) -> bool {
    if executable.contains('/') {
        return Path::new(executable).exists();
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|path| path.join(executable).exists())
    })
}

fn command_env_vars(command: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let chars: Vec<char> = command.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '$' {
            index += 1;
            continue;
        }
        let start = index + 1;
        if start >= chars.len() || !is_env_var_start(chars[start]) {
            index += 1;
            continue;
        }
        let mut end = start + 1;
        while end < chars.len() && is_env_var_char(chars[end]) {
            end += 1;
        }
        append_unique(&mut vars, chars[start..end].iter().collect::<String>());
        index = end;
    }
    vars
}

fn handoff_lint_check(category: &str, payload: Value) -> Value {
    let ok = payload
        .get("ok")
        .and_then(Value::as_bool)
        .or_else(|| payload.get("ready").and_then(Value::as_bool))
        .unwrap_or(false);
    let issue_count = payload
        .get("issue_count")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            payload
                .get("missing")
                .and_then(Value::as_array)
                .map(|missing| missing.len() as u64)
                .unwrap_or_default()
        });
    let references = payload
        .get("issues")
        .and_then(Value::as_array)
        .map(|issues| {
            issues
                .iter()
                .filter_map(|issue| issue.get("reference").and_then(Value::as_str))
                .take(3)
                .map(|reference| json!(reference))
                .collect::<Vec<Value>>()
        })
        .or_else(|| {
            payload
                .get("missing")
                .and_then(Value::as_array)
                .map(|missing| {
                    missing
                        .iter()
                        .filter_map(Value::as_str)
                        .take(3)
                        .map(|reference| json!(reference))
                        .collect::<Vec<Value>>()
                })
        })
        .unwrap_or_default();
    json!({
        "category": category,
        "ok": ok,
        "issue_count": issue_count,
        "references": references,
    })
}

fn handoff_fix_actions_from_lint(lint: &Value) -> Vec<Value> {
    let mut actions = Vec::new();
    let Some(checks) = lint.get("checks").and_then(Value::as_array) else {
        return actions;
    };
    for check in checks {
        if check.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let category = check
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let references = check
            .get("references")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let first_reference = references
            .iter()
            .filter_map(Value::as_str)
            .next()
            .unwrap_or_default();
        let action = match category {
            "replay" => json!({
                "kind": "repair_handoff_packet",
                "reference": first_reference,
                "next": "regenerate handoff with objective, next action, debt signal, and evidence handle",
                "command": "Packet28 prepare_handoff",
            }),
            "dependencies" => json!({
                "kind": "attach_missing_artifact",
                "reference": first_reference,
                "next": "attach referenced artifact handle or remove the stale reference",
                "command": format!("packet28.fetch_tool_result handle={first_reference}"),
            }),
            "paths" => json!({
                "kind": "read_or_correct_path",
                "reference": first_reference,
                "next": "read the referenced path or correct the handoff path before replay",
                "command": format!("rg --files | rg '{}'", path_search_fragment(first_reference)),
            }),
            "tests" => json!({
                "kind": "add_test_command",
                "reference": first_reference,
                "next": "add or run a concrete command for the mentioned test",
                "command": format!("cargo test {first_reference}"),
            }),
            "stale_commands" => json!({
                "kind": "rerun_stale_command",
                "reference": first_reference,
                "next": "rerun the command after the latest relevant edit",
                "command": first_reference,
            }),
            "environment" => json!({
                "kind": "setup_environment",
                "reference": first_reference,
                "next": "set the missing variable or remove the command dependency",
                "command": format!("export {first_reference}=<value>"),
            }),
            _ => continue,
        };
        actions.push(action);
    }
    actions.truncate(6);
    actions
}

fn path_search_fragment(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).replace('\'', "")
}

fn lint_failing_categories(lint: &Value) -> Vec<String> {
    lint.get("failing_categories")
        .and_then(Value::as_array)
        .map(|categories| {
            categories
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn discover_handoff_artifact_ids(
    root: &Path,
    task_id: &str,
    max_artifacts: usize,
) -> Result<Vec<String>> {
    let probe = task_version_json_path(root, task_id, "__packet28_probe__");
    let Some(dir) = probe.parent() else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    if !dir.exists() {
        return Ok(ids);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
            ids.push(stem.to_string());
        }
    }
    ids.sort();
    if ids.len() > max_artifacts {
        ids = ids.split_off(ids.len() - max_artifacts);
    }
    Ok(ids)
}

fn lint_latest_category_contains(latest_categories: &[String], category: &str) -> bool {
    latest_categories.iter().any(|latest| latest == category)
}

fn is_env_var_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_uppercase()
}

fn is_env_var_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit()
}

fn is_test_name_reference(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    token.len() >= 6
        && (lower.starts_with("test_")
            || lower.ends_with("_test")
            || lower.ends_with("_tests")
            || lower.contains("::tests::")
            || lower.contains("test::"))
}

fn clean_reference_token(token: &str) -> &str {
    token.trim_matches(|ch: char| {
        matches!(
            ch,
            '`' | '\'' | '"' | ',' | '.' | ';' | ':' | ')' | '(' | '[' | ']'
        )
    })
}

fn collect_path_references(text: &str, references: &mut Vec<String>) {
    for token in text.split_whitespace() {
        let token = clean_reference_token(token);
        if is_repo_relative_path_reference(token) {
            append_unique(references, token.to_string());
        }
    }
}

fn is_repo_relative_path_reference(token: &str) -> bool {
    !token.starts_with('/')
        && !token.contains("://")
        && token.contains('/')
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-' | '.'))
        && token
            .rsplit('/')
            .next()
            .is_some_and(|name| name.contains('.'))
}

fn latest_relevant_edit_at(
    events: &[packet28_daemon_core::DaemonEventFrame],
    changed_paths: &[String],
) -> Option<u64> {
    events
        .iter()
        .filter(|frame| is_edit_event(frame, changed_paths))
        .map(|frame| frame.event.occurred_at_unix)
        .max()
}

fn latest_command_event_at(
    events: &[packet28_daemon_core::DaemonEventFrame],
    command_ref: &str,
) -> Option<u64> {
    events
        .iter()
        .filter(|frame| {
            frame
                .event
                .data
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| command == command_ref || command.contains(command_ref))
        })
        .map(|frame| frame.event.occurred_at_unix)
        .max()
}

fn is_edit_event(frame: &packet28_daemon_core::DaemonEventFrame, changed_paths: &[String]) -> bool {
    let kind = frame.event.kind.to_ascii_lowercase();
    if !kind.contains("edit") && !kind.contains("write") {
        return false;
    }
    if changed_paths.is_empty() {
        return true;
    }
    frame_event_paths(frame)
        .iter()
        .any(|path| changed_paths.iter().any(|changed| changed == path))
}

fn frame_event_paths(frame: &packet28_daemon_core::DaemonEventFrame) -> Vec<String> {
    frame
        .event
        .data
        .get("paths")
        .or_else(|| frame.event.data.get("changed_paths"))
        .and_then(Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn section_identifier(section: &Value) -> String {
    section
        .get("id")
        .or_else(|| section.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("section")
        .to_string()
}

fn is_replay_critical_section(section: &Value) -> bool {
    let id = section_identifier(section).to_ascii_lowercase();
    let title = section
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    id.contains("objective")
        || id.contains("next_action")
        || id.contains("context_debt")
        || id.contains("evidence_freshness")
        || title.contains("objective")
        || title.contains("next action")
        || title.contains("context debt")
        || title.contains("evidence freshness")
}

fn push_handoff_delta(deltas: &mut Vec<Value>, field: &str, left: String, right: String) {
    if left != right {
        deltas.push(json!({
            "field": field,
            "left": compact_handoff_text(&left),
            "right": compact_handoff_text(&right),
        }));
    }
}

fn compact_handoff_text(value: &str) -> String {
    let value = value.trim();
    let mut compact = String::new();
    for ch in value.chars().take(120) {
        compact.push(ch);
    }
    if value.chars().count() > 120 {
        compact.push_str("...");
    }
    compact
}

fn handoff_objective(payload: &Value) -> String {
    payload
        .get("brief")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .lines()
        .find(|line| !line.trim().is_empty() && !line.contains("Task Objective"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn handoff_next_action(payload: &Value) -> String {
    payload
        .get("next_action_summary")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            payload
                .get("latest_intention")
                .filter(|value| !value.is_null())
                .map(Value::to_string)
        })
        .unwrap_or_default()
}

fn handoff_evidence_handle_summary(payload: &Value) -> String {
    let artifact_id = payload
        .get("artifact_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let evidence_count = payload
        .get("evidence_artifact_ids")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    format!("artifact_id={artifact_id} evidence_count={evidence_count}")
}

fn handoff_debt_signal(payload: &Value) -> bool {
    section_exists(payload, "context_debt")
        || section_exists(payload, "evidence_freshness")
        || payload
            .get("changed_paths_since_checkpoint")
            .and_then(Value::as_array)
            .is_some_and(|paths| !paths.is_empty())
        || payload
            .get("open_questions")
            .and_then(Value::as_array)
            .is_some_and(|questions| !questions.is_empty())
}

fn section_exists(payload: &Value, id: &str) -> bool {
    payload
        .get("sections")
        .and_then(Value::as_array)
        .is_some_and(|sections| {
            sections
                .iter()
                .any(|section| section.get("id").and_then(Value::as_str) == Some(id))
        })
}

pub(crate) fn handle_packet28_prepare_handoff(
    root: &Path,
    args: Packet28PrepareHandoffArgs,
) -> Result<Value> {
    let response = crate::broker_client::prepare_handoff(
        root,
        BrokerPrepareHandoffRequest {
            task_id: args.task_id,
            query: args.query,
            response_mode: args.response_mode,
            include_debug_memory: false,
        },
    )?;
    Ok(serde_json::to_value(response)?)
}
