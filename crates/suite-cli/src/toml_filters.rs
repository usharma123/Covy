use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::{Regex, RegexSet};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppliedTomlFilter {
    pub(crate) name: String,
    pub(crate) source: String,
    pub(crate) output: String,
    pub(crate) filter_stderr: bool,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct TomlFilterFile {
    schema_version: u32,
    filters: BTreeMap<String, TomlFilterDef>,
}

impl Default for TomlFilterFile {
    fn default() -> Self {
        Self {
            schema_version: 1,
            filters: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct TomlFilterDef {
    description: Option<String>,
    match_command: String,
    strip_ansi: bool,
    replace: Vec<ReplaceRule>,
    match_output: Vec<MatchOutputRule>,
    strip_lines_matching: Vec<String>,
    keep_lines_matching: Vec<String>,
    truncate_lines_at: Option<usize>,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
    max_lines: Option<usize>,
    on_empty: Option<String>,
    filter_stderr: bool,
}

impl Default for TomlFilterDef {
    fn default() -> Self {
        Self {
            description: None,
            match_command: String::new(),
            strip_ansi: false,
            replace: Vec::new(),
            match_output: Vec::new(),
            strip_lines_matching: Vec::new(),
            keep_lines_matching: Vec::new(),
            truncate_lines_at: None,
            head_lines: None,
            tail_lines: None,
            max_lines: None,
            on_empty: None,
            filter_stderr: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ReplaceRule {
    pattern: String,
    replacement: String,
}

impl Default for ReplaceRule {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            replacement: String::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct MatchOutputRule {
    pattern: String,
    message: String,
    unless: Option<String>,
}

impl Default for MatchOutputRule {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            message: String::new(),
            unless: None,
        }
    }
}

#[derive(Debug)]
struct CompiledFilter {
    name: String,
    source: String,
    match_command: Regex,
    strip_ansi: bool,
    replace: Vec<CompiledReplaceRule>,
    match_output: Vec<CompiledMatchOutputRule>,
    line_filter: LineFilter,
    truncate_lines_at: Option<usize>,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
    max_lines: Option<usize>,
    on_empty: Option<String>,
    filter_stderr: bool,
}

#[derive(Debug)]
struct CompiledReplaceRule {
    pattern: Regex,
    replacement: String,
}

#[derive(Debug)]
struct CompiledMatchOutputRule {
    pattern: Regex,
    message: String,
    unless: Option<Regex>,
}

#[derive(Debug)]
enum LineFilter {
    None,
    Strip(RegexSet),
    Keep(RegexSet),
}

pub(crate) fn apply_configured_filter(
    root: &Path,
    command: &str,
    stdout: &str,
    stderr: &str,
) -> Result<Option<AppliedTomlFilter>> {
    for filter in load_filters(root)? {
        if !filter.match_command.is_match(command) {
            continue;
        }
        let input = if filter.filter_stderr && !stderr.is_empty() {
            format!("{stdout}{stderr}")
        } else {
            stdout.to_string()
        };
        let mut output = apply_filter(&filter, &input);
        if !filter.filter_stderr && !stderr.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(stderr);
        }
        return Ok(Some(AppliedTomlFilter {
            name: filter.name,
            source: filter.source,
            output,
            filter_stderr: filter.filter_stderr,
        }));
    }
    Ok(None)
}

fn load_filters(root: &Path) -> Result<Vec<CompiledFilter>> {
    let mut filters = Vec::new();
    for path in filter_config_paths(root) {
        if !path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read filter config '{}'", path.display()))?;
        filters.extend(parse_filters(&content, &path.display().to_string())?);
    }
    Ok(filters)
}

fn filter_config_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        root.join(".packet28").join("filters.toml"),
        root.join(".rtk").join("filters.toml"),
    ];
    if let Some(config_home) = config_home_dir() {
        paths.push(config_home.join("packet28").join("filters.toml"));
        paths.push(config_home.join("rtk").join("filters.toml"));
    }
    paths
}

fn config_home_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

fn parse_filters(content: &str, source: &str) -> Result<Vec<CompiledFilter>> {
    let file: TomlFilterFile = toml::from_str(content)
        .with_context(|| format!("failed to parse filter config '{source}'"))?;
    anyhow::ensure!(
        file.schema_version == 1,
        "unsupported filter schema_version {} in '{}' (expected 1)",
        file.schema_version,
        source
    );
    file.filters
        .into_iter()
        .map(|(name, def)| compile_filter(name, source.to_string(), def))
        .collect()
}

fn compile_filter(name: String, source: String, def: TomlFilterDef) -> Result<CompiledFilter> {
    anyhow::ensure!(
        !def.match_command.trim().is_empty(),
        "filter '{name}' in '{source}' is missing match_command"
    );
    anyhow::ensure!(
        def.strip_lines_matching.is_empty() || def.keep_lines_matching.is_empty(),
        "filter '{name}' in '{source}' cannot set both strip_lines_matching and keep_lines_matching"
    );
    let match_command = Regex::new(&def.match_command)
        .with_context(|| format!("invalid match_command for filter '{name}' in '{source}'"))?;
    let replace = def
        .replace
        .into_iter()
        .map(|rule| {
            Ok(CompiledReplaceRule {
                pattern: Regex::new(&rule.pattern).with_context(|| {
                    format!("invalid replace pattern for filter '{name}' in '{source}'")
                })?,
                replacement: rule.replacement,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let match_output = def
        .match_output
        .into_iter()
        .map(|rule| {
            Ok(CompiledMatchOutputRule {
                pattern: Regex::new(&rule.pattern).with_context(|| {
                    format!("invalid match_output pattern for filter '{name}' in '{source}'")
                })?,
                message: rule.message,
                unless: rule
                    .unless
                    .as_deref()
                    .map(Regex::new)
                    .transpose()
                    .with_context(|| {
                        format!(
                            "invalid match_output unless pattern for filter '{name}' in '{source}'"
                        )
                    })?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let line_filter = if !def.strip_lines_matching.is_empty() {
        LineFilter::Strip(RegexSet::new(&def.strip_lines_matching).with_context(|| {
            format!("invalid strip_lines_matching for filter '{name}' in '{source}'")
        })?)
    } else if !def.keep_lines_matching.is_empty() {
        LineFilter::Keep(RegexSet::new(&def.keep_lines_matching).with_context(|| {
            format!("invalid keep_lines_matching for filter '{name}' in '{source}'")
        })?)
    } else {
        LineFilter::None
    };
    Ok(CompiledFilter {
        name,
        source,
        match_command,
        strip_ansi: def.strip_ansi,
        replace,
        match_output,
        line_filter,
        truncate_lines_at: def.truncate_lines_at,
        head_lines: def.head_lines,
        tail_lines: def.tail_lines,
        max_lines: def.max_lines,
        on_empty: def.on_empty,
        filter_stderr: def.filter_stderr,
    })
}

fn apply_filter(filter: &CompiledFilter, input: &str) -> String {
    let mut lines = input.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    if filter.strip_ansi {
        lines = lines.into_iter().map(|line| strip_ansi(&line)).collect();
    }
    for rule in &filter.replace {
        lines = lines
            .into_iter()
            .map(|line| {
                rule.pattern
                    .replace_all(&line, rule.replacement.as_str())
                    .into_owned()
            })
            .collect();
    }
    if !filter.match_output.is_empty() {
        let blob = lines.join("\n");
        for rule in &filter.match_output {
            if rule.pattern.is_match(&blob)
                && rule
                    .unless
                    .as_ref()
                    .map(|unless| !unless.is_match(&blob))
                    .unwrap_or(true)
            {
                return rule.message.clone();
            }
        }
    }
    match &filter.line_filter {
        LineFilter::None => {}
        LineFilter::Strip(set) => lines.retain(|line| !set.is_match(line)),
        LineFilter::Keep(set) => lines.retain(|line| set.is_match(line)),
    }
    if let Some(max_chars) = filter.truncate_lines_at {
        lines = lines
            .into_iter()
            .map(|line| truncate_chars(&line, max_chars))
            .collect();
    }
    let total = lines.len();
    match (filter.head_lines, filter.tail_lines) {
        (Some(head), Some(tail)) if total > head.saturating_add(tail) => {
            let omitted = total.saturating_sub(head).saturating_sub(tail);
            let mut selected = lines[..head].to_vec();
            selected.push(format!("... ({omitted} lines omitted)"));
            selected.extend_from_slice(&lines[total - tail..]);
            lines = selected;
        }
        (Some(head), _) if total > head => {
            lines.truncate(head);
            lines.push(format!("... ({} lines omitted)", total - head));
        }
        (_, Some(tail)) if total > tail => {
            let omitted = total - tail;
            lines = lines[omitted..].to_vec();
            lines.insert(0, format!("... ({omitted} lines omitted)"));
        }
        _ => {}
    }
    if let Some(max_lines) = filter.max_lines {
        if lines.len() > max_lines {
            let truncated = lines.len() - max_lines;
            lines.truncate(max_lines);
            lines.push(format!("... ({truncated} lines truncated)"));
        }
    }
    let output = lines.join("\n");
    if output.trim().is_empty() {
        filter.on_empty.clone().unwrap_or(output)
    } else {
        output
    }
}

fn strip_ansi(value: &str) -> String {
    Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
        .expect("valid ansi regex")
        .replace_all(value, "")
        .into_owned()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_rtk_style_filter_pipeline() {
        let content = r#"
schema_version = 1

[filters.demo]
match_command = "^demo-tool\\b"
strip_ansi = true
strip_lines_matching = ["^debug:"]
truncate_lines_at = 12
head_lines = 1
tail_lines = 1

[[filters.demo.replace]]
pattern = "TOKEN=[A-Za-z0-9]+"
replacement = "TOKEN=<redacted>"
"#;
        let filters = parse_filters(content, "inline").unwrap();
        let filter = filters.first().unwrap();
        assert!(filter.match_command.is_match("demo-tool run"));
        let output = apply_filter(
            filter,
            "\u{1b}[31mfirst TOKEN=abcdef\u{1b}[0m\ndebug: skip\nmiddle\nlast",
        );
        assert_eq!(output, "first TOKEN=...\n... (1 lines omitted)\nlast");
    }

    #[test]
    fn match_output_honors_unless() {
        let content = r#"
schema_version = 1

[filters.ok]
match_command = "^ok-tool\\b"

[[filters.ok.match_output]]
pattern = "success"
message = "ok-tool: success"
unless = "ERROR"
"#;
        let filters = parse_filters(content, "inline").unwrap();
        let filter = filters.first().unwrap();
        assert_eq!(
            apply_filter(filter, "success\nall good"),
            "ok-tool: success"
        );
        assert_eq!(apply_filter(filter, "success\nERROR"), "success\nERROR");
    }
}
