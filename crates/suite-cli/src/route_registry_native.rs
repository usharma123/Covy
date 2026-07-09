use std::path::Path;

use crate::route_registry::{NativeToolKind, NativeToolPlan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Postprocess {
    Head(usize),
    Tail(usize),
    SedRange(usize, usize),
}

pub(super) fn classify_native_tool(argv: &[String]) -> Option<NativeToolPlan> {
    match argv.first()?.as_str() {
        "ls" | "find" | "tree" => classify_tree_tool(argv),
        "cat" | "head" | "tail" | "sed" => classify_read_tool(argv),
        "grep" | "rg" => classify_grep_tool(argv),
        "env" | "printenv" => classify_env_tool(argv),
        _ => None,
    }
}

pub(super) fn contains_unsupported_glob_tokens(
    argv: &[String],
    native_tool: Option<&NativeToolPlan>,
) -> bool {
    if !argv.iter().any(|arg| contains_glob_chars(arg)) {
        return false;
    }
    !matches!(
        native_tool.map(|plan| &plan.kind),
        Some(NativeToolKind::Tree) | Some(NativeToolKind::Grep)
    )
}

pub(super) fn apply_postprocess_to_native_tool(
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

pub(super) fn build_native_tool_rewrite(
    root: &Path,
    task_id: &str,
    cwd: &str,
    plan: &NativeToolPlan,
) -> String {
    let exe = super::current_exe();
    let mut parts = vec![
        super::shell_quote(&exe),
        "--via-daemon".to_string(),
        "--daemon-root".to_string(),
        super::shell_quote(&root.display().to_string()),
    ];
    let (prefix, suffix) = plan.argv.split_at(plan.argv.len().min(2));
    parts.extend(prefix.iter().map(|arg| super::shell_quote(arg)));
    parts.push("--root".to_string());
    parts.push(super::shell_quote(&root.display().to_string()));
    parts.push("--task-id".to_string());
    parts.push(super::shell_quote(task_id));
    parts.push("--cwd".to_string());
    parts.push(super::shell_quote(cwd));
    parts.extend(suffix.iter().map(|arg| super::shell_quote(arg)));
    parts.join(" ")
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
                    if arg == "--" {
                        continue;
                    }
                    if arg == "--all" || arg == "--almost-all" {
                        hidden = true;
                        continue;
                    }
                    if arg.starts_with("--color") {
                        continue;
                    }
                    let short_flags = arg.strip_prefix('-')?;
                    if !arg.starts_with("--")
                        && short_flags.chars().all(|ch| {
                            matches!(ch, 'a' | 'A' | 'l' | 'h' | '1' | 'C' | 'F' | 'G' | 'R')
                        })
                    {
                        if short_flags.chars().any(|ch| matches!(ch, 'a' | 'A')) {
                            hidden = true;
                        }
                        if arg.contains('R') {
                            max_depth = Some("8".to_string());
                        }
                        continue;
                    }
                    return None;
                }
                paths.push(arg.clone());
            }
        }
        "find" => {
            let mut iter = argv.iter().skip(1);
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "-maxdepth" => {
                        max_depth = Some(iter.next()?.clone());
                    }
                    "--" => {
                        paths.extend(iter.cloned());
                        break;
                    }
                    value if value.starts_with('-') => return None,
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
                        max_depth = Some(iter.next()?.clone());
                    }
                    "--" => {
                        paths.extend(iter.cloned());
                        break;
                    }
                    value if value.starts_with('-') => return None,
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
            let path = parse_cat_read_path(argv)?;
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
            if argv.len() != 4 || argv.get(1).map(String::as_str) != Some("-n") {
                return None;
            }
            let range = argv.get(2)?;
            let path = argv.get(3)?.clone();
            let (start, end) = super::parse_sed_range(range)?;
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

fn parse_cat_read_path(argv: &[String]) -> Option<String> {
    let mut path = None::<String>;
    let mut iter = argv.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-n" => {}
            "--" => {
                path = iter.next().cloned();
                if iter.next().is_some() {
                    return None;
                }
                break;
            }
            value if value.starts_with('-') => return None,
            value => {
                if path.is_some() {
                    return None;
                }
                path = Some(value.to_string());
            }
        }
    }
    path
}

fn classify_grep_tool(argv: &[String]) -> Option<NativeToolPlan> {
    if has_grep_extraction_mode(argv) {
        return None;
    }
    let program = argv.first()?.as_str();
    let mut tool_argv = vec!["compact".to_string(), "grep".to_string()];
    let mut fixed_string = false;
    let mut case_insensitive = false;
    let mut whole_word = false;
    let mut query = None::<String>;
    let mut paths = Vec::<String>::new();
    let mut iter = argv.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--" => {
                for value in iter {
                    push_grep_operand(value, &mut query, &mut paths)?;
                }
                break;
            }
            "-F" | "--fixed-strings" => fixed_string = true,
            "-i" | "--ignore-case" => case_insensitive = true,
            "-w" | "--word-regexp" => whole_word = true,
            "-e" | "--regexp" => set_grep_query(&mut query, iter.next()?.clone())?,
            value if value.starts_with("--regexp=") => {
                set_grep_query(&mut query, value["--regexp=".len()..].to_string())?
            }
            value if value.starts_with("-e") && value.len() > 2 => {
                set_grep_query(&mut query, value[2..].to_string())?
            }
            value if value.starts_with('-') && !value.starts_with("--") => {
                if value[1..].chars().all(|ch| matches!(ch, 'F' | 'i' | 'w')) {
                    fixed_string |= value.contains('F');
                    case_insensitive |= value.contains('i');
                    whole_word |= value.contains('w');
                } else {
                    return None;
                }
            }
            value if value.starts_with('-') => return None,
            value => {
                push_grep_operand(value, &mut query, &mut paths)?;
            }
        }
    }
    let query = query?;
    if program == "grep" && !fixed_string {
        tool_argv.push("--basic-regexp".to_string());
    }
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

fn push_grep_operand(
    value: &str,
    query: &mut Option<String>,
    paths: &mut Vec<String>,
) -> Option<()> {
    if query.is_none() {
        *query = Some(value.to_string());
    } else {
        paths.push(value.to_string());
    }
    Some(())
}

fn set_grep_query(query: &mut Option<String>, value: String) -> Option<()> {
    if query.replace(value).is_some() {
        return None;
    }
    Some(())
}

fn has_grep_extraction_mode(argv: &[String]) -> bool {
    argv.iter().skip(1).any(|arg| match arg.as_str() {
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
            "-n" | "--lines" => {
                count = parse_line_count(iter.next()?.as_str())?;
            }
            "--" => {
                path = Some(iter.next()?);
                if iter.next().is_some() {
                    return None;
                }
                break;
            }
            value if value.starts_with("--lines=") => {
                count = parse_line_count(&value["--lines=".len()..])?;
            }
            value
                if value.starts_with('-')
                    && value.len() > 1
                    && value[1..].chars().all(|ch| ch.is_ascii_digit()) =>
            {
                count = parse_line_count(&value[1..])?;
            }
            value if value.starts_with('-') => return None,
            value => {
                if path.is_some() {
                    return None;
                }
                path = Some(value.to_string());
            }
        }
    }
    Some((count, path?))
}

fn parse_line_count(value: &str) -> Option<usize> {
    value.parse::<usize>().ok()
}

pub(super) fn contains_glob_chars(value: &str) -> bool {
    value.contains('*') || value.contains('?') || value.contains('[')
}
