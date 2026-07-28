use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use packet28_reducer_core::{SearchRequest, SearchResult};
use regex::Regex;
use serde_json::Value;

use super::{
    compact_for_log, emit_system_output, summarize_rendered_lines, FindArgs, GrepArgs, GrepEngine,
    SystemOutput,
};

pub fn run_find(args: FindArgs) -> Result<i32> {
    let parsed = parse_find_args(&args.args)?;
    let text = render_find_results(&parsed)?;
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 find".to_string(),
            summary: summarize_rendered_lines("find", &text),
            text,
        },
    )
}

pub fn run_grep(args: GrepArgs) -> Result<i32> {
    let text = render_grep_results(&args)?;
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 grep".to_string(),
            summary: summarize_rendered_lines("grep", &text),
            text,
        },
    )
}

#[derive(Debug, Clone)]
pub(super) struct ParsedFindArgs {
    pub(super) pattern: String,
    pub(super) path: PathBuf,
    pub(super) max_results: usize,
    pub(super) max_depth: Option<usize>,
    pub(super) file_type: String,
    pub(super) case_insensitive: bool,
}

impl Default for ParsedFindArgs {
    fn default() -> Self {
        Self {
            pattern: "*".to_string(),
            path: PathBuf::from("."),
            max_results: 50,
            max_depth: None,
            file_type: "f".to_string(),
            case_insensitive: false,
        }
    }
}

pub(super) fn parse_find_args(args: &[String]) -> Result<ParsedFindArgs> {
    const UNSUPPORTED: &[&str] = &[
        "-not", "!", "-or", "-o", "-and", "-a", "-exec", "-execdir", "-delete", "-print0",
        "-newer", "-perm", "-size", "-mtime", "-mmin", "-atime", "-amin", "-ctime", "-cmin",
        "-empty", "-link", "-regex", "-iregex",
    ];
    if args.iter().any(|arg| UNSUPPORTED.contains(&arg.as_str())) {
        bail!("Packet28 find does not support compound predicates or actions; use native find directly");
    }
    if args.is_empty() {
        return Ok(ParsedFindArgs::default());
    }
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-name" | "-iname" | "-type" | "-maxdepth"))
    {
        parse_native_find_args(args)
    } else {
        parse_compact_find_args(args)
    }
}

fn parse_native_find_args(args: &[String]) -> Result<ParsedFindArgs> {
    let mut parsed = ParsedFindArgs::default();
    let mut idx = 0usize;
    if let Some(first) = args.first() {
        if !first.starts_with('-') {
            parsed.path = PathBuf::from(first);
            idx = 1;
        }
    }
    while idx < args.len() {
        match args[idx].as_str() {
            "-name" => {
                idx += 1;
                parsed.pattern = args.get(idx).cloned().context("missing -name value")?;
            }
            "-iname" => {
                idx += 1;
                parsed.pattern = args.get(idx).cloned().context("missing -iname value")?;
                parsed.case_insensitive = true;
            }
            "-type" => {
                idx += 1;
                parsed.file_type = args.get(idx).cloned().context("missing -type value")?;
            }
            "-maxdepth" => {
                idx += 1;
                parsed.max_depth = Some(
                    args.get(idx)
                        .context("missing -maxdepth value")?
                        .parse()
                        .context("invalid -maxdepth value")?,
                );
            }
            _ => {}
        }
        idx += 1;
    }
    Ok(parsed)
}

fn parse_compact_find_args(args: &[String]) -> Result<ParsedFindArgs> {
    let mut parsed = ParsedFindArgs {
        pattern: args.first().cloned().unwrap_or_else(|| "*".to_string()),
        ..ParsedFindArgs::default()
    };
    let mut idx = 1usize;
    if let Some(path) = args.get(idx) {
        if !path.starts_with('-') {
            parsed.path = PathBuf::from(path);
            idx += 1;
        }
    }
    while idx < args.len() {
        match args[idx].as_str() {
            "-m" | "--max" => {
                idx += 1;
                parsed.max_results = args
                    .get(idx)
                    .context("missing --max value")?
                    .parse()
                    .context("invalid --max value")?;
            }
            "-t" | "--file-type" => {
                idx += 1;
                parsed.file_type = args
                    .get(idx)
                    .cloned()
                    .context("missing --file-type value")?;
            }
            _ => {}
        }
        idx += 1;
    }
    Ok(parsed)
}

pub(super) fn render_find_results(args: &ParsedFindArgs) -> Result<String> {
    let mut matches = Vec::new();
    let include_hidden = args.pattern.starts_with('.');
    visit_find_path(
        &args.path,
        0,
        args.max_depth,
        include_hidden,
        args,
        &mut matches,
    )?;
    matches.sort();
    let total = matches.len();
    let shown = total.min(args.max_results);
    let mut out = format!(
        "{} match(es) under {} for {}\n",
        total,
        args.path.display(),
        args.pattern
    );
    for path in matches.iter().take(args.max_results) {
        out.push_str(&format!("{}\n", path.display()));
    }
    if total > shown {
        out.push_str(&format!("[+{} more]\n", total - shown));
    }
    Ok(out)
}

fn visit_find_path(
    path: &Path,
    depth: usize,
    max_depth: Option<usize>,
    include_hidden: bool,
    args: &ParsedFindArgs,
    matches: &mut Vec<PathBuf>,
) -> Result<()> {
    if max_depth.is_some_and(|max| depth > max) {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !include_hidden && depth > 0 && name.starts_with('.') {
        return Ok(());
    }
    let is_dir = metadata.is_dir();
    let type_matches = match args.file_type.as_str() {
        "d" => is_dir,
        "f" => metadata.is_file(),
        _ => true,
    };
    if type_matches && glob_name_matches(&args.pattern, name, args.case_insensitive) {
        matches.push(path.to_path_buf());
    }
    if is_dir {
        for entry in
            fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))?
        {
            let entry = entry?;
            visit_find_path(
                &entry.path(),
                depth + 1,
                max_depth,
                include_hidden,
                args,
                matches,
            )?;
        }
    }
    Ok(())
}

fn glob_name_matches(pattern: &str, name: &str, case_insensitive: bool) -> bool {
    let pattern = if pattern == "." { "*" } else { pattern };
    if case_insensitive {
        glob_match(&pattern.to_lowercase(), &name.to_lowercase())
    } else {
        glob_match(pattern, name)
    }
}

fn glob_match(pattern: &str, name: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), name.as_bytes())
}

fn glob_match_inner(pattern: &[u8], name: &[u8]) -> bool {
    match (pattern.first(), name.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            glob_match_inner(&pattern[1..], name)
                || (!name.is_empty() && glob_match_inner(pattern, &name[1..]))
        }
        (Some(b'?'), Some(_)) => glob_match_inner(&pattern[1..], &name[1..]),
        (Some(pattern_byte), Some(name_byte)) if pattern_byte == name_byte => {
            glob_match_inner(&pattern[1..], &name[1..])
        }
        _ => false,
    }
}

pub(super) fn render_grep_results(args: &GrepArgs) -> Result<String> {
    let pattern = normalize_grep_pattern(&args.pattern);
    let regex =
        Regex::new(&pattern).with_context(|| format!("invalid grep pattern '{pattern}'"))?;
    let mut result = search_result_for_grep(args)?;
    filter_search_result_by_file_type(&mut result, args.file_type.as_deref());
    render_search_result_for_grep(&result, args, &regex)
}

fn search_result_for_grep(args: &GrepArgs) -> Result<SearchResult> {
    if matches!(args.engine, GrepEngine::Fff | GrepEngine::Indexed) {
        return p28_search_result(args).or_else(|err| {
            if args.engine == GrepEngine::Fff {
                Err(err)
            } else {
                reducer_search_result(args)
            }
        });
    }
    reducer_search_result(args)
}

fn reducer_search_result(args: &GrepArgs) -> Result<SearchResult> {
    let (root, requested_paths) = grep_root_and_paths(&args.path)?;
    packet28_reducer_core::search(
        &root,
        &SearchRequest {
            query: normalize_grep_pattern(&args.pattern),
            requested_paths,
            max_matches_per_file: Some(1000),
            max_total_matches: Some(1000),
            ..SearchRequest::default()
        },
    )
}

fn normalize_grep_pattern(pattern: &str) -> String {
    pattern.replace(r"\|", "|")
}

fn p28_search_result(args: &GrepArgs) -> Result<SearchResult> {
    let bin = p28_binary().context("p28 search binary was not found")?;
    let (root, requested_paths) = grep_root_and_paths(&args.path)?;
    let mut command = Command::new(&bin);
    command.current_dir(&root);
    command.arg(&args.pattern);
    for path in requested_paths {
        command.arg(path);
    }
    command
        .args(["--json", "--engine", grep_engine_arg(args.engine)])
        .arg("--max-total-matches")
        .arg("1000")
        .arg("--max-matches-per-file")
        .arg("1000");
    let output = command
        .output()
        .with_context(|| format!("failed to execute p28 search backend '{}'", bin.display()))?;
    if !output.status.success() {
        bail!(
            "p28 search backend failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).context("p28 emitted invalid JSON")?;
    serde_json::from_value(value["result"].clone()).context("p28 JSON missing search result")
}

fn grep_root_and_paths(path: &Path) -> Result<(PathBuf, Vec<String>)> {
    if path.is_absolute() {
        if path.is_file() {
            let root = path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("absolute file path has no parent"))?
                .to_path_buf();
            let file = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| anyhow::anyhow!("absolute file path is not valid UTF-8"))?
                .to_string();
            return Ok((root, vec![file]));
        }
        return Ok((path.to_path_buf(), Vec::new()));
    }
    let root = env::current_dir().context("failed to resolve current directory")?;
    let value = path.to_string_lossy().replace('\\', "/");
    let requested_paths = (value != ".").then_some(value).into_iter().collect();
    Ok((root, requested_paths))
}

fn grep_engine_arg(engine: GrepEngine) -> &'static str {
    match engine {
        GrepEngine::Auto => "auto",
        GrepEngine::Indexed => "indexed",
        GrepEngine::Legacy => "legacy",
        GrepEngine::Fff => "fff",
    }
}

fn p28_binary() -> Option<PathBuf> {
    if let Some(bin) = env::var_os("P28_SEARCH_BIN").map(PathBuf::from) {
        if bin.is_file() {
            return Some(bin);
        }
    }
    if let Ok(current) = env::current_exe() {
        if let Some(parent) = current.parent() {
            let sibling = parent.join("p28");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|path| path.join("p28"))
            .find(|path| path.is_file())
    })
}

fn filter_search_result_by_file_type(result: &mut SearchResult, file_type: Option<&str>) {
    let Some(file_type) = file_type.map(|value| value.trim_start_matches('.')) else {
        return;
    };
    result.groups.retain_mut(|group| {
        if !Path::new(&group.path)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext == file_type)
        {
            return false;
        }
        group.matches.retain(|item| {
            Path::new(&item.path)
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == file_type)
        });
        group.match_count = group.matches.len();
        group.displayed_match_count = group.matches.len();
        !group.matches.is_empty()
    });
    result.match_count = result.groups.iter().map(|group| group.match_count).sum();
    result.returned_match_count = result
        .groups
        .iter()
        .map(|group| group.matches.len())
        .sum::<usize>();
    result.paths = result
        .groups
        .iter()
        .map(|group| group.path.clone())
        .collect();
    result.resolved_paths = result.paths.clone();
    result.regions = result
        .groups
        .iter()
        .flat_map(|group| {
            group
                .matches
                .iter()
                .map(|item| format!("{}:{}-{}", item.path, item.line, item.line))
        })
        .collect();
}

fn render_search_result_for_grep(
    result: &SearchResult,
    args: &GrepArgs,
    regex: &Regex,
) -> Result<String> {
    let mut matches = result
        .groups
        .iter()
        .flat_map(|group| group.matches.iter())
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    if matches.is_empty() {
        return Ok(format!("0 matches for '{}'\n", args.pattern));
    }
    let file_count = result.paths.len();
    let total = result.match_count;
    let mut out = format!("{total} matches in {file_count} files:\n\n");
    for item in matches.iter().take(args.max) {
        out.push_str(&format!(
            "{}:{}:{}\n",
            compact_path_for_grep(Path::new(&item.path)),
            item.line,
            clean_grep_line(&item.text, args.max_len, args.context_only, regex)
        ));
    }
    if total > args.max {
        out.push_str(&format!("[+{} more]\n", total - args.max));
    }
    Ok(out)
}

fn clean_grep_line(line: &str, max_len: usize, context_only: bool, regex: &Regex) -> String {
    let trimmed = line.trim();
    if context_only {
        if let Some(mat) = regex.find(trimmed) {
            let chars = trimmed.chars().collect::<Vec<_>>();
            let start = trimmed[..mat.start()].chars().count().saturating_sub(20);
            let end = (trimmed[..mat.end()].chars().count() + 20).min(chars.len());
            return chars[start..end].iter().collect::<String>();
        }
    }
    compact_for_log(trimmed, max_len)
}

fn compact_path_for_grep(path: &Path) -> String {
    let display = path.display().to_string();
    if display.len() <= 50 {
        return display;
    }
    let parts = display.split('/').collect::<Vec<_>>();
    if parts.len() <= 3 {
        display
    } else {
        format!(
            "{}/.../{}/{}",
            parts[0],
            parts[parts.len() - 2],
            parts[parts.len() - 1]
        )
    }
}
