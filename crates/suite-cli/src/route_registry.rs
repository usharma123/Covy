use std::path::Path;

use packet28_reducer_core::{classify_command_argv, CommandReducerSpec};

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Postprocess {
    Head(usize),
    Tail(usize),
    SedRange(usize, usize),
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
    if argv.is_empty() {
        return raw_passthrough("empty_command");
    }
    strip_supported_runtime_prefixes(&mut argv, &mut env_assignments);
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
    if policy_excludes_command(trimmed, &argv, policy_root) {
        return raw_passthrough("config_excluded");
    }
    let wrapper_prefix = configured_wrapper_prefix(&argv, policy_root).unwrap_or_default();
    if !wrapper_prefix.is_empty() {
        argv = argv[wrapper_prefix.len()..].to_vec();
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
    Some(apply_wrapper_prefix(&decision.wrapper_prefix, rewritten))
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
) {
    loop {
        if argv.first().is_some_and(|arg| arg == "sudo") && argv.len() > 1 {
            argv.remove(0);
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

fn classify_native_tool(argv: &[String]) -> Option<NativeToolPlan> {
    match argv.first()?.as_str() {
        "ls" | "find" | "tree" => classify_tree_tool(argv),
        "cat" | "head" | "tail" | "sed" => classify_read_tool(argv),
        "grep" | "rg" => classify_grep_tool(argv),
        "env" | "printenv" => classify_env_tool(argv),
        _ => None,
    }
}

fn is_packet28_invocation(argv: &[String]) -> bool {
    argv.first()
        .and_then(|arg| Path::new(arg).file_name())
        .and_then(|name| name.to_str())
        .map(|name| matches!(name, "Packet28" | "packet28" | "covy"))
        .unwrap_or(false)
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

fn policy_excludes_command(command: &str, argv: &[String], root: Option<&Path>) -> bool {
    let Some(root) = root else {
        return false;
    };
    let excludes = load_policy_excludes(root);
    if excludes.is_empty() {
        return false;
    }
    let base = argv
        .first()
        .and_then(|arg| Path::new(arg).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    excludes.iter().any(|exclude| {
        let exclude = exclude.trim();
        if exclude.is_empty() {
            return false;
        }
        if exclude.starts_with('^') {
            return regex::Regex::new(exclude)
                .map(|regex| regex.is_match(command))
                .unwrap_or(false);
        }
        base == exclude
            || argv.first().map(String::as_str) == Some(exclude)
            || command == exclude
            || command.starts_with(&format!("{exclude} "))
    })
}

fn load_policy_excludes(root: &Path) -> Vec<String> {
    let mut excludes = Vec::new();
    for path in policy_config_paths(root) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&content) else {
            continue;
        };
        excludes.extend(extract_policy_excludes(&value));
    }
    excludes
}

fn configured_wrapper_prefix(argv: &[String], root: Option<&Path>) -> Option<Vec<String>> {
    let root = root?;
    load_policy_prefixes(root)
        .into_iter()
        .filter_map(|prefix| shell_words::split(&prefix).ok())
        .filter(|prefix| !prefix.is_empty() && argv.starts_with(prefix))
        .max_by_key(Vec::len)
}

fn load_policy_prefixes(root: &Path) -> Vec<String> {
    let mut prefixes = Vec::new();
    for path in policy_config_paths(root) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&content) else {
            continue;
        };
        prefixes.extend(extract_policy_prefixes(&value));
    }
    prefixes
}

fn policy_config_paths(root: &Path) -> Vec<std::path::PathBuf> {
    let mut paths = ["covy.toml", "packet28.toml", ".packet28.toml"]
        .into_iter()
        .map(|rel| root.join(rel))
        .collect::<Vec<_>>();
    if let Some(config_home) = config_home_dir() {
        paths.push(config_home.join("packet28").join("config.toml"));
        paths.push(config_home.join("rtk").join("config.toml"));
    }
    paths
}

fn config_home_dir() -> Option<std::path::PathBuf> {
    if let Some(value) = std::env::var_os("XDG_CONFIG_HOME") {
        if !value.is_empty() {
            return Some(std::path::PathBuf::from(value));
        }
    }
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .map(|home| home.join(".config"))
}

fn extract_policy_excludes(value: &toml::Value) -> Vec<String> {
    let paths: &[&[&str]] = &[
        &["packet28", "rewrite", "exclude_commands"],
        &["rewrite", "exclude_commands"],
        &["hooks", "exclude_commands"],
        &["packet28", "hooks", "exclude_commands"],
    ];
    for path in paths {
        if let Some(values) = toml_array_at_path(value, path) {
            let excludes = values
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            if !excludes.is_empty() {
                return excludes;
            }
        }
    }
    Vec::new()
}

fn extract_policy_prefixes(value: &toml::Value) -> Vec<String> {
    let paths: &[&[&str]] = &[
        &["packet28", "rewrite", "transparent_prefixes"],
        &["rewrite", "transparent_prefixes"],
        &["hooks", "transparent_prefixes"],
        &["packet28", "hooks", "transparent_prefixes"],
    ];
    for path in paths {
        if let Some(values) = toml_array_at_path(value, path) {
            let prefixes = values
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            if !prefixes.is_empty() {
                return prefixes;
            }
        }
    }
    Vec::new()
}

fn toml_array_at_path<'a>(value: &'a toml::Value, path: &[&str]) -> Option<&'a Vec<toml::Value>> {
    let mut current = value;
    for key in &path[..path.len().saturating_sub(1)] {
        current = current.get(*key)?;
    }
    current.get(*path.last()?)?.as_array()
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

fn classify_tree_tool(argv: &[String]) -> Option<NativeToolPlan> {
    let mut tool_argv = vec!["compact".to_string(), "tree".to_string()];
    let mut paths = Vec::<String>::new();
    let mut hidden = false;
    let mut max_depth = None::<String>;
    match argv.first()?.as_str() {
        "ls" => {
            for arg in argv.iter().skip(1) {
                if arg.starts_with('-') {
                    if arg.contains('a') {
                        hidden = true;
                    }
                    if arg.contains('R') {
                        max_depth = Some("8".to_string());
                    }
                    continue;
                }
                paths.push(arg.clone());
            }
        }
        "find" => {
            let mut iter = argv.iter().skip(1);
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "-maxdepth" => {
                        if let Some(value) = iter.next() {
                            max_depth = Some(value.clone());
                        }
                    }
                    "-name" | "-type" | "-path" => {
                        let _ = iter.next();
                    }
                    value if value.starts_with('-') => {}
                    value => paths.push(value.to_string()),
                }
            }
        }
        "tree" => {
            let mut iter = argv.iter().skip(1);
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "-a" => hidden = true,
                    "-L" => {
                        if let Some(value) = iter.next() {
                            max_depth = Some(value.clone());
                        }
                    }
                    value if value.starts_with('-') => {}
                    value => paths.push(value.to_string()),
                }
            }
        }
        _ => return None,
    }
    if hidden {
        tool_argv.push("--hidden".to_string());
    }
    if let Some(max_depth) = max_depth {
        tool_argv.push("--max-depth".to_string());
        tool_argv.push(max_depth);
    }
    if paths.is_empty() {
        tool_argv.push(".".to_string());
    } else {
        tool_argv.extend(paths);
    }
    Some(NativeToolPlan {
        kind: NativeToolKind::Tree,
        argv: tool_argv,
    })
}

fn classify_read_tool(argv: &[String]) -> Option<NativeToolPlan> {
    let mut tool_argv = vec!["compact".to_string(), "read".to_string()];
    match argv.first()?.as_str() {
        "cat" => {
            let path = argv.get(1)?.clone();
            tool_argv.push(path);
        }
        "head" => {
            let (count, path) = parse_line_count_and_path(argv.iter().skip(1).cloned().collect())?;
            tool_argv.push("--line-start".to_string());
            tool_argv.push("1".to_string());
            tool_argv.push("--line-end".to_string());
            tool_argv.push(count.to_string());
            tool_argv.push(path);
        }
        "tail" => {
            let (count, path) = parse_line_count_and_path(argv.iter().skip(1).cloned().collect())?;
            tool_argv.push("--last".to_string());
            tool_argv.push(count.to_string());
            tool_argv.push(path);
        }
        "sed" => {
            if argv.len() < 4 || argv.get(1).map(String::as_str) != Some("-n") {
                return None;
            }
            let range = argv.get(2)?;
            let path = argv.get(3)?.clone();
            let (start, end) = parse_sed_range(range)?;
            tool_argv.push("--line-start".to_string());
            tool_argv.push(start.to_string());
            tool_argv.push("--line-end".to_string());
            tool_argv.push(end.to_string());
            tool_argv.push(path);
        }
        _ => return None,
    }
    Some(NativeToolPlan {
        kind: NativeToolKind::Read,
        argv: tool_argv,
    })
}

fn classify_grep_tool(argv: &[String]) -> Option<NativeToolPlan> {
    let mut tool_argv = vec!["compact".to_string(), "grep".to_string()];
    let mut fixed_string = false;
    let mut case_insensitive = false;
    let mut whole_word = false;
    let mut query = None::<String>;
    let mut paths = Vec::<String>::new();
    let mut iter = argv.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-F" => fixed_string = true,
            "-i" => case_insensitive = true,
            "-w" => whole_word = true,
            "-e" => query = iter.next().cloned(),
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
    let query = query?;
    if fixed_string {
        tool_argv.push("--fixed-string".to_string());
    }
    if case_insensitive {
        tool_argv.push("--ignore-case".to_string());
    }
    if whole_word {
        tool_argv.push("--whole-word".to_string());
    }
    tool_argv.push(query);
    if paths.is_empty() {
        tool_argv.push(".".to_string());
    } else {
        tool_argv.extend(paths);
    }
    Some(NativeToolPlan {
        kind: NativeToolKind::Grep,
        argv: tool_argv,
    })
}

fn classify_env_tool(argv: &[String]) -> Option<NativeToolPlan> {
    let mut tool_argv = vec!["compact".to_string(), "env".to_string()];
    if let Some(prefix) = argv.get(1).filter(|value| !value.starts_with('-')) {
        tool_argv.push("--prefix".to_string());
        tool_argv.push(prefix.clone());
    }
    Some(NativeToolPlan {
        kind: NativeToolKind::Env,
        argv: tool_argv,
    })
}

fn apply_postprocess_to_native_tool(
    plan: &mut NativeToolPlan,
    postprocess: Option<&Postprocess>,
) -> bool {
    let Some(postprocess) = postprocess else {
        return true;
    };
    match (&plan.kind, postprocess) {
        (NativeToolKind::Tree, Postprocess::Head(count)) => {
            upsert_tree_option(&mut plan.argv, "--max-entries", *count);
            true
        }
        (NativeToolKind::Grep, Postprocess::Head(count)) => {
            upsert_grep_option(&mut plan.argv, "--max-total-matches", *count);
            true
        }
        (NativeToolKind::Read, Postprocess::Head(count)) => {
            constrain_read_head(&mut plan.argv, *count)
        }
        (NativeToolKind::Read, Postprocess::Tail(count)) => {
            constrain_read_tail(&mut plan.argv, *count)
        }
        (NativeToolKind::Read, Postprocess::SedRange(start, end)) => {
            constrain_read_range(&mut plan.argv, *start, *end)
        }
        _ => false,
    }
}

fn constrain_read_head(argv: &mut Vec<String>, count: usize) -> bool {
    if option_value(argv, "--last").is_some() {
        return false;
    }
    let line_start = option_value(argv, "--line-start").unwrap_or(1);
    let existing_end = option_value(argv, "--line-end").unwrap_or(line_start + count - 1);
    let target_end = existing_end.min(line_start + count.saturating_sub(1));
    upsert_read_option(argv, "--line-start", line_start);
    upsert_read_option(argv, "--line-end", target_end);
    true
}

fn constrain_read_tail(argv: &mut Vec<String>, count: usize) -> bool {
    if option_value(argv, "--line-start").is_some() || option_value(argv, "--line-end").is_some() {
        return false;
    }
    let existing_last = option_value(argv, "--last").unwrap_or(count);
    upsert_read_option(argv, "--last", existing_last.min(count));
    true
}

fn constrain_read_range(argv: &mut Vec<String>, start: usize, end: usize) -> bool {
    if option_value(argv, "--last").is_some() {
        return false;
    }
    let existing_start = option_value(argv, "--line-start").unwrap_or(start);
    let existing_end = option_value(argv, "--line-end").unwrap_or(end);
    let target_start = existing_start.max(start);
    let target_end = existing_end.min(end);
    if target_end < target_start {
        return false;
    }
    upsert_read_option(argv, "--line-start", target_start);
    upsert_read_option(argv, "--line-end", target_end);
    true
}

fn option_value(argv: &[String], flag: &str) -> Option<usize> {
    argv.windows(2).find_map(|pair| match pair {
        [found_flag, value] if found_flag == flag => value.parse::<usize>().ok(),
        _ => None,
    })
}

fn upsert_option_value(argv: &mut Vec<String>, flag: &str, value: usize, insert_idx: usize) {
    let value = value.to_string();
    if let Some(idx) = argv.iter().position(|arg| arg == flag) {
        if let Some(slot) = argv.get_mut(idx + 1) {
            *slot = value;
            return;
        }
    }
    argv.insert(insert_idx.min(argv.len()), flag.to_string());
    argv.insert((insert_idx + 1).min(argv.len()), value);
}

fn upsert_read_option(argv: &mut Vec<String>, flag: &str, value: usize) {
    let insert_idx = argv.len().saturating_sub(1);
    upsert_option_value(argv, flag, value, insert_idx);
}

fn upsert_tree_option(argv: &mut Vec<String>, flag: &str, value: usize) {
    let mut idx = 2usize;
    while idx < argv.len() {
        match argv[idx].as_str() {
            "--hidden" => idx += 1,
            "--max-depth" | "--max-entries" => idx += 2,
            _ => break,
        }
    }
    upsert_option_value(argv, flag, value, idx);
}

fn upsert_grep_option(argv: &mut Vec<String>, flag: &str, value: usize) {
    let mut idx = 2usize;
    while idx < argv.len() {
        match argv[idx].as_str() {
            "--fixed-string" | "--ignore-case" | "--whole-word" => idx += 1,
            "--context-lines" | "--max-matches-per-file" | "--max-total-matches" => idx += 2,
            _ => break,
        }
    }
    upsert_option_value(argv, flag, value, idx);
}

fn parse_line_count_and_path(argv: Vec<String>) -> Option<(usize, String)> {
    let mut count = 10usize;
    let mut path = None::<String>;
    let mut iter = argv.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-n" => {
                count = iter.next()?.parse().ok()?;
            }
            value
                if value.starts_with('-')
                    && value.len() > 1
                    && value[1..].chars().all(|ch| ch.is_ascii_digit()) =>
            {
                count = value[1..].parse().ok()?;
            }
            value if value.starts_with('-') => {}
            value => path = Some(value.to_string()),
        }
    }
    Some((count, path?))
}

fn contains_unsupported_glob_tokens(argv: &[String], native_tool: Option<&NativeToolPlan>) -> bool {
    if !argv.iter().any(|arg| contains_glob_chars(arg)) {
        return false;
    }
    !matches!(
        native_tool.map(|plan| &plan.kind),
        Some(NativeToolKind::Tree) | Some(NativeToolKind::Grep)
    )
}

fn contains_glob_chars(value: &str) -> bool {
    value.contains('*') || value.contains('?') || value.contains('[')
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

fn build_native_tool_rewrite(
    root: &Path,
    task_id: &str,
    cwd: &str,
    plan: &NativeToolPlan,
) -> String {
    let exe = current_exe();
    let mut parts = vec![
        shell_quote(&exe),
        "--via-daemon".to_string(),
        "--daemon-root".to_string(),
        shell_quote(&root.display().to_string()),
    ];
    let (prefix, suffix) = plan.argv.split_at(plan.argv.len().min(2));
    parts.extend(prefix.iter().map(|arg| shell_quote(arg)));
    parts.push("--root".to_string());
    parts.push(shell_quote(&root.display().to_string()));
    parts.push("--task-id".to_string());
    parts.push(shell_quote(task_id));
    parts.push("--cwd".to_string());
    parts.push(shell_quote(cwd));
    parts.extend(suffix.iter().map(|arg| shell_quote(arg)));
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
    let mut rewrite_next_segment = true;
    let mut changed = false;
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            CompoundPart::Operator(op) => {
                rewrite_next_segment = op != "|";
                out.push(op.to_string());
            }
            CompoundPart::Segment(segment) => {
                let rewritten = if rewrite_next_segment {
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
    let mut rewrite_next_segment = true;
    for part in parts {
        match part {
            CompoundPart::Operator(op) => rewrite_next_segment = op != "|",
            CompoundPart::Segment(segment) if rewrite_next_segment => {
                let decision = decide_command_route_inner(segment, cwd, policy_root, false);
                if decision.kind != RouteKind::RawPassthrough {
                    return true;
                }
            }
            CompoundPart::Segment(_) => {}
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
            } else if parts.get(idx - 1).map(String::as_str) == Some(";") {
                out.push(' ');
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
    fn routes_reducer_commands_with_truncation_pipe() {
        let decision = decide_command_route("git show HEAD | head -n 20");
        assert_eq!(decision.kind, RouteKind::ReducerRewrite);
        assert_eq!(
            decision
                .reducer_spec
                .as_ref()
                .map(|spec| spec.canonical_kind.as_str()),
            Some("git_show")
        );
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
    fn routes_pipe_commands_with_supported_left_side() {
        let decision = decide_command_route("git status --short | wc -l");
        assert_eq!(decision.kind, RouteKind::CompoundRewrite);
        assert_eq!(decision.reason.as_deref(), Some("compound_rewrite"));
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
        assert!(rewritten.contains(" cargo test | grep FAIL && "));
        assert!(rewritten.contains(" git status --short"));
        assert_eq!(rewritten.matches("hook reducer-runner").count(), 2);
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
            ("npm run-script biome check .", "javascript_lint"),
            ("pnpm dlx next build", "javascript_next_build"),
            ("pnpm exec prisma migrate status", "javascript_prisma"),
            ("pnpx vitest run", "javascript_vitest"),
            ("npx jest run", "javascript_test"),
            ("npx playwright test", "javascript_test"),
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
    fn routes_rtk_python_package_manager_forms() {
        for (command, expected) in [
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
    fn routes_rtk_docker_and_kubectl_mutation_forms() {
        for (command, expected) in [
            ("docker build .", "docker_build"),
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
    fn routes_grep_with_head_pipe_to_native_tool_limit() {
        let decision = decide_command_route("rg task_id crates/suite-cli/src | head -n 5");
        assert_eq!(decision.kind, RouteKind::NativeTool);
        assert_eq!(
            decision.native_tool.as_ref().map(|plan| plan.argv.clone()),
            Some(vec![
                "compact".to_string(),
                "grep".to_string(),
                "--max-total-matches".to_string(),
                "5".to_string(),
                "task_id".to_string(),
                "crates/suite-cli/src".to_string(),
            ])
        );
    }

    #[test]
    fn routes_tree_with_head_pipe_to_native_tool_limit() {
        let decision = decide_command_route("tree -L 2 crates | head -n 10");
        assert_eq!(decision.kind, RouteKind::NativeTool);
        assert_eq!(
            decision.native_tool.as_ref().map(|plan| plan.argv.clone()),
            Some(vec![
                "compact".to_string(),
                "tree".to_string(),
                "--max-depth".to_string(),
                "2".to_string(),
                "--max-entries".to_string(),
                "10".to_string(),
                "crates".to_string(),
            ])
        );
    }

    #[test]
    fn routes_cat_with_sed_pipe_to_read_range() {
        let decision = decide_command_route("cat Cargo.toml | sed -n '1,20p'");
        assert_eq!(decision.kind, RouteKind::NativeTool);
        assert_eq!(
            decision.native_tool.as_ref().map(|plan| plan.argv.clone()),
            Some(vec![
                "compact".to_string(),
                "read".to_string(),
                "--line-start".to_string(),
                "1".to_string(),
                "--line-end".to_string(),
                "20".to_string(),
                "Cargo.toml".to_string(),
            ])
        );
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
