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
    let sanitized = strip_supported_trailing_redirects(command.trim());
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
    out.split_whitespace().collect::<Vec<_>>().join(" ")
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
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn routes_env_prefixed_reducer_commands() {
        let decision = decide_command_route("FOO=1 cargo test");
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        assert_eq!(decision.env_assignments.len(), 1);
    }

    #[test]
    fn routes_multiple_env_prefixed_reducer_commands() {
        let decision = decide_command_route("RUST_BACKTRACE=1 CARGO_TERM_COLOR=never cargo check");
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        assert_eq!(
            decision.env_assignments,
            vec![
                ("RUST_BACKTRACE".to_string(), "1".to_string()),
                ("CARGO_TERM_COLOR".to_string(), "never".to_string())
            ]
        );
    }

    #[test]
    fn disabled_env_prefix_bypasses_rewrite() {
        let decision = decide_command_route("PACKET28_DISABLED=1 git status --short");
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("rewrite_disabled"));
    }

    #[test]
    fn rtk_disabled_env_prefix_bypasses_rewrite_for_compatibility() {
        let decision = decide_command_route("FOO=1 RTK_DISABLED=true cargo test");
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("rewrite_disabled"));
    }

    #[test]
    fn disabled_env_prefix_false_value_still_routes() {
        let decision = decide_command_route("PACKET28_DISABLED=0 git status --short");
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    }

    #[test]
    fn routes_simple_reads_to_native_tool() {
        let decision = decide_command_route("head -n 20 README.md");
        assert_eq!(decision.kind, RouteKind::NativeTool);
    }

    #[test]
    fn routes_cat_line_numbers_like_rtk_and_declines_incompatible_flags() {
        let decision = decide_command_route("cat -n README.md");
        assert_eq!(decision.kind, RouteKind::NativeTool);
        assert_eq!(
            decision.native_tool.as_ref().map(|tool| tool.argv.clone()),
            Some(vec![
                "compact".to_string(),
                "read".to_string(),
                "README.md".to_string()
            ])
        );

        for command in ["cat -A README.md", "cat --show-all README.md"] {
            let decision = decide_command_route(command);
            assert_eq!(decision.kind, RouteKind::RawPassthrough, "{command}");
            assert_eq!(
                decision.reason.as_deref(),
                Some("unsupported_command"),
                "{command}"
            );
        }
    }

    #[test]
    fn routes_tree_command_to_native_tool() {
        let decision = decide_command_route("tree -L 2 crates");
        assert_eq!(decision.kind, RouteKind::NativeTool);
    }

    #[test]
    fn tolerates_trailing_redirects_for_reducer_routes() {
        let decision = decide_command_route("FOO=1 cargo test -q 2>&1");
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        assert_eq!(decision.env_assignments.len(), 1);
    }

    #[test]
    fn declines_reducer_commands_with_truncation_pipe() {
        let decision = decide_command_route("git show HEAD | head -n 20");
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
    }

    #[test]
    fn routes_supported_compound_commands() {
        let decision = decide_command_route("cargo check && cargo test");
        assert_eq!(decision.kind, RouteKind::CompoundRewrite);
        assert_eq!(decision.reason.as_deref(), Some("compound_rewrite"));
    }

    #[test]
    fn rejects_unsupported_heredoc_commands() {
        let decision = decide_command_route("cat <<EOF\nhello\nEOF");
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
    }

    #[test]
    fn declines_pipe_commands_with_supported_left_side() {
        let decision = decide_command_route("git status --short | wc -l");
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
    }

    #[test]
    fn builds_compound_rewrite_for_supported_and_mixed_segments() {
        let tmp = tempfile::tempdir().unwrap();
        let decision = decide_command_route_with_cwd_and_root(
            "cargo test && htop || git status --short",
            tmp.path(),
            tmp.path(),
        );
        let rewritten =
            build_route_rewrite(tmp.path(), "task-compound", None, ".", &decision).unwrap();
        assert!(rewritten.contains("hook reducer-runner"));
        assert!(rewritten.contains(" cargo test"));
        assert!(rewritten.contains("&& htop ||"));
        assert!(rewritten.contains(" git status --short"));
    }

    #[test]
    fn builds_pipe_rewrite_for_left_side_only_then_resumes_after_chain_operator() {
        let tmp = tempfile::tempdir().unwrap();
        let decision = decide_command_route_with_cwd_and_root(
            "cargo test | grep FAIL && git status --short",
            tmp.path(),
            tmp.path(),
        );
        let rewritten = build_route_rewrite(tmp.path(), "task-pipe", None, ".", &decision).unwrap();
        assert!(rewritten.contains("cargo test | grep FAIL &&"));
        assert!(rewritten.contains(" git status --short"));
        assert_eq!(rewritten.matches("hook reducer-runner").count(), 1);
    }

    #[test]
    fn reducer_rewrite_output_does_not_contain_control_characters() {
        let tmp = tempfile::tempdir().unwrap();
        let decision =
            decide_command_route_with_cwd_and_root("git status --short", tmp.path(), tmp.path());
        let rewritten =
            build_route_rewrite(tmp.path(), "task-safe-output", None, ".", &decision).unwrap();
        assert!(
            rewritten.chars().all(|ch| ch == '\t' || ch >= ' '),
            "{rewritten:?}"
        );
    }

    #[test]
    fn leaves_already_packet28_commands_alone() {
        let decision = decide_command_route("Packet28 run git status --short");
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("already_packet28"));
    }

    #[test]
    fn marks_rtk_ignored_commands_as_intentional_passthrough() {
        for command in [
            "cd crates/suite-cli",
            "echo hello",
            "mkdir -p target/tmp",
            "python -c 'print(1)'",
            "node -e 'console.log(1)'",
            "pwd",
        ] {
            let decision = decide_command_route(command);
            assert_eq!(decision.kind, RouteKind::RawPassthrough, "{command}");
            assert_eq!(
                decision.reason.as_deref(),
                Some("ignored_command"),
                "{command}"
            );
        }
    }

    #[test]
    fn routes_git_global_options_to_reducer() {
        let decision = decide_command_route("git -C crates/suite-cli status --short");
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        assert_eq!(
            decision
                .reducer_spec
                .as_ref()
                .map(|spec| spec.canonical_kind.as_str()),
            Some("git_status")
        );
    }

    #[test]
    fn routes_golangci_global_options_before_run_like_rtk() {
        for command in [
            "golangci-lint -v run ./...",
            "golangci-lint --color never run ./...",
            "golangci-lint --color=never run ./...",
            "golangci-lint --config=.golangci.yml run ./...",
            "golangci run ./...",
        ] {
            let decision = decide_command_route(command);
            assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
            assert_eq!(
                decision
                    .reducer_spec
                    .as_ref()
                    .map(|spec| spec.canonical_kind.as_str()),
                Some("golangci_lint"),
                "{command}"
            );
        }

        let bare = decide_command_route("golangci-lint version");
        assert_eq!(bare.kind, RouteKind::RawPassthrough);
    }

    #[test]
    fn routes_rtk_javascript_package_runner_prefixes() {
        for (command, expected) in [
            ("npm exec tsc --noEmit", "javascript_tsc"),
            ("npm x eslint src", "javascript_eslint"),
            ("npm rum biome check .", "javascript_lint"),
            ("npm run test", "javascript_test"),
            ("npm run-script biome check .", "javascript_lint"),
            ("npm urn jest run", "javascript_test"),
            ("pnpm dlx next build", "javascript_next_build"),
            ("pnpm exec prisma migrate status", "javascript_prisma"),
            ("pnpx vitest run", "javascript_vitest"),
            ("npx jest run", "javascript_test"),
            ("npx playwright test", "javascript_test"),
            ("npx svgo", "javascript_npx"),
        ] {
            let decision = decide_command_route(command);
            assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
            assert_eq!(
                decision
                    .reducer_spec
                    .as_ref()
                    .map(|spec| spec.canonical_kind.as_str()),
                Some(expected),
                "{command}"
            );
        }

        let npx = decide_command_route("npx svgo");
        assert!(npx.reducer_spec.as_ref().unwrap().mutation);
    }

    #[test]
    fn routes_rtk_python_package_manager_forms() {
        for (command, expected) in [
            ("python3.11 -m pytest tests", "python_pytest"),
            ("pip3 list", "python_pip_list"),
            ("pip outdated", "python_pip_outdated"),
            ("pip install pytest", "python_pip_install"),
            ("pip show pytest", "python_pip_show"),
            ("uv pip outdated", "python_pip_outdated"),
            ("uv pip install pytest", "python_pip_install"),
            ("uv pip show pytest", "python_pip_show"),
        ] {
            let decision = decide_command_route(command);
            assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
            assert_eq!(
                decision
                    .reducer_spec
                    .as_ref()
                    .map(|spec| spec.canonical_kind.as_str()),
                Some(expected),
                "{command}"
            );
        }
    }

    #[test]
    fn routes_rtk_ruff_format_as_mutation() {
        let decision = decide_command_route("ruff format src");
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        let spec = decision.reducer_spec.as_ref().unwrap();
        assert_eq!(spec.canonical_kind, "python_ruff_format");
        assert!(spec.mutation);

        let diff = decide_command_route("ruff format --diff src");
        assert_eq!(diff.kind, RouteKind::RawPassthrough);
    }

    #[test]
    fn routes_rtk_docker_and_kubectl_mutation_forms() {
        for (command, expected) in [
            ("docker build .", "docker_build"),
            ("docker compose build", "docker_compose_build"),
            ("docker run alpine echo hi", "docker_run"),
            ("docker exec app ls", "docker_exec"),
            ("kubectl apply -f deploy.yaml", "kubectl_apply"),
        ] {
            let decision = decide_command_route(command);
            assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
            let spec = decision.reducer_spec.as_ref().unwrap();
            assert_eq!(spec.canonical_kind, expected, "{command}");
            assert!(spec.mutation, "{command}");
        }
    }

    #[test]
    fn routes_rtk_cargo_install_as_mutation() {
        let decision = decide_command_route("cargo install ripgrep");
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        let spec = decision.reducer_spec.as_ref().unwrap();
        assert_eq!(spec.canonical_kind, "rust_install");
        assert!(spec.mutation);
    }

    #[test]
    fn routes_rtk_cargo_fmt_as_mutation() {
        let decision = decide_command_route("cargo fmt --all");
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        let spec = decision.reducer_spec.as_ref().unwrap();
        assert_eq!(spec.canonical_kind, "rust_fmt");
        assert!(spec.mutation);

        let check = decide_command_route("cargo fmt --check");
        assert_eq!(check.kind, RouteKind::ReducerRewrite);
        let spec = check.reducer_spec.as_ref().unwrap();
        assert_eq!(spec.canonical_kind, "rust_fmt");
        assert!(!spec.mutation);
    }

    #[test]
    fn routes_rtk_github_release_and_glab_ci_forms() {
        for (command, expected) in [
            ("gh release list", "gh_release_list"),
            ("glab ci status", "glab_ci_status"),
        ] {
            let decision = decide_command_route(command);
            assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
            let spec = decision.reducer_spec.as_ref().unwrap();
            assert_eq!(spec.canonical_kind, expected, "{command}");
            assert!(!spec.mutation, "{command}");
        }
    }

    #[test]
    fn routes_rtk_psql_connection_forms() {
        for command in ["psql -U postgres -d mydb", "psql postgres://localhost/mydb"] {
            let decision = decide_command_route(command);
            assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
            let spec = decision.reducer_spec.as_ref().unwrap();
            assert_eq!(spec.canonical_kind, "psql_query", "{command}");
            assert!(!spec.mutation, "{command}");
        }

        let exact_output = decide_command_route("psql -o out.txt -U postgres");
        assert_eq!(exact_output.kind, RouteKind::RawPassthrough);
    }

    #[test]
    fn routes_yadm_like_rtk_git_alias() {
        for (command, expected) in [
            ("yadm status --short", "git_status"),
            ("yadm diff", "git_diff"),
            ("yadm log --oneline", "git_log"),
        ] {
            let decision = decide_command_route(command);
            assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
            assert_eq!(
                decision
                    .reducer_spec
                    .as_ref()
                    .map(|spec| spec.canonical_kind.as_str()),
                Some(expected),
                "{command}"
            );
        }
    }

    #[test]
    fn routes_every_upstream_rtk_rule_representative() {
        let tmp = tempfile::tempdir().unwrap();
        for command in [
            "git status --short",
            "yadm status --short",
            "gh pr list",
            "gh release list",
            "glab mr list",
            "glab ci status",
            "cargo build",
            "cargo fmt --all",
            "cargo install ripgrep",
            "pnpm install",
            "npm run test",
            "npm rum lint",
            "npm urn jest run",
            "npx svgo",
            "cat README.md",
            "head -n 5 README.md",
            "tail -n 5 README.md",
            "rg packet README.md",
            "grep packet README.md",
            "ls -la",
            "find . -maxdepth 1 -type f",
            "tsc --noEmit",
            "eslint src",
            "biome check .",
            "prettier --check README.md",
            "next build",
            "jest run",
            "vitest run",
            "playwright test",
            "prisma migrate status",
            "docker ps",
            "docker compose ps",
            "docker build .",
            "kubectl get pods",
            "kubectl apply -f deploy.yaml",
            "tree -L 2",
            "diff old.txt new.txt",
            "curl https://example.com",
            "wget https://example.com",
            "python3 -m mypy src",
            "ruff check src",
            "ruff format src",
            "python3.11 -m pytest tests",
            "pip3 list",
            "uv pip install pytest",
            "go test ./...",
            "golangci-lint run ./...",
            "bundle install",
            "bundle exec rails test",
            "bundle exec rspec",
            "bundle exec rubocop",
            "aws sts get-caller-identity",
            "psql postgres://localhost/db",
            "ansible-playbook site.yml",
            "brew install ripgrep",
            "composer install",
            "df -h",
            "dotnet build",
            "du -sh .",
            "fail2ban-client status sshd",
            "gcloud container clusters list",
            "gradle test",
            "./gradlew build",
            "hadolint Dockerfile",
            "helm status app",
            "iptables -L",
            "make test",
            "markdownlint README.md",
            "mix compile",
            "mix format",
            "mvn package",
            "ping example.com",
            "pio run",
            "poetry install",
            "pre-commit run --all-files",
            "ps aux",
            "quarto render index.qmd",
            "rsync -a src remote:",
            "shellcheck script.sh",
            "shopify theme push",
            "sops -d secrets.yaml",
            "swift test",
            "systemctl status nginx",
            "terraform plan",
            "tofu validate",
            "trunk build",
            "uv sync",
            "yamllint config.yml",
            "wc -l README.md",
            "gt log",
            "liquibase update",
        ] {
            let decision = decide_command_route_with_cwd_and_root(command, tmp.path(), tmp.path());
            assert!(
                matches!(
                    decision.kind,
                    RouteKind::ReducerRewrite
                        | RouteKind::NativeTool
                        | RouteKind::TomlFilterRewrite
                ),
                "{command} routed as {:?} ({:?})",
                decision.kind,
                decision.reason
            );
        }
    }

    #[test]
    fn routes_sudo_and_env_prefixed_commands_like_rtk() {
        let sudo = decide_command_route("sudo git status --short");
        assert_eq!(sudo.kind, RouteKind::ReducerRewrite);
        assert_eq!(
            sudo.reducer_spec
                .as_ref()
                .map(|spec| spec.canonical_kind.as_str()),
            Some("git_status")
        );
        assert_eq!(sudo.argv.first().map(String::as_str), Some("git"));
        assert_eq!(sudo.wrapper_prefix, vec!["sudo".to_string()]);

        let env = decide_command_route("env RUST_BACKTRACE=1 cargo test");
        assert_eq!(env.kind, RouteKind::ReducerRewrite);
        assert_eq!(
            env.reducer_spec
                .as_ref()
                .map(|spec| spec.canonical_kind.as_str()),
            Some("rust_test")
        );
        assert_eq!(
            env.env_assignments,
            vec![("RUST_BACKTRACE".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn routes_rtk_shell_builtin_prefixes_like_rtk() {
        for prefix in ["noglob", "command", "builtin", "exec", "nocorrect"] {
            let command = format!("{prefix} git status");
            let decision = decide_command_route(&command);
            assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
            assert_eq!(
                decision.wrapper_prefix,
                vec![prefix.to_string()],
                "{command}"
            );
            assert_eq!(
                decision
                    .reducer_spec
                    .as_ref()
                    .map(|spec| spec.canonical_kind.as_str()),
                Some("git_status"),
                "{command}"
            );
        }

        let unknown = decide_command_route("noglob unknown_cmd --flag");
        assert_eq!(unknown.kind, RouteKind::RawPassthrough);
    }

    #[test]
    fn builds_rewrite_with_runtime_wrapper_prefixes() {
        let decision = decide_command_route("sudo noglob git status");
        assert_eq!(
            decision.wrapper_prefix,
            vec!["sudo".to_string(), "noglob".to_string()]
        );
        let rewritten =
            build_route_rewrite(Path::new("/workspace"), "task-prefix", None, ".", &decision)
                .unwrap();
        assert!(rewritten.starts_with("sudo noglob "));
        assert!(rewritten.contains("hook reducer-runner"));
    }

    #[test]
    fn normalizes_absolute_binary_paths_like_rtk() {
        let decision = decide_command_route("/usr/bin/git status --short");
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        assert_eq!(
            decision
                .reducer_spec
                .as_ref()
                .map(|spec| spec.canonical_kind.as_str()),
            Some("git_status")
        );
        assert_eq!(decision.argv.first().map(String::as_str), Some("git"));
    }

    #[test]
    fn repo_config_excludes_commands_from_rewrite() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("covy.toml"),
            "[packet28.rewrite]\nexclude_commands = [\"git\"]\n",
        )
        .unwrap();
        let decision =
            decide_command_route_with_cwd_and_root("git status --short", tmp.path(), tmp.path());
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("config_excluded"));
    }

    #[test]
    fn hooks_config_excludes_commands_from_rewrite() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("covy.toml"),
            "[hooks]\nexclude_commands = [\"cargo\"]\n",
        )
        .unwrap();
        let decision = decide_command_route_with_cwd_and_root("cargo test", tmp.path(), tmp.path());
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("config_excluded"));
    }

    #[test]
    fn rtk_global_config_excludes_subcommand_patterns_from_rewrite() {
        let _guard = env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let config_home = tmp.path().join("config-home");
        fs::create_dir_all(config_home.join("rtk")).unwrap();
        fs::write(
            config_home.join("rtk").join("config.toml"),
            "[hooks]\nexclude_commands = [\"git push\"]\n",
        )
        .unwrap();
        let previous = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &config_home);

        let decision =
            decide_command_route_with_cwd_and_root("git push origin main", tmp.path(), tmp.path());

        if let Some(previous) = previous {
            std::env::set_var("XDG_CONFIG_HOME", previous);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("config_excluded"));
    }

    #[test]
    fn rtk_global_config_excludes_regex_patterns_from_rewrite() {
        let _guard = env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let config_home = tmp.path().join("config-home");
        fs::create_dir_all(config_home.join("rtk")).unwrap();
        fs::write(
            config_home.join("rtk").join("config.toml"),
            "[hooks]\nexclude_commands = [\"^git (push|tag)\"]\n",
        )
        .unwrap();
        let previous = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &config_home);

        let decision =
            decide_command_route_with_cwd_and_root("git tag v1.2.3", tmp.path(), tmp.path());

        if let Some(previous) = previous {
            std::env::set_var("XDG_CONFIG_HOME", previous);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("config_excluded"));
    }

    #[test]
    fn builtin_toml_filters_route_to_packet28_run() {
        let tmp = tempfile::tempdir().unwrap();
        let decision =
            decide_command_route_with_cwd_and_root("brew install rtk", tmp.path(), tmp.path());
        assert_eq!(decision.kind, RouteKind::TomlFilterRewrite);
        assert_eq!(decision.reason.as_deref(), Some("toml_filter"));

        let rewritten =
            build_route_rewrite(tmp.path(), "task-filter", None, ".", &decision).unwrap();
        assert!(rewritten.contains(" run --root "));
        assert!(rewritten.ends_with(" -- brew install rtk"));

        let swift = decide_command_route_with_cwd_and_root("swift test", tmp.path(), tmp.path());
        assert_eq!(swift.kind, RouteKind::TomlFilterRewrite);
        assert_eq!(swift.reason.as_deref(), Some("toml_filter"));

        let diff =
            decide_command_route_with_cwd_and_root("diff old.txt new.txt", tmp.path(), tmp.path());
        assert_eq!(diff.kind, RouteKind::ReducerRewrite);
        assert_eq!(
            diff.reducer_spec
                .as_ref()
                .map(|spec| spec.canonical_kind.as_str()),
            Some("fs_diff")
        );
    }

    #[test]
    fn repo_config_transparent_prefix_routes_wrapped_command() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("covy.toml"),
            "[packet28.rewrite]\ntransparent_prefixes = [\"poetry run\"]\n",
        )
        .unwrap();
        let decision =
            decide_command_route_with_cwd_and_root("poetry run pytest -q", tmp.path(), tmp.path());
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        assert_eq!(
            decision.wrapper_prefix,
            vec!["poetry".to_string(), "run".to_string()]
        );
        let rewritten =
            build_route_rewrite(tmp.path(), "task-prefix", None, ".", &decision).unwrap();
        assert!(rewritten.starts_with("poetry run "));
        assert!(rewritten.contains("hook reducer-runner"));
    }

    #[test]
    fn declines_grep_with_head_pipe_to_preserve_stream_semantics() {
        let decision = decide_command_route("rg task_id crates/suite-cli/src | head -n 5");
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
    }

    #[test]
    fn declines_tree_with_head_pipe_to_preserve_stream_semantics() {
        let decision = decide_command_route("tree -L 2 crates | head -n 10");
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
    }

    #[test]
    fn declines_cat_with_sed_pipe_to_preserve_stream_semantics() {
        let decision = decide_command_route("cat Cargo.toml | sed -n '1,20p'");
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
    }

    #[test]
    fn routes_tree_glob_to_native_tool() {
        let decision = decide_command_route("tree crates/*");
        assert_eq!(decision.kind, RouteKind::NativeTool);
    }

    #[test]
    fn routes_grep_glob_path_to_native_tool() {
        let decision = decide_command_route("rg task_id crates/**/*.rs");
        assert_eq!(decision.kind, RouteKind::NativeTool);
    }

    #[test]
    fn declines_extraction_mode_grep_rewrites() {
        for command in [
            "grep -o 'BUG-[0-9]*' bug.md",
            "grep -c BUG bug.md",
            "grep -l BUG bug.md",
            "grep -q BUG bug.md",
            "rg --files-with-matches BUG crates",
            "rg -oc BUG crates",
        ] {
            let decision = decide_command_route(command);
            assert_eq!(decision.kind, RouteKind::RawPassthrough, "{command}");
        }
    }

    #[test]
    fn declines_read_glob_without_shell_expansion() {
        let decision = decide_command_route("cat crates/*/Cargo.toml");
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("shell_glob"));
    }

    #[test]
    fn routes_git_diff_glob_with_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        fs::create_dir_all(cwd.join("src")).unwrap();
        fs::write(cwd.join("src/a.rs"), "fn a() {}").unwrap();
        fs::write(cwd.join("src/b.rs"), "fn b() {}").unwrap();
        let decision = decide_command_route_with_cwd("git diff src/*.rs", cwd);
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        assert!(decision.argv.contains(&"src/a.rs".to_string()));
        assert!(decision.argv.contains(&"src/b.rs".to_string()));
        assert_eq!(
            decision.original_argv,
            Some(vec![
                "git".to_string(),
                "diff".to_string(),
                "src/*.rs".to_string()
            ])
        );
    }

    #[test]
    fn glob_expansion_preserves_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        fs::create_dir_all(cwd.join("src")).unwrap();
        fs::write(cwd.join("src/x.rs"), "fn x() {}").unwrap();
        let decision = decide_command_route_with_cwd("git diff --cached src/*.rs", cwd);
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        assert!(decision.argv.contains(&"--cached".to_string()));
        assert!(decision.argv.contains(&"src/x.rs".to_string()));
    }

    #[test]
    fn glob_no_matches_keeps_original() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let decision = decide_command_route_with_cwd("git diff nonexistent/*.rs", cwd);
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("shell_glob"));
    }

    #[test]
    fn glob_without_cwd_still_rejects() {
        let decision = decide_command_route("git diff src/*.rs");
        assert_eq!(decision.kind, RouteKind::RawPassthrough);
        assert_eq!(decision.reason.as_deref(), Some("shell_glob"));
    }

    #[test]
    fn cargo_test_glob_expansion() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        fs::create_dir_all(cwd.join("tests")).unwrap();
        fs::write(cwd.join("tests/test_foo.rs"), "#[test] fn t() {}").unwrap();
        fs::write(
            cwd.join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\n",
        )
        .unwrap();
        let decision = decide_command_route_with_cwd("cargo test tests/test_*.rs", cwd);
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        assert!(decision.argv.contains(&"tests/test_foo.rs".to_string()));
    }
}
