use std::path::Path;

use packet28_reducer_core::{classify_command_argv, CommandReducerSpec};

#[path = "route_registry_native.rs"]
mod route_registry_native;
#[path = "route_registry_policy.rs"]
mod route_registry_policy;
use route_registry_native::{
    apply_postprocess_to_native_tool, build_native_tool_rewrite, classify_native_tool,
    contains_glob_chars, contains_unsupported_glob_tokens, Postprocess,
};
use route_registry_policy::{configured_wrapper_prefix, policy_excludes_command};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteKind {
    ReducerRewrite,
    NativeTool,
    TomlFilterRewrite,
    CompoundRewrite,
    ProxyPassthrough,
    RawPassthrough,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolKind {
    Tree,
    Read,
    Grep,
    Env,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolPlan {
    pub kind: NativeToolKind,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RouteDecision {
    pub kind: RouteKind,
    pub reason: Option<String>,
    pub argv: Vec<String>,
    pub env_assignments: Vec<(String, String)>,
    pub reducer_spec: Option<CommandReducerSpec>,
    pub native_tool: Option<NativeToolPlan>,
    /// Original argv before glob expansion, when expansion was applied.
    pub original_argv: Option<Vec<String>>,
    pub wrapper_prefix: Vec<String>,
    pub original_command: Option<String>,
}

pub fn decide_command_route(command: &str) -> RouteDecision {
    decide_command_route_inner(command, None, None, true)
}

pub fn decide_command_route_with_cwd(command: &str, cwd: &Path) -> RouteDecision {
    decide_command_route_inner(command, Some(cwd), None, true)
}

pub fn decide_command_route_with_cwd_and_root(
    command: &str,
    cwd: &Path,
    root: &Path,
) -> RouteDecision {
    decide_command_route_inner(command, Some(cwd), Some(root), true)
}

fn decide_command_route_inner(
    command: &str,
    cwd: Option<&Path>,
    policy_root: Option<&Path>,
    allow_compound: bool,
) -> RouteDecision {
    let raw_trimmed = command.trim();
    let sanitized = strip_supported_trailing_redirects(raw_trimmed);
    if sanitized != raw_trimmed && contains_shell_composition(&sanitized) {
        return raw_passthrough("shell_composition");
    }
    let (normalized, postprocess) = strip_supported_postprocess(&sanitized);
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return RouteDecision {
            kind: RouteKind::RawPassthrough,
            reason: Some("empty_command".to_string()),
            argv: Vec::new(),
            env_assignments: Vec::new(),
            reducer_spec: None,
            native_tool: None,
            original_argv: None,
            wrapper_prefix: Vec::new(),
            original_command: Some(trimmed.to_string()),
        };
    }

    if contains_disallowed_shell_expansion(trimmed) {
        return raw_passthrough("shell_expansion");
    }
    if contains_shell_composition(trimmed) {
        if allow_compound && compound_has_rewritable_segment(trimmed, cwd, policy_root) {
            return RouteDecision {
                kind: RouteKind::CompoundRewrite,
                reason: Some("compound_rewrite".to_string()),
                argv: Vec::new(),
                env_assignments: Vec::new(),
                reducer_spec: None,
                native_tool: None,
                original_argv: None,
                wrapper_prefix: Vec::new(),
                original_command: Some(trimmed.to_string()),
            };
        }
        return raw_passthrough("shell_composition");
    }

    let Ok(argv) = shell_words::split(trimmed) else {
        return raw_passthrough("shell_parse_error");
    };
    let (mut env_assignments, mut argv) = split_leading_env_assignments(argv);
    let mut wrapper_prefix = Vec::new();
    if argv.is_empty() {
        return raw_passthrough("empty_command");
    }
    strip_supported_runtime_prefixes(&mut argv, &mut env_assignments, &mut wrapper_prefix);
    if argv.is_empty() {
        return raw_passthrough("empty_command");
    }
    if has_rewrite_disabled_prefix(&env_assignments) {
        return raw_passthrough("rewrite_disabled");
    }
    normalize_absolute_binary_path(&mut argv);
    if is_packet28_invocation(&argv) {
        return raw_passthrough("already_packet28");
    }
    if is_rtk_ignored_command(&argv) {
        return raw_passthrough("ignored_command");
    }
    if is_grep_extraction_command(&argv) {
        return raw_passthrough("grep_extraction_mode");
    }
    if policy_excludes_command(trimmed, &argv, policy_root) {
        return raw_passthrough("config_excluded");
    }
    let configured_wrapper_prefix =
        configured_wrapper_prefix(&argv, policy_root).unwrap_or_default();
    if !configured_wrapper_prefix.is_empty() {
        argv = argv[configured_wrapper_prefix.len()..].to_vec();
        wrapper_prefix.extend(configured_wrapper_prefix);
        if argv.is_empty() {
            return raw_passthrough("empty_command");
        }
    }

    let mut original_argv = None;
    if let Some(cwd) = cwd {
        if let Some(expanded) = try_expand_globs(&argv, cwd) {
            original_argv = Some(argv);
            argv = expanded;
        }
    }

    if env_assignments.is_empty() {
        if let Some(mut native_tool) = classify_native_tool(&argv) {
            if contains_unsupported_glob_tokens(&argv, Some(&native_tool)) {
                return raw_passthrough("shell_glob");
            }
            if !apply_postprocess_to_native_tool(&mut native_tool, postprocess.as_ref()) {
                return raw_passthrough("unsupported_postprocess");
            }
            return RouteDecision {
                kind: RouteKind::NativeTool,
                reason: None,
                argv,
                env_assignments,
                reducer_spec: None,
                native_tool: Some(native_tool),
                original_argv,
                wrapper_prefix,
                original_command: Some(trimmed.to_string()),
            };
        }
    }

    if contains_unsupported_glob_tokens(&argv, None) {
        return raw_passthrough("shell_glob");
    }

    let normalized = shell_join(&argv);
    let classification_argv = normalize_git_global_options_for_classification(&argv)
        .or_else(|| normalize_golangci_global_options_for_classification(&argv));
    let classification_command = classification_argv
        .as_ref()
        .map(|argv| shell_join(argv))
        .unwrap_or_else(|| normalized.clone());
    let classification_argv = classification_argv.as_deref().unwrap_or(&argv);
    if let Some(spec) = classify_command_argv(&classification_command, classification_argv) {
        return RouteDecision {
            kind: RouteKind::ReducerRewrite,
            reason: None,
            argv,
            env_assignments,
            reducer_spec: Some(spec),
            native_tool: None,
            original_argv,
            wrapper_prefix,
            original_command: Some(trimmed.to_string()),
        };
    }

    if suite_proxy_core::command_supported(&argv) {
        return RouteDecision {
            kind: RouteKind::ProxyPassthrough,
            reason: Some("proxy_summary".to_string()),
            argv,
            env_assignments,
            reducer_spec: None,
            native_tool: None,
            original_argv,
            wrapper_prefix,
            original_command: Some(trimmed.to_string()),
        };
    }

    if let Some(root) = policy_root {
        if let Some(_filter_name) = crate::toml_filters::matching_filter_name(root, &normalized)
            .ok()
            .flatten()
        {
            return RouteDecision {
                kind: RouteKind::TomlFilterRewrite,
                reason: Some("toml_filter".to_string()),
                argv,
                env_assignments,
                reducer_spec: None,
                native_tool: None,
                original_argv,
                wrapper_prefix,
                original_command: Some(trimmed.to_string()),
            };
        }
    }

    raw_passthrough("unsupported_command")
}

fn try_expand_globs(argv: &[String], cwd: &Path) -> Option<Vec<String>> {
    const MAX_MATCHES_PER_PATTERN: usize = 100;
    const MAX_TOTAL_ARGS: usize = 500;

    let command_name = argv.first().map(String::as_str);
    if matches!(command_name, Some("grep" | "rg" | "find")) {
        return None;
    }
    let path_only_command = matches!(
        command_name,
        Some("ls" | "cat" | "head" | "tail" | "sed" | "wc" | "diff")
    );

    let mut expanded = Vec::with_capacity(argv.len());
    let mut total_args = 0usize;
    let mut any_expanded = false;

    for (i, arg) in argv.iter().enumerate() {
        if arg.starts_with('-') {
            expanded.push(arg.clone());
            total_args += 1;
            continue;
        }
        if i == 0 {
            expanded.push(arg.clone());
            total_args += 1;
            continue;
        }
        if i == 1 && !contains_glob_chars(arg) {
            expanded.push(arg.clone());
            total_args += 1;
            continue;
        }
        if !contains_glob_chars(arg) {
            expanded.push(arg.clone());
            total_args += 1;
            continue;
        }
        if !path_only_command && !arg.contains('/') && !Path::new(arg).is_absolute() {
            expanded.push(arg.clone());
            total_args += 1;
            continue;
        }

        let pattern = if Path::new(arg).is_absolute() {
            arg.clone()
        } else {
            cwd.join(arg).display().to_string()
        };

        let mut matches: Vec<String> = match glob::glob(&pattern) {
            Ok(iter) => iter
                .filter_map(Result::ok)
                .take(MAX_MATCHES_PER_PATTERN)
                .map(|p| {
                    let s = p.display().to_string();
                    if Path::new(arg).is_absolute() {
                        s
                    } else if let Ok(rel) = p.strip_prefix(cwd) {
                        rel.display().to_string()
                    } else {
                        s
                    }
                })
                .collect(),
            Err(_) => {
                expanded.push(arg.clone());
                total_args += 1;
                continue;
            }
        };

        if matches.is_empty() {
            expanded.push(arg.clone());
            total_args += 1;
            continue;
        }

        matches.sort();
        matches.dedup();
        any_expanded = true;
        for m in matches {
            expanded.push(m);
            total_args += 1;
            if total_args > MAX_TOTAL_ARGS {
                return None;
            }
        }
    }

    if any_expanded {
        Some(expanded)
    } else {
        None
    }
}

pub fn build_route_rewrite(
    root: &Path,
    task_id: &str,
    session_id: Option<&str>,
    cwd: &str,
    decision: &RouteDecision,
) -> Option<String> {
    let rewritten = match decision.kind {
        RouteKind::ReducerRewrite => {
            let spec = decision.reducer_spec.as_ref()?;
            build_reducer_rewrite(
                root,
                task_id,
                session_id,
                cwd,
                spec,
                &decision.argv,
                &decision.env_assignments,
            )
        }
        RouteKind::NativeTool => {
            let native_tool = decision.native_tool.as_ref()?;
            build_native_tool_rewrite(root, task_id, cwd, native_tool)
        }
        RouteKind::TomlFilterRewrite => {
            build_toml_filter_rewrite(root, cwd, &decision.argv, &decision.env_assignments)
        }
        RouteKind::CompoundRewrite => {
            let command = decision.original_command.as_deref()?;
            build_compound_rewrite(root, task_id, session_id, cwd, command)?
        }
        RouteKind::ProxyPassthrough => build_proxy_rewrite(root, task_id, cwd, &decision.argv),
        RouteKind::RawPassthrough => return None,
    };
    let rewritten = apply_wrapper_prefix(&decision.wrapper_prefix, rewritten);
    is_rewrite_output_safe(&rewritten).then_some(rewritten)
}

fn raw_passthrough(reason: &str) -> RouteDecision {
    RouteDecision {
        kind: RouteKind::RawPassthrough,
        reason: Some(reason.to_string()),
        argv: Vec::new(),
        env_assignments: Vec::new(),
        reducer_spec: None,
        native_tool: None,
        original_argv: None,
        wrapper_prefix: Vec::new(),
        original_command: None,
    }
}

fn split_leading_env_assignments(argv: Vec<String>) -> (Vec<(String, String)>, Vec<String>) {
    let mut assignments = Vec::new();
    let mut idx = 0usize;
    while idx < argv.len() && looks_like_env_assignment(&argv[idx]) {
        let mut parts = argv[idx].splitn(2, '=');
        let key = parts.next().unwrap_or_default().trim().to_string();
        let value = parts.next().unwrap_or_default().to_string();
        assignments.push((key, value));
        idx += 1;
    }
    (assignments, argv[idx..].to_vec())
}

fn strip_supported_runtime_prefixes(
    argv: &mut Vec<String>,
    env_assignments: &mut Vec<(String, String)>,
    wrapper_prefix: &mut Vec<String>,
) {
    loop {
        if argv.first().is_some_and(|arg| arg == "sudo") && argv.len() > 1 {
            wrapper_prefix.push(argv.remove(0));
            continue;
        }
        if argv.first().is_some_and(|arg| {
            matches!(
                arg.as_str(),
                "noglob" | "command" | "builtin" | "exec" | "nocorrect"
            )
        }) && argv.len() > 1
        {
            wrapper_prefix.push(argv.remove(0));
            continue;
        }
        if argv.first().is_some_and(|arg| arg == "env") && argv.len() > 2 {
            argv.remove(0);
            let mut consumed = 0usize;
            while consumed < argv.len() && looks_like_env_assignment(&argv[consumed]) {
                let mut parts = argv[consumed].splitn(2, '=');
                let key = parts.next().unwrap_or_default().trim().to_string();
                let value = parts.next().unwrap_or_default().to_string();
                env_assignments.push((key, value));
                consumed += 1;
            }
            if consumed > 0 && consumed < argv.len() {
                argv.drain(0..consumed);
                continue;
            }
            break;
        }
        break;
    }
}

fn normalize_absolute_binary_path(argv: &mut [String]) {
    let Some(first) = argv.first_mut() else {
        return;
    };
    let path = Path::new(first);
    if !path.is_absolute() {
        return;
    }
    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        *first = name.to_string();
    }
}

fn has_rewrite_disabled_prefix(env_assignments: &[(String, String)]) -> bool {
    env_assignments.iter().any(|(key, value)| {
        matches!(key.as_str(), "PACKET28_DISABLED" | "RTK_DISABLED")
            && !matches!(value.trim(), "" | "0" | "false" | "FALSE" | "False")
    })
}

fn is_packet28_invocation(argv: &[String]) -> bool {
    argv.first()
        .and_then(|arg| Path::new(arg).file_name())
        .and_then(|name| name.to_str())
        .map(|name| matches!(name, "Packet28" | "packet28" | "covy"))
        .unwrap_or(false)
}

fn is_grep_extraction_command(argv: &[String]) -> bool {
    matches!(argv.first().map(String::as_str), Some("grep" | "rg"))
        && argv.iter().skip(1).any(|arg| match arg.as_str() {
            "-o"
            | "--only-matching"
            | "-c"
            | "--count"
            | "-l"
            | "--files-with-matches"
            | "-L"
            | "--files-without-match"
            | "-q"
            | "--quiet"
            | "--silent" => true,
            value if value.starts_with("--only-matching=") || value.starts_with("--count=") => true,
            value if value.starts_with('-') && !value.starts_with("--") => value
                .trim_start_matches('-')
                .chars()
                .any(|ch| matches!(ch, 'o' | 'c' | 'l' | 'L' | 'q')),
            _ => false,
        })
}

fn is_rtk_ignored_command(argv: &[String]) -> bool {
    let Some(first) = argv.first().map(String::as_str) else {
        return true;
    };
    const IGNORED_EXACT: &[&str] = &[
        "cd", "echo", "true", "false", "wait", "pwd", "bash", "sh", "fi", "done",
    ];
    if argv.len() == 1 && IGNORED_EXACT.contains(&first) {
        return true;
    }
    match first {
        "cd" | "echo" | "printf" | "export" | "source" | "mkdir" | "rm" | "mv" | "cp" | "chmod"
        | "chown" | "touch" | "which" | "type" | "test" | "sleep" | "kill" | "set" | "unset"
        | "sort" | "uniq" | "tr" | "cut" | "awk" | "sed" | "bash" | "sh" | "for" | "while"
        | "if" | "case" | "then" | "else" | "do" => true,
        "python3" | "python" => argv.get(1).map(String::as_str) == Some("-c"),
        "node" | "ruby" => argv.get(1).map(String::as_str) == Some("-e"),
        _ => false,
    }
}

fn apply_wrapper_prefix(prefix: &[String], rewritten: String) -> String {
    if prefix.is_empty() {
        rewritten
    } else {
        format!("{} {}", shell_join(prefix), rewritten)
    }
}

fn normalize_git_global_options_for_classification(argv: &[String]) -> Option<Vec<String>> {
    if argv.first().map(String::as_str) != Some("git") {
        return None;
    }
    let mut normalized = vec!["git".to_string()];
    let mut idx = 1usize;
    let mut removed_any = false;
    while idx < argv.len() {
        match argv[idx].as_str() {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" => {
                if idx + 1 >= argv.len() {
                    return None;
                }
                idx += 2;
                removed_any = true;
            }
            arg if arg.starts_with("--git-dir=")
                || arg.starts_with("--work-tree=")
                || arg.starts_with("--namespace=")
                || arg.starts_with("-c") && arg.len() > 2 =>
            {
                idx += 1;
                removed_any = true;
            }
            _ => break,
        }
    }
    if !removed_any {
        return None;
    }
    normalized.extend(argv[idx..].iter().cloned());
    Some(normalized)
}

fn normalize_golangci_global_options_for_classification(argv: &[String]) -> Option<Vec<String>> {
    let first = argv.first()?.as_str();
    if !matches!(first, "golangci-lint" | "golangci") {
        return None;
    }
    let mut idx = 1usize;
    while idx < argv.len() {
        let arg = argv[idx].as_str();
        if arg == "--" {
            return None;
        }
        if !arg.starts_with('-') {
            if arg == "run" {
                let mut normalized = Vec::with_capacity(argv.len() - idx + 1);
                normalized.push("golangci-lint".to_string());
                normalized.extend(argv[idx..].iter().cloned());
                return Some(normalized);
            }
            return None;
        }
        if golangci_global_flag_takes_value(arg) {
            idx += 1;
        }
        idx += 1;
    }
    None
}

fn golangci_global_flag_takes_value(arg: &str) -> bool {
    let flag = arg.split_once('=').map(|(flag, _)| flag).unwrap_or(arg);
    if arg.starts_with("--") && arg.contains('=') {
        return false;
    }
    matches!(
        flag,
        "-c" | "--color"
            | "--config"
            | "--cpu-profile-path"
            | "--mem-profile-path"
            | "--trace-path"
    )
}

fn parse_sed_range(value: &str) -> Option<(usize, usize)> {
    let trimmed = value.trim().trim_end_matches('p');
    let (start, end) = trimmed.split_once(',')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

fn build_reducer_rewrite(
    root: &Path,
    task_id: &str,
    session_id: Option<&str>,
    cwd: &str,
    spec: &CommandReducerSpec,
    argv: &[String],
    env_assignments: &[(String, String)],
) -> String {
    let exe = current_exe();
    let mut parts = vec![
        shell_quote(&exe),
        "hook".to_string(),
        "reducer-runner".to_string(),
        "--root".to_string(),
        shell_quote(&root.display().to_string()),
        "--task-id".to_string(),
        shell_quote(task_id),
        "--family".to_string(),
        shell_quote(&spec.family),
        "--kind".to_string(),
        shell_quote(&spec.canonical_kind),
        "--fingerprint".to_string(),
        shell_quote(&spec.cache_fingerprint),
    ];
    if let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) {
        parts.push("--session-id".to_string());
        parts.push(shell_quote(session_id));
    }
    for (key, value) in env_assignments {
        parts.push("--env".to_string());
        parts.push(shell_quote(&format!("{key}={value}")));
    }
    parts.push("--cwd".to_string());
    parts.push(shell_quote(cwd));
    parts.push("--".to_string());
    parts.extend(argv.iter().map(|arg| shell_quote(arg)));
    parts.join(" ")
}

fn build_proxy_rewrite(root: &Path, task_id: &str, cwd: &str, argv: &[String]) -> String {
    let exe = current_exe();
    let mut parts = vec![
        shell_quote(&exe),
        "--via-daemon".to_string(),
        "--daemon-root".to_string(),
        shell_quote(&root.display().to_string()),
        "proxy".to_string(),
        "run".to_string(),
        "--task-id".to_string(),
        shell_quote(task_id),
        "--cwd".to_string(),
        shell_quote(cwd),
        "--".to_string(),
    ];
    parts.extend(argv.iter().map(|arg| shell_quote(arg)));
    parts.join(" ")
}

fn build_toml_filter_rewrite(
    root: &Path,
    _cwd: &str,
    argv: &[String],
    env_assignments: &[(String, String)],
) -> String {
    let exe = current_exe();
    let mut parts = Vec::new();
    for (key, value) in env_assignments {
        parts.push(shell_quote(&format!("{key}={value}")));
    }
    parts.extend([
        shell_quote(&exe),
        "run".to_string(),
        "--root".to_string(),
        shell_quote(&root.display().to_string()),
        "--".to_string(),
    ]);
    parts.extend(argv.iter().map(|arg| shell_quote(arg)));
    parts.join(" ")
}

fn build_compound_rewrite(
    root: &Path,
    task_id: &str,
    session_id: Option<&str>,
    cwd: &str,
    command: &str,
) -> Option<String> {
    let parts = split_compound_command(command)?;
    let cwd_path = Path::new(cwd);
    let mut previous_operator_allows_rewrite = true;
    let mut changed = false;
    let mut out = Vec::with_capacity(parts.len());
    for (idx, part) in parts.iter().enumerate() {
        match part {
            CompoundPart::Operator(op) => {
                previous_operator_allows_rewrite = *op != "|";
                out.push(op.to_string());
            }
            CompoundPart::Segment(segment) => {
                let stdout_feeds_pipe =
                    matches!(parts.get(idx + 1), Some(CompoundPart::Operator("|")));
                let rewritten = if previous_operator_allows_rewrite && !stdout_feeds_pipe {
                    let decision =
                        decide_command_route_inner(segment, Some(cwd_path), Some(root), false);
                    build_route_rewrite(root, task_id, session_id, cwd, &decision)
                } else {
                    None
                };
                if let Some(rewritten) = rewritten {
                    changed = true;
                    out.push(rewritten);
                } else {
                    out.push(segment.trim().to_string());
                }
            }
        }
    }
    changed.then(|| join_compound_parts(&out))
}

fn current_exe() -> String {
    std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "Packet28".to_string())
}

fn looks_like_env_assignment(arg: &str) -> bool {
    arg.contains('=')
        && !arg.starts_with('=')
        && arg
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
}

fn contains_shell_composition(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut idx = 0usize;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if ch == '\'' && !in_double {
            in_single = !in_single;
        } else if ch == '"' && !in_single {
            in_double = !in_double;
        } else if !in_single && !in_double {
            if matches!(ch, '|' | ';' | '<' | '>' | '`' | '&') {
                return true;
            }
            if ch == '$' && bytes.get(idx + 1).is_some_and(|next| *next == b'(') {
                return true;
            }
        }
        idx += 1;
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompoundPart<'a> {
    Segment(&'a str),
    Operator(&'a str),
}

fn compound_has_rewritable_segment(
    command: &str,
    cwd: Option<&Path>,
    policy_root: Option<&Path>,
) -> bool {
    let Some(parts) = split_compound_command(command) else {
        return false;
    };
    let mut previous_operator_allows_rewrite = true;
    for (idx, part) in parts.iter().enumerate() {
        match part {
            CompoundPart::Operator(op) => previous_operator_allows_rewrite = *op != "|",
            CompoundPart::Segment(segment) => {
                let stdout_feeds_pipe =
                    matches!(parts.get(idx + 1), Some(CompoundPart::Operator("|")));
                if !previous_operator_allows_rewrite || stdout_feeds_pipe {
                    continue;
                }
                let decision = decide_command_route_inner(segment, cwd, policy_root, false);
                if decision.kind != RouteKind::RawPassthrough {
                    return true;
                }
            }
        }
    }
    false
}

fn split_compound_command(command: &str) -> Option<Vec<CompoundPart<'_>>> {
    let bytes = command.as_bytes();
    let mut parts = Vec::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut segment_start = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if ch == '\'' && !in_double {
            in_single = !in_single;
            idx += 1;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            idx += 1;
            continue;
        }
        if !in_single && !in_double {
            let op = if bytes.get(idx) == Some(&b'&') && bytes.get(idx + 1) == Some(&b'&') {
                Some(("&&", 2usize))
            } else if bytes.get(idx) == Some(&b'|') && bytes.get(idx + 1) == Some(&b'|') {
                Some(("||", 2usize))
            } else if bytes.get(idx) == Some(&b'|') {
                Some(("|", 1usize))
            } else if bytes.get(idx) == Some(&b';') {
                Some((";", 1usize))
            } else {
                None
            };
            if let Some((op, len)) = op {
                let segment = command[segment_start..idx].trim();
                if segment.is_empty() {
                    return None;
                }
                parts.push(CompoundPart::Segment(segment));
                parts.push(CompoundPart::Operator(op));
                idx += len;
                segment_start = idx;
                continue;
            }
            if matches!(ch, '<' | '>' | '`') || (ch == '&' && bytes.get(idx + 1) != Some(&b'&')) {
                return None;
            }
        }
        idx += 1;
    }
    let segment = command[segment_start..].trim();
    if segment.is_empty() || parts.is_empty() {
        return None;
    }
    parts.push(CompoundPart::Segment(segment));
    Some(parts)
}

fn join_compound_parts(parts: &[String]) -> String {
    let mut out = String::new();
    for (idx, part) in parts.iter().enumerate() {
        if idx > 0 {
            if part == ";" {
                // no leading space before semicolon
            } else {
                out.push(' ');
            }
        }
        out.push_str(part);
        if part == ";" && idx + 1 < parts.len() {
            out.push(' ');
        }
    }
    out
}

fn strip_supported_postprocess(command: &str) -> (String, Option<Postprocess>) {
    if std::env::var_os("PACKET28_ENABLE_PIPE_POSTPROCESS_REWRITE").is_none() {
        return (command.trim().to_string(), None);
    }
    let Some(pipe_idx) = find_last_top_level_pipe(command) else {
        return (command.trim().to_string(), None);
    };
    let lhs = command[..pipe_idx].trim();
    let rhs = command[pipe_idx + 1..].trim();
    match parse_supported_postprocess(rhs) {
        Some(postprocess) => (lhs.to_string(), Some(postprocess)),
        None => (command.trim().to_string(), None),
    }
}

fn find_last_top_level_pipe(command: &str) -> Option<usize> {
    let bytes = command.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut last_pipe = None;
    let mut idx = 0usize;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if ch == '\'' && !in_double {
            in_single = !in_single;
        } else if ch == '"' && !in_single {
            in_double = !in_double;
        } else if ch == '|'
            && !in_single
            && !in_double
            && bytes.get(idx + 1).map(|next| *next != b'|').unwrap_or(true)
            && idx > 0
            && bytes[idx - 1] != b'|'
        {
            last_pipe = Some(idx);
        }
        idx += 1;
    }
    last_pipe
}

fn parse_supported_postprocess(command: &str) -> Option<Postprocess> {
    let argv = shell_words::split(command).ok()?;
    match argv.first()?.as_str() {
        "head" => parse_head_postprocess(&argv),
        "tail" => parse_tail_postprocess(&argv),
        "sed" => parse_sed_postprocess(&argv),
        _ => None,
    }
}

fn parse_head_postprocess(argv: &[String]) -> Option<Postprocess> {
    Some(Postprocess::Head(parse_count_flag(
        argv.iter().skip(1).cloned().collect(),
    )?))
}

fn parse_tail_postprocess(argv: &[String]) -> Option<Postprocess> {
    Some(Postprocess::Tail(parse_count_flag(
        argv.iter().skip(1).cloned().collect(),
    )?))
}

fn parse_sed_postprocess(argv: &[String]) -> Option<Postprocess> {
    if argv.len() != 3 || argv.get(1).map(String::as_str) != Some("-n") {
        return None;
    }
    let (start, end) = parse_sed_range(&argv[2])?;
    Some(Postprocess::SedRange(start, end))
}

fn parse_count_flag(argv: Vec<String>) -> Option<usize> {
    let mut count = 10usize;
    let mut iter = argv.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-n" => count = iter.next()?.parse().ok()?,
            value
                if value.starts_with('-')
                    && value.len() > 1
                    && value[1..].chars().all(|ch| ch.is_ascii_digit()) =>
            {
                count = value[1..].parse().ok()?;
            }
            value if value.starts_with('-') => {}
            _ => return None,
        }
    }
    Some(count)
}

fn strip_supported_trailing_redirects(command: &str) -> String {
    let mut trimmed = command.trim().to_string();
    loop {
        let Some(stripped) = strip_one_trailing_redirect(&trimmed) else {
            break;
        };
        trimmed = stripped.trim_end().to_string();
    }
    trimmed
}

fn strip_one_trailing_redirect(command: &str) -> Option<&str> {
    const REDIRECT_SUFFIXES: &[&str] = &[
        "2>&1",
        "1>&2",
        ">/dev/null",
        "1>/dev/null",
        "2>/dev/null",
        "1> /dev/null",
        "2> /dev/null",
        "> /dev/null",
    ];
    REDIRECT_SUFFIXES
        .iter()
        .find_map(|suffix| command.strip_suffix(suffix))
}

fn contains_disallowed_shell_expansion(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut idx = 0usize;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if escaped {
            escaped = false;
            idx += 1;
            continue;
        }
        match ch {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '$' if !in_single => return true,
            '~' if !in_single && !in_double && idx == 0 => return true,
            '{' if !in_single && !in_double => return true,
            _ => {}
        }
        idx += 1;
    }
    false
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '='))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_rewrite_output_safe(command: &str) -> bool {
    command.chars().all(|ch| ch == '\t' || ch >= ' ')
}

#[cfg(test)]
#[path = "route_registry_tests.rs"]
mod tests;
