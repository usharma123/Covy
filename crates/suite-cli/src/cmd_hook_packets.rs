use packet28_daemon_core::{HookEventKind, HookReducerPacket, HookRuntimeConfig};
use packet28_reducer_core::classify_command;
use serde_json::{json, Value};

use crate::cmd_hook_support::{
    compact_text, extract_command_paths, first_nonempty_line, hook_output_text, json_array_len,
    json_array_strings, json_string, response_failed,
};

pub(crate) fn build_reducer_packet(
    runtime_config: &HookRuntimeConfig,
    payload: &Value,
    event_kind: HookEventKind,
) -> Option<HookReducerPacket> {
    if !matches!(
        event_kind,
        HookEventKind::PostToolUse | HookEventKind::PostToolUseFailure
    ) || !runtime_config.fallback_post_tool_capture
    {
        return None;
    }
    let tool_name = json_string(payload, "tool_name")?;
    let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
    if tool_name == "Bash"
        && json_string(&input, "command")
            .as_deref()
            .is_some_and(|command| command.contains(" hook reducer-runner "))
    {
        return None;
    }
    match event_kind {
        HookEventKind::PostToolUse => {
            let response = payload.get("tool_response").cloned().unwrap_or(Value::Null);
            match tool_name.as_str() {
                "Bash" => build_bash_packet(&input, &response),
                "Read" => build_read_packet(&input, &response),
                "Grep" => build_grep_packet(&input, &response),
                "Glob" => build_glob_packet(&input, &response),
                "Edit" | "MultiEdit" | "Write" => build_edit_packet(&tool_name, &input, &response),
                _ => Some(build_generic_packet(&tool_name, &input, &response)),
            }
        }
        HookEventKind::PostToolUseFailure => {
            let error = json_string(payload, "error").unwrap_or_else(|| hook_output_text(payload));
            Some(build_failed_tool_packet(
                &tool_name, &input, payload, &error,
            ))
        }
        _ => None,
    }
}

pub(crate) fn build_bash_packet(input: &Value, response: &Value) -> Option<HookReducerPacket> {
    let command = json_string(input, "command")?;
    if let Some(packet) = build_bash_grep_packet(&command, response) {
        return Some(packet);
    }
    let output = hook_output_text(response);
    let summary = first_nonempty_line(&output)
        .unwrap_or_else(|| format!("command completed: {}", compact_text(&command, 100)));
    let spec = classify_command(&command);
    let (packet_type, operation_kind, family, canonical_kind, fingerprint, paths, equivalence_key) =
        if let Some(spec) = spec {
            (
                format!("packet28.hook.fallback.{}.v1", spec.family),
                spec.operation_kind,
                Some(spec.family),
                Some(spec.canonical_kind),
                Some(spec.cache_fingerprint),
                spec.paths,
                spec.equivalence_key,
            )
        } else {
            (
                "packet28.hook.command.v1".to_string(),
                suite_packet_core::ToolOperationKind::Generic,
                Some("generic".to_string()),
                None,
                None,
                extract_command_paths(&command),
                None,
            )
        };
    Some(packet_from_parts(
        &packet_type,
        "Bash",
        operation_kind,
        family,
        canonical_kind,
        summary,
        Some(command),
        None,
        paths,
        Vec::new(),
        Vec::new(),
        equivalence_key,
        fingerprint,
        Some(false),
        response.clone(),
        response_failed(response),
    ))
}

fn build_bash_grep_packet(command: &str, response: &Value) -> Option<HookReducerPacket> {
    let (query, paths) = parse_bash_grep_command(command)?;
    let input = json!({
        "pattern": query,
        "include": paths,
    });
    let mut packet = build_grep_packet(&input, response)?;
    packet.packet_type = "packet28.hook.bash.grep.v1".to_string();
    packet.tool_name = "Bash".to_string();
    packet.command = Some(command.to_string());
    packet.reducer_family = Some("shell_native".to_string());
    packet.compact_path = Some("bash_grep_post_capture".to_string());
    Some(packet)
}

fn parse_bash_grep_command(command: &str) -> Option<(String, Vec<String>)> {
    let argv = shell_words::split(command).ok()?;
    if !matches!(argv.first().map(String::as_str), Some("grep" | "rg")) {
        return None;
    }
    parse_grep_argv(&argv)
}

fn parse_grep_argv(argv: &[String]) -> Option<(String, Vec<String>)> {
    let mut query = None::<String>;
    let mut paths = Vec::<String>::new();
    let mut iter = argv.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-e" | "--regexp" => {
                query = iter.next().cloned();
            }
            "-m" | "--max-count" | "-A" | "--after-context" | "-B" | "--before-context" | "-C"
            | "--context" | "--include" | "--exclude" | "--glob" | "-g" => {
                let _ = iter.next();
            }
            "-F" | "--fixed-strings" | "-i" | "--ignore-case" | "-w" | "--word-regexp" | "-n"
            | "--line-number" | "-H" | "--with-filename" | "-h" | "--no-filename" | "-I" | "-R"
            | "-r" | "--recursive" | "--hidden" | "--no-heading" => {}
            value if value.starts_with('-') => {}
            value => {
                if query.is_none() {
                    query = Some(value.to_string());
                } else {
                    paths.push(value.to_string());
                }
            }
        }
    }
    Some((query?, paths))
}

pub(crate) fn build_read_packet(input: &Value, response: &Value) -> Option<HookReducerPacket> {
    let path = json_string(input, "file_path")
        .or_else(|| json_string(input, "path"))
        .or_else(|| json_string(input, "target"))?;
    let line_start = input.get("offset").and_then(Value::as_u64).unwrap_or(1);
    let count = input.get("limit").and_then(Value::as_u64).unwrap_or(1);
    let line_end = line_start.saturating_add(count.saturating_sub(1));
    let summary = format!("Read {} lines from {}", count, path);
    let mut regions = json_array_strings(response, "regions");
    if regions.is_empty() {
        regions.push(format!("{path}:{line_start}-{line_end}"));
    }
    Some(packet_from_parts(
        "packet28.hook.read.v1",
        "Read",
        suite_packet_core::ToolOperationKind::Read,
        Some("claude_native".to_string()),
        Some("read".to_string()),
        summary,
        None,
        None,
        vec![path.clone()],
        regions,
        json_array_strings(response, "symbols"),
        Some(format!("read:{path}")),
        Some(format!("read:{}:{}:{}", path, line_start, line_end)),
        Some(true),
        response.clone(),
        response_failed(response),
    ))
}

pub(crate) fn build_grep_packet(input: &Value, response: &Value) -> Option<HookReducerPacket> {
    let query = json_string(input, "pattern")
        .or_else(|| json_string(input, "query"))
        .or_else(|| json_string(input, "search"))?;
    let include_paths = json_array_strings(input, "include");
    let default_path = (include_paths.len() == 1).then(|| include_paths[0].clone());
    let extracted_matches = extract_grep_hook_matches(response, default_path.as_deref());
    let mut paths = json_array_strings(response, "files")
        .into_iter()
        .chain(include_paths)
        .chain(extracted_matches.iter().map(|item| item.0.clone()))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    let regions = extracted_matches
        .iter()
        .map(|(path, line, _)| format!("{path}:{line}-{line}"))
        .collect::<Vec<_>>();
    let count = json_array_len(response, "matches").unwrap_or_else(|| {
        if extracted_matches.is_empty() {
            hook_output_text(response).lines().count()
        } else {
            extracted_matches.len()
        }
    });
    let summary = format!("Grep found {count} matches for '{query}'");
    let compact_preview = render_grep_hook_preview(&summary, &extracted_matches);
    let mut packet = packet_from_parts(
        "packet28.hook.grep.v1",
        "Grep",
        suite_packet_core::ToolOperationKind::Search,
        Some("claude_native".to_string()),
        Some("grep".to_string()),
        summary,
        None,
        Some(query.clone()),
        paths.clone(),
        regions,
        Vec::new(),
        Some(format!("grep:{}:{}", query, paths.join(","))),
        Some(format!("grep:{}:{}", query, paths.join(","))),
        Some(true),
        response.clone(),
        response_failed(response),
    );
    packet.compact_preview = Some(compact_preview);
    Some(packet)
}

fn extract_grep_hook_matches(
    response: &Value,
    default_path: Option<&str>,
) -> Vec<(String, usize, String)> {
    if let Some(items) = response.get("matches").and_then(Value::as_array) {
        let mut matches = Vec::new();
        for item in items {
            let Some(path) = json_string(item, "path").or_else(|| json_string(item, "file")) else {
                continue;
            };
            let Some(line) = item
                .get("line")
                .or_else(|| item.get("line_number"))
                .and_then(Value::as_u64)
                .and_then(|line| usize::try_from(line).ok())
            else {
                continue;
            };
            let text = json_string(item, "text")
                .or_else(|| json_string(item, "line_text"))
                .unwrap_or_default();
            matches.push((path, line, text));
        }
        if !matches.is_empty() {
            return matches;
        }
    }
    hook_output_text(response)
        .lines()
        .filter_map(|line| parse_grep_hook_output_line(line, default_path))
        .collect()
}

fn parse_grep_hook_output_line(
    line: &str,
    default_path: Option<&str>,
) -> Option<(String, usize, String)> {
    let (first, rest) = line.split_once(':')?;
    let (path, line_no, text) = if first.parse::<usize>().is_ok() {
        let default_path = default_path?;
        (
            default_path.to_string(),
            first.to_string(),
            rest.to_string(),
        )
    } else {
        let (line_no, text) = rest.split_once(':')?;
        (first.to_string(), line_no.to_string(), text.to_string())
    };
    let line_no = line_no.parse::<usize>().ok()?;
    let path = path.trim().trim_start_matches("./").replace('\\', "/");
    if path.is_empty() {
        return None;
    }
    Some((path, line_no, text))
}

fn render_grep_hook_preview(summary: &str, matches: &[(String, usize, String)]) -> String {
    let mut lines = vec![summary.to_string()];
    for (path, line, text) in matches.iter().take(12) {
        lines.push(format!("{path}:{line}:{}", compact_text(text, 120)));
    }
    if matches.len() > 12 {
        lines.push(format!("+{} more match(es)", matches.len() - 12));
    }
    lines.join("\n")
}

fn build_glob_packet(input: &Value, response: &Value) -> Option<HookReducerPacket> {
    let pattern = json_string(input, "pattern")?;
    let paths = if let Some(array) = response.as_array() {
        array
            .iter()
            .filter_map(|item| item.as_str().map(ToOwned::to_owned))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let summary = format!("Glob matched {} path(s) for '{}'", paths.len(), pattern);
    Some(packet_from_parts(
        "packet28.hook.glob.v1",
        "Glob",
        suite_packet_core::ToolOperationKind::Search,
        Some("claude_native".to_string()),
        Some("glob".to_string()),
        summary,
        None,
        Some(pattern.clone()),
        paths,
        Vec::new(),
        Vec::new(),
        Some(format!("glob:{pattern}")),
        Some(format!("glob:{pattern}")),
        Some(true),
        response.clone(),
        response_failed(response),
    ))
}

fn build_failed_tool_packet(
    tool_name: &str,
    input: &Value,
    payload: &Value,
    error: &str,
) -> HookReducerPacket {
    match tool_name {
        "Bash" => build_bash_failure_packet(input, payload, error)
            .unwrap_or_else(|| build_generic_failure_packet(tool_name, input, payload, error)),
        "Read" => build_read_failure_packet(input, payload, error)
            .unwrap_or_else(|| build_generic_failure_packet(tool_name, input, payload, error)),
        "Grep" => build_grep_failure_packet(input, payload, error)
            .unwrap_or_else(|| build_generic_failure_packet(tool_name, input, payload, error)),
        "Glob" => build_glob_failure_packet(input, payload, error)
            .unwrap_or_else(|| build_generic_failure_packet(tool_name, input, payload, error)),
        "Edit" | "MultiEdit" | "Write" => {
            build_edit_failure_packet(tool_name, input, payload, error)
                .unwrap_or_else(|| build_generic_failure_packet(tool_name, input, payload, error))
        }
        _ => build_generic_failure_packet(tool_name, input, payload, error),
    }
}

fn build_bash_failure_packet(
    input: &Value,
    payload: &Value,
    error: &str,
) -> Option<HookReducerPacket> {
    let command = json_string(input, "command")?;
    let spec = classify_command(&command);
    let summary = first_nonempty_line(error)
        .unwrap_or_else(|| format!("command failed: {}", compact_text(&command, 100)));
    let (packet_type, operation_kind, family, canonical_kind, paths, equivalence_key) =
        if let Some(spec) = spec {
            (
                format!("packet28.hook.fallback.{}.failure.v1", spec.family),
                spec.operation_kind,
                Some(spec.family),
                Some(spec.canonical_kind),
                spec.paths,
                spec.equivalence_key,
            )
        } else {
            (
                "packet28.hook.command.failure.v1".to_string(),
                suite_packet_core::ToolOperationKind::Generic,
                Some("generic".to_string()),
                None,
                extract_command_paths(&command),
                None,
            )
        };
    Some(packet_from_parts(
        &packet_type,
        "Bash",
        operation_kind,
        family,
        canonical_kind,
        summary,
        Some(command),
        None,
        paths,
        Vec::new(),
        Vec::new(),
        equivalence_key,
        None,
        Some(false),
        payload.clone(),
        true,
    ))
}

fn build_read_failure_packet(
    input: &Value,
    payload: &Value,
    error: &str,
) -> Option<HookReducerPacket> {
    let path = json_string(input, "file_path")
        .or_else(|| json_string(input, "path"))
        .or_else(|| json_string(input, "target"))?;
    let line_start = input.get("offset").and_then(Value::as_u64).unwrap_or(1);
    let count = input.get("limit").and_then(Value::as_u64).unwrap_or(1);
    let line_end = line_start.saturating_add(count.saturating_sub(1));
    Some(packet_from_parts(
        "packet28.hook.read.failure.v1",
        "Read",
        suite_packet_core::ToolOperationKind::Read,
        Some("claude_native".to_string()),
        Some("read".to_string()),
        format!("Read failed for {}: {}", path, compact_text(error, 140)),
        None,
        None,
        vec![path.clone()],
        vec![format!("{path}:{line_start}-{line_end}")],
        Vec::new(),
        Some(format!("read:{path}")),
        None,
        Some(false),
        payload.clone(),
        true,
    ))
}

fn build_grep_failure_packet(
    input: &Value,
    payload: &Value,
    error: &str,
) -> Option<HookReducerPacket> {
    let query = json_string(input, "pattern")
        .or_else(|| json_string(input, "query"))
        .or_else(|| json_string(input, "search"))?;
    let paths = json_array_strings(input, "include");
    Some(packet_from_parts(
        "packet28.hook.grep.failure.v1",
        "Grep",
        suite_packet_core::ToolOperationKind::Search,
        Some("claude_native".to_string()),
        Some("grep".to_string()),
        format!("Grep failed for '{}': {}", query, compact_text(error, 140)),
        None,
        Some(query.clone()),
        paths.clone(),
        Vec::new(),
        Vec::new(),
        Some(format!("grep:{}:{}", query, paths.join(","))),
        None,
        Some(false),
        payload.clone(),
        true,
    ))
}

fn build_glob_failure_packet(
    input: &Value,
    payload: &Value,
    error: &str,
) -> Option<HookReducerPacket> {
    let pattern = json_string(input, "pattern")?;
    Some(packet_from_parts(
        "packet28.hook.glob.failure.v1",
        "Glob",
        suite_packet_core::ToolOperationKind::Search,
        Some("claude_native".to_string()),
        Some("glob".to_string()),
        format!(
            "Glob failed for '{}': {}",
            pattern,
            compact_text(error, 140)
        ),
        None,
        Some(pattern.clone()),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Some(format!("glob:{pattern}")),
        None,
        Some(false),
        payload.clone(),
        true,
    ))
}

fn build_edit_failure_packet(
    tool_name: &str,
    input: &Value,
    payload: &Value,
    error: &str,
) -> Option<HookReducerPacket> {
    let path = json_string(input, "file_path")
        .or_else(|| json_string(input, "path"))
        .or_else(|| json_string(input, "target"))?;
    Some(packet_from_parts(
        "packet28.hook.edit.failure.v1",
        tool_name,
        suite_packet_core::ToolOperationKind::Edit,
        Some("claude_native".to_string()),
        Some("edit".to_string()),
        format!(
            "{} failed for {}: {}",
            tool_name,
            path,
            compact_text(error, 140)
        ),
        None,
        None,
        vec![path],
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        payload.clone(),
        true,
    ))
}

fn build_generic_failure_packet(
    tool_name: &str,
    input: &Value,
    payload: &Value,
    error: &str,
) -> HookReducerPacket {
    packet_from_parts(
        "packet28.hook.generic.failure.v1",
        tool_name,
        suite_packet_core::ToolOperationKind::Generic,
        Some("claude_native".to_string()),
        None,
        format!("{tool_name} failed: {}", compact_text(error, 140)),
        json_string(input, "command"),
        None,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        payload.clone(),
        true,
    )
}

fn build_edit_packet(
    tool_name: &str,
    input: &Value,
    response: &Value,
) -> Option<HookReducerPacket> {
    let path = json_string(input, "file_path")
        .or_else(|| json_string(input, "path"))
        .or_else(|| json_string(input, "target"))?;
    let summary = format!("{tool_name} updated {path}");
    Some(packet_from_parts(
        "packet28.hook.edit.v1",
        tool_name,
        suite_packet_core::ToolOperationKind::Edit,
        Some("claude_native".to_string()),
        Some("edit".to_string()),
        summary,
        None,
        None,
        vec![path],
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        json!({
            "tool_input": input,
            "tool_response": response,
        }),
        response_failed(response),
    ))
}

fn build_generic_packet(tool_name: &str, input: &Value, response: &Value) -> HookReducerPacket {
    packet_from_parts(
        "packet28.hook.generic.v1",
        tool_name,
        suite_packet_core::ToolOperationKind::Generic,
        Some("claude_native".to_string()),
        None,
        format!("{tool_name} completed"),
        None,
        None,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        None,
        Some(false),
        json!({
            "tool_input": input,
            "tool_response": response,
        }),
        response_failed(response),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn packet_from_parts(
    packet_type: &str,
    tool_name: &str,
    operation_kind: suite_packet_core::ToolOperationKind,
    reducer_family: Option<String>,
    canonical_command_kind: Option<String>,
    summary: String,
    command: Option<String>,
    search_query: Option<String>,
    paths: Vec<String>,
    regions: Vec<String>,
    symbols: Vec<String>,
    equivalence_key: Option<String>,
    cache_fingerprint: Option<String>,
    cacheable: Option<bool>,
    artifact: Value,
    failed: bool,
) -> HookReducerPacket {
    let raw_text = hook_output_text(&artifact);
    let raw_est_tokens = (((raw_text.len()) as f64) / 4.0).ceil() as u64;
    let est_bytes = summary.len() as u64;
    let est_tokens = ((est_bytes as f64) / 4.0).ceil() as u64;
    let compact_path = if reducer_family.as_deref() == Some("claude_native") {
        Some("native_tool".to_string())
    } else if tool_name == "Bash" {
        Some("raw_passthrough".to_string())
    } else {
        Some("native_tool".to_string())
    };
    let passthrough_reason = (tool_name == "Bash").then(|| "post_tool_capture".to_string());
    HookReducerPacket {
        packet_type: packet_type.to_string(),
        tool_name: tool_name.to_string(),
        operation_kind,
        reducer_family,
        canonical_command_kind,
        summary,
        compact_preview: None,
        command,
        search_query,
        compact_path,
        passthrough_reason,
        raw_est_tokens: Some(raw_est_tokens),
        reduced_est_tokens: Some(est_tokens),
        paths,
        regions,
        symbols,
        equivalence_key,
        est_tokens,
        est_bytes,
        failed,
        error_class: failed.then(|| "tool_error".to_string()),
        error_message: failed.then(|| compact_text(&hook_output_text(&artifact), 200)),
        retryable: failed.then_some(false),
        duration_ms: None,
        exit_code: None,
        cache_fingerprint,
        cacheable,
        mutation: Some(false),
        raw_artifact_handle: None,
        raw_artifact_available: false,
        artifact: Some(artifact),
    }
}
