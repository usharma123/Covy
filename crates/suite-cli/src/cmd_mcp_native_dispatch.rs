use super::*;

pub(super) fn handle_native_tool_call(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    name: &str,
    arguments: &Value,
) -> Result<Option<Value>> {
    let payload = match name {
        "packet28.search" => {
            let mut request: Packet28SearchArgs = serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                Some(request.query.as_str()),
                "packet28.search",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_search(root, session, request)?
        }
        "packet28.search_fast" => {
            let request: Packet28SearchFastArgs = serde_json::from_value(arguments.clone())?;
            handle_packet28_search_fast(root, session, request)?
        }
        "packet28.read_regions" => {
            let mut request: Packet28ReadRegionsArgs = serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                Some(request.path.as_str()),
                "packet28.read_regions",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_read_regions(root, session, request)?
        }
        "packet28.glob" => {
            let mut request: Packet28GlobArgs = serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                Some(request.pattern.as_str()),
                "packet28.glob",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_glob(root, session, request)?
        }
        "packet28.fetch_tool_result" => {
            let mut request: Packet28FetchToolResultArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                None,
                "packet28.fetch_tool_result",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_fetch_tool_result(root, request)?
        }
        "packet28.fetch_raw_output" => {
            let mut request: Packet28FetchRawOutputArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                Some(request.handle.as_str()),
                "packet28.fetch_raw_output",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_fetch_raw_output(root, request)?
        }
        "packet28.fetch_context" => {
            let mut request: Packet28FetchContextArgs = serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                None,
                "packet28.fetch_context",
            )?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_fetch_context(root, request)?
        }
        "packet28.verify_handoff" => {
            let mut request: Packet28VerifyHandoffArgs = serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_verify_handoff(root, request)?
        }
        "packet28.prompt_pressure" => {
            let mut request: Packet28PromptPressureArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_prompt_pressure(root, request)?
        }
        "packet28.handoff_diff" => {
            let mut request: Packet28HandoffDiffArgs = serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .left_artifact_id
                    .as_deref()
                    .or(request.left_context_version.as_deref())
                    .or(request.right_artifact_id.as_deref())
                    .or(request.right_context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_diff(root, request)?
        }
        "packet28.handoff_compress" => {
            let mut request: Packet28HandoffCompressionArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_compress(root, request)?
        }
        "packet28.handoff_lint_dependencies" => {
            let mut request: Packet28HandoffDependencyLintArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_lint_dependencies(root, request)?
        }
        "packet28.handoff_lint_paths" => {
            let mut request: Packet28HandoffPathLintArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_lint_paths(root, request)?
        }
        "packet28.handoff_lint_tests" => {
            let mut request: Packet28HandoffTestLintArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_lint_tests(root, request)?
        }
        "packet28.handoff_lint_stale_commands" => {
            let mut request: Packet28HandoffStaleCommandLintArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_lint_stale_commands(root, request)?
        }
        "packet28.handoff_lint_environment" => {
            let mut request: Packet28HandoffEnvironmentLintArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_lint_environment(root, request)?
        }
        "packet28.handoff_lint_all" => {
            let mut request: Packet28HandoffLintAllArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_lint_all(root, request)?
        }
        "packet28.handoff_fix_plan" => {
            let mut request: Packet28HandoffFixPlanArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .artifact_id
                    .as_deref()
                    .or(request.context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_fix_plan(root, request)?
        }
        "packet28.handoff_repair_verify" => {
            let mut request: Packet28HandoffRepairVerifyArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request
                    .after_artifact_id
                    .as_deref()
                    .or(request.after_context_version.as_deref())
                    .or(request.before_artifact_id.as_deref())
                    .or(request.before_context_version.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_repair_verify(root, request)?
        }
        "packet28.handoff_lint_trends" => {
            let mut request: Packet28HandoffLintTrendArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(session, root, &request.task_id, None, name)?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_lint_trends(root, request)?
        }
        "packet28.handoff_lint_regressions" => {
            let mut request: Packet28HandoffLintRegressionArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(session, root, &request.task_id, None, name)?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_handoff_lint_regressions(root, request)?
        }
        "packet28.prepare_handoff" | "packet28.handoff" => {
            let mut request: Packet28PrepareHandoffArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(session, root, &request.task_id, None, name)?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_prepare_handoff(root, request)?
        }
        "packet28.validate_plan" => {
            let mut request: Packet28ValidatePlanArgs = serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(session, root, &request.task_id, None, name)?;
            track_task(session, root, &request.task_id)?;
            handle_packet28_validate_plan(root, request)?
        }
        "packet28.action_critic" => {
            let mut request: Packet28ActionCriticArgs = serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request.query.as_deref().or(request.tool_name.as_deref()),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_action_critic(root, request)?
        }
        "packet28.recommend_next_tool" => {
            let mut request: Packet28RecommendNextToolArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request.query.as_deref(),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_recommend_next_tool(root, request)?
        }
        "packet28.validate_tool_outcome" => {
            let mut request: Packet28ValidateToolOutcomeArgs =
                serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request.command.as_deref(),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_validate_tool_outcome(root, request)?
        }
        "packet28.agent_status" => handle_packet28_agent_status(root, arguments.clone())?,
        "packet28.patch_risk" => {
            let mut request: Packet28PatchRiskArgs = serde_json::from_value(arguments.clone())?;
            request.task_id = resolve_session_task_id(
                session,
                root,
                &request.task_id,
                request.paths.first().map(String::as_str),
                name,
            )?;
            track_task(session, root, &request.task_id)?;
            native_tools::handle_packet28_patch_risk(root, request)?
        }
        _ => return Ok(None),
    };
    Ok(Some(payload))
}
