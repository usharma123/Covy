use super::*;

use glob::Pattern;

pub(crate) fn handle_packet28_read_regions(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    args: Packet28ReadRegionsArgs,
) -> Result<Value> {
    let task_id = args.task_id.as_str();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.read_regions requires task_id"));
    }
    validated_task_storage_id(task_id)?;
    let (sequence, invocation_id) = next_task_invocation(session, task_id)?;
    let request_summary = read_regions_request_summary(&args, &args.path);
    write_native_tool_started(
        root,
        session,
        NativeToolStartedRecord {
            task_id,
            invocation_id: &invocation_id,
            sequence,
            tool_name: "packet28.read_regions",
            operation_kind: suite_packet_core::ToolOperationKind::Read,
            request_summary: request_summary.clone(),
        },
    )?;

    let started_at = Instant::now();
    let read_result = match packet28_reducer_core::read_regions(
        root,
        &packet28_reducer_core::ReadRegionsRequest {
            path: args.path.clone(),
            regions: args.regions.clone(),
            line_start: args.line_start,
            line_end: args.line_end,
        },
    ) {
        Ok(result) => result,
        Err(error) => {
            let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            write_native_tool_failure(
                root,
                session,
                NativeToolFailureRecord {
                    task_id,
                    invocation_id: &invocation_id,
                    sequence,
                    tool_name: "packet28.read_regions",
                    operation_kind: suite_packet_core::ToolOperationKind::Read,
                    request_summary,
                    error_message: error.to_string(),
                    compact_path: "native_tool",
                    raw_est_tokens: None,
                    reduced_est_tokens: None,
                    command: None,
                    raw_artifact_handle: None,
                    duration_ms,
                },
            )?;
            return Err(error);
        }
    };
    let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let result_summary = format!(
        "Read {} line(s) from {} across {} region(s)",
        read_result.lines.len(),
        read_result.path,
        read_result.regions.len()
    );
    let full_payload =
        build_read_regions_full_payload(task_id, &invocation_id, sequence, &read_result);
    let artifact_id = Some(store_result_artifact(
        root,
        task_id,
        full_payload["invocation_id"].as_str().unwrap_or_default(),
        &full_payload,
    )?);
    let payload = build_read_regions_response_payload(&full_payload, &args, artifact_id.clone());
    let raw_est_tokens = Some(estimate_tokens_for_value(&full_payload));
    let reduced_est_tokens = Some(estimate_tokens_for_value(&payload));
    write_native_tool_result(
        root,
        session,
        NativeToolResultRecord {
            task_id,
            invocation_id: payload["invocation_id"].as_str().unwrap_or_default(),
            sequence,
            tool_name: "packet28.read_regions",
            operation_kind: suite_packet_core::ToolOperationKind::Read,
            request_summary,
            result_summary,
            compact_path: "native_tool",
            raw_est_tokens,
            reduced_est_tokens,
            search_query: None,
            command: None,
            paths: vec![payload["path"].as_str().unwrap_or_default().to_string()],
            regions: json_array_strings(&full_payload, "regions"),
            symbols: json_array_strings(&full_payload, "symbols"),
            artifact_id,
            raw_artifact_handle: None,
            duration_ms,
        },
    )?;
    Ok(payload)
}

fn read_regions_request_summary(args: &Packet28ReadRegionsArgs, path: &str) -> String {
    if !args.regions.is_empty() {
        format!(
            "read_regions {path} using {} region hint(s)",
            args.regions.len()
        )
    } else if args.line_start.is_some() || args.line_end.is_some() {
        format!(
            "read_regions {path} lines {}-{}",
            args.line_start.unwrap_or(1),
            args.line_end.unwrap_or(args.line_start.unwrap_or(1))
        )
    } else {
        format!("read_regions {path}")
    }
}

pub(super) fn build_read_regions_full_payload(
    task_id: &str,
    invocation_id: &str,
    sequence: u64,
    read_result: &packet28_reducer_core::ReadRegionsResult,
) -> Value {
    json!({
        "task_id": task_id,
        "invocation_id": invocation_id,
        "sequence": sequence,
        "path": read_result.path,
        "regions": read_result.regions,
        "symbols": read_result.symbols,
        "content": render_read_region_content(&read_result.lines),
        "line_count": read_result.lines.len(),
        "compact_preview": read_result.compact_preview,
        "response_mode": "full",
    })
}

pub(super) fn build_read_regions_response_payload(
    full_payload: &Value,
    args: &Packet28ReadRegionsArgs,
    artifact_id: Option<String>,
) -> Value {
    match args.response_mode {
        Packet28SearchResponseMode::Full => {
            let mut payload = full_payload.clone();
            payload["artifact_id"] = json!(artifact_id);
            payload
        }
        Packet28SearchResponseMode::Slim => {
            let mut payload = json!({
                "path": full_payload["path"].clone(),
                "regions": full_payload["regions"].clone(),
                "symbols": full_payload["symbols"].clone(),
                "line_count": full_payload["line_count"].clone(),
                "compact_preview": full_payload["compact_preview"].clone(),
                "artifact_id": artifact_id,
                "response_mode": "slim",
            });
            if read_regions_request_is_explicit(args) {
                payload["content"] = full_payload["content"].clone();
            }
            payload
        }
    }
}

fn read_regions_request_is_explicit(args: &Packet28ReadRegionsArgs) -> bool {
    !args.regions.is_empty() || args.line_start.is_some() || args.line_end.is_some()
}

fn render_read_region_content(lines: &[packet28_reducer_core::ReadLine]) -> String {
    lines
        .iter()
        .map(|item| format!("{}: {}", item.line, item.text))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn handle_packet28_glob(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    args: Packet28GlobArgs,
) -> Result<Value> {
    let task_id = args.task_id.as_str();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.glob requires task_id"));
    }
    validated_task_storage_id(task_id)?;
    let pattern = args.pattern.trim();
    if pattern.is_empty() {
        return Err(anyhow!("packet28.glob requires pattern"));
    }
    let (sequence, invocation_id) = next_task_invocation(session, task_id)?;
    let request_summary = glob_request_summary(&args);
    write_native_tool_started(
        root,
        session,
        NativeToolStartedRecord {
            task_id,
            invocation_id: &invocation_id,
            sequence,
            tool_name: "packet28.glob",
            operation_kind: suite_packet_core::ToolOperationKind::Search,
            request_summary: request_summary.clone(),
        },
    )?;
    let started_at = Instant::now();
    let (resolved_paths, mut matches) = match collect_glob_matches(root, pattern, &args.paths) {
        Ok(result) => result,
        Err(error) => {
            let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            write_native_tool_failure(
                root,
                session,
                NativeToolFailureRecord {
                    task_id,
                    invocation_id: &invocation_id,
                    sequence,
                    tool_name: "packet28.glob",
                    operation_kind: suite_packet_core::ToolOperationKind::Search,
                    request_summary,
                    error_message: error.to_string(),
                    compact_path: "native_tool",
                    raw_est_tokens: None,
                    reduced_est_tokens: None,
                    command: None,
                    raw_artifact_handle: None,
                    duration_ms,
                },
            )?;
            return Err(error);
        }
    };
    let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let max_results = args.max_results.unwrap_or(200).clamp(1, 500);
    let truncated = matches.len() > max_results;
    if truncated {
        matches.truncate(max_results);
    }
    let compact_preview = render_glob_compact_preview(pattern, &matches);
    let result_summary = compact_preview
        .lines()
        .next()
        .unwrap_or("Glob completed")
        .to_string();
    let slim_preview = result_summary.clone();
    let matched_paths = matches.clone();
    let symbols = packet28_reducer_core::infer_symbols_from_pattern(pattern);
    let full_payload = json!({
        "task_id": task_id,
        "invocation_id": invocation_id,
        "sequence": sequence,
        "pattern": pattern,
        "requested_paths": args.paths,
        "resolved_paths": resolved_paths,
        "match_count": matches.len(),
        "truncated": truncated,
        "paths": matched_paths.clone(),
        "symbols": symbols.clone(),
        "compact_preview": compact_preview,
        "response_mode": "full",
    });
    let artifact_id = Some(store_result_artifact(
        root,
        task_id,
        full_payload["invocation_id"].as_str().unwrap_or_default(),
        &full_payload,
    )?);
    let payload = match args.response_mode {
        Packet28SearchResponseMode::Full => {
            let mut payload = full_payload.clone();
            payload["artifact_id"] = json!(artifact_id.clone());
            payload
        }
        Packet28SearchResponseMode::Slim => json!({
            "match_count": matches.len(),
            "compact_preview": slim_preview,
            "artifact_id": artifact_id.clone(),
            "response_mode": "slim",
        }),
    };
    let raw_est_tokens = Some(estimate_tokens_for_value(&full_payload));
    let reduced_est_tokens = Some(estimate_tokens_for_value(&payload));
    write_native_tool_result(
        root,
        session,
        NativeToolResultRecord {
            task_id,
            invocation_id: &invocation_id,
            sequence,
            tool_name: "packet28.glob",
            operation_kind: suite_packet_core::ToolOperationKind::Search,
            request_summary,
            result_summary,
            compact_path: "native_tool",
            raw_est_tokens,
            reduced_est_tokens,
            search_query: Some(pattern.to_string()),
            command: None,
            paths: matched_paths,
            regions: Vec::new(),
            symbols,
            artifact_id,
            raw_artifact_handle: None,
            duration_ms,
        },
    )?;
    Ok(payload)
}

fn glob_request_summary(args: &Packet28GlobArgs) -> String {
    if args.paths.is_empty() {
        format!(
            "glob '{}' across repo ({:?})",
            args.pattern, args.response_mode
        )
    } else {
        format!(
            "glob '{}' in {} path(s) ({:?})",
            args.pattern,
            args.paths.len(),
            args.response_mode
        )
    }
}

fn collect_glob_matches(
    root: &Path,
    pattern: &str,
    requested_paths: &[String],
) -> Result<(Vec<String>, Vec<String>)> {
    let compiled =
        Pattern::new(pattern).with_context(|| format!("invalid glob pattern '{pattern}'"))?;
    let mut stack = Vec::<std::path::PathBuf>::new();
    let mut resolved_paths = Vec::<String>::new();
    if requested_paths.is_empty() {
        stack.push(root.to_path_buf());
    } else {
        for requested in requested_paths {
            let normalized = packet28_reducer_core::normalize_capture_path(root, requested);
            if normalized.is_empty() {
                continue;
            }
            let candidate = root.join(&normalized);
            if candidate.exists() {
                resolved_paths.push(normalized);
                stack.push(candidate);
            }
        }
    }
    let mut matches = Vec::<String>::new();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            for entry in fs::read_dir(&path)
                .with_context(|| format!("failed to read directory '{}'", path.display()))?
            {
                let entry = entry?;
                let child = entry.path();
                let relative = packet28_reducer_core::normalize_capture_path(
                    root,
                    &child.display().to_string(),
                );
                if relative.starts_with(".git/") || relative.starts_with(".packet28/") {
                    continue;
                }
                if child.is_dir() {
                    stack.push(child);
                    continue;
                }
                if !relative.is_empty() && compiled.matches(&relative) {
                    matches.push(relative);
                }
            }
        } else {
            let relative =
                packet28_reducer_core::normalize_capture_path(root, &path.display().to_string());
            if !relative.is_empty() && compiled.matches(&relative) {
                matches.push(relative);
            }
        }
    }
    matches.sort();
    matches.dedup();
    resolved_paths.sort();
    resolved_paths.dedup();
    Ok((resolved_paths, matches))
}

fn render_glob_compact_preview(pattern: &str, matches: &[String]) -> String {
    let mut rendered = vec![format!(
        "Glob matched {} path(s) for '{}'.",
        matches.len(),
        pattern
    )];
    for path in matches.iter().take(12) {
        rendered.push(path.clone());
    }
    if matches.len() > 12 {
        rendered.push(format!("+{} more path(s)", matches.len() - 12));
    }
    rendered.join("\n")
}
