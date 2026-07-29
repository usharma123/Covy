use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;

use super::{
    emit_system_output, summarize_rendered_lines, ReadArgs, ReadFilterLevel, SmartArgs,
    SystemOutput,
};

pub fn run_read(args: ReadArgs) -> Result<i32> {
    let mut rendered_files = Vec::new();
    for file in &args.files {
        let raw = if file.as_os_str() == "-" {
            let mut content = String::new();
            io::stdin()
                .lock()
                .read_to_string(&mut content)
                .context("failed to read from stdin")?;
            content
        } else {
            fs::read_to_string(file)
                .with_context(|| format!("failed to read {}", file.display()))?
        };
        let language = detect_language(file);
        let filtered = filter_read_content(&raw, language, args.level);
        let windowed = apply_line_window(&filtered, args.max_lines, args.tail_lines, language);
        let final_text = if args.line_numbers {
            format_with_line_numbers(&windowed)
        } else {
            windowed
        };
        if args.files.len() > 1 && file.as_os_str() != "-" {
            rendered_files.push(format!("==> {} <==\n{}", file.display(), final_text));
        } else {
            rendered_files.push(final_text);
        }
    }
    let text = rendered_files.join(if args.line_numbers { "\n" } else { "" });
    let summary = summarize_rendered_lines("read", &text);
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 read".to_string(),
            summary,
            text,
        },
    )
}

pub fn run_smart(args: SmartArgs) -> Result<i32> {
    let raw = fs::read_to_string(&args.file)
        .with_context(|| format!("failed to read {}", args.file.display()))?;
    let language = detect_language(&args.file);
    let summary = summarize_source_file(&raw, language);
    let text = format!("{}\n{}", summary.line1, summary.line2);
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 smart".to_string(),
            summary: summarize_rendered_lines("smart", &text),
            text,
        },
    )?;
    Ok(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadLanguage {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Go,
    Shell,
    Markdown,
    Json,
    Toml,
    Yaml,
    Unknown,
}

fn detect_language(path: &Path) -> ReadLanguage {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => ReadLanguage::Rust,
        Some("js" | "jsx" | "mjs" | "cjs") => ReadLanguage::JavaScript,
        Some("ts" | "tsx") => ReadLanguage::TypeScript,
        Some("py") => ReadLanguage::Python,
        Some("go") => ReadLanguage::Go,
        Some("sh" | "bash" | "zsh") => ReadLanguage::Shell,
        Some("md" | "markdown") => ReadLanguage::Markdown,
        Some("json") => ReadLanguage::Json,
        Some("toml") => ReadLanguage::Toml,
        Some("yaml" | "yml") => ReadLanguage::Yaml,
        _ => ReadLanguage::Unknown,
    }
}

pub(super) fn filter_read_content(
    content: &str,
    language: ReadLanguage,
    level: ReadFilterLevel,
) -> String {
    match level {
        ReadFilterLevel::None => content.to_string(),
        ReadFilterLevel::Minimal => strip_language_comments(content, language, false),
        ReadFilterLevel::Aggressive => strip_language_comments(content, language, true),
    }
}

fn strip_language_comments(content: &str, language: ReadLanguage, aggressive: bool) -> String {
    let mut out = Vec::new();
    let mut blank_pending = false;
    for line in content.lines() {
        let trimmed = line.trim();
        let is_comment = match language {
            ReadLanguage::Rust
            | ReadLanguage::JavaScript
            | ReadLanguage::TypeScript
            | ReadLanguage::Go => {
                trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*')
            }
            ReadLanguage::Python | ReadLanguage::Shell => trimmed.starts_with('#'),
            ReadLanguage::Markdown
            | ReadLanguage::Json
            | ReadLanguage::Toml
            | ReadLanguage::Yaml
            | ReadLanguage::Unknown => false,
        };
        if is_comment {
            continue;
        }
        if aggressive && trimmed.is_empty() {
            if !blank_pending {
                out.push(String::new());
                blank_pending = true;
            }
            continue;
        }
        blank_pending = false;
        out.push(line.to_string());
    }
    let mut rendered = out.join("\n");
    if content.ends_with('\n') && !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered
}

pub(super) fn apply_line_window(
    content: &str,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    language: ReadLanguage,
) -> String {
    if let Some(tail) = tail_lines {
        if tail == 0 {
            return String::new();
        }
        let lines = content.lines().collect::<Vec<_>>();
        let start = lines.len().saturating_sub(tail);
        let mut result = lines[start..].join("\n");
        if content.ends_with('\n') {
            result.push('\n');
        }
        return result;
    }

    if let Some(max) = max_lines {
        return smart_truncate_for_read(content, max, language);
    }

    content.to_string()
}

fn smart_truncate_for_read(content: &str, max_lines: usize, language: ReadLanguage) -> String {
    if max_lines == 0 {
        return String::new();
    }
    let lines = content.lines().collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return content.to_string();
    }
    let mut rendered = lines
        .iter()
        .take(max_lines)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    rendered.push('\n');
    rendered.push_str(&format!(
        "... {} more lines ({:?})\n",
        lines.len() - max_lines,
        language
    ));
    rendered
}

pub(super) struct SmartSourceSummary {
    pub(super) line1: String,
    pub(super) line2: String,
}

pub(super) fn summarize_source_file(content: &str, language: ReadLanguage) -> SmartSourceSummary {
    let line_count = content.lines().count();
    let functions = extract_source_symbols(content, language, SourceSymbolKind::Function);
    let structs = extract_source_symbols(content, language, SourceSymbolKind::Type);
    let traits = extract_source_symbols(content, language, SourceSymbolKind::Trait);
    let imports = extract_source_imports(content, language);
    let patterns = detect_source_patterns(content, language);

    let language_name = language_display_name(language);
    let subject = if !structs.is_empty() && !functions.is_empty() {
        format!("{language_name} module")
    } else if !structs.is_empty() {
        format!("{language_name} data structures")
    } else if !functions.is_empty() {
        format!("{language_name} functions")
    } else {
        format!("{language_name} code")
    };

    let mut components = Vec::new();
    if !functions.is_empty() {
        components.push(format!("{} fn", functions.len()));
    }
    if !structs.is_empty() {
        components.push(format!("{} type", structs.len()));
    }
    if !traits.is_empty() {
        components.push(format!("{} trait", traits.len()));
    }
    let line1 = if components.is_empty() {
        format!("{subject} ({line_count} lines)")
    } else {
        format!("{subject} ({}) - {line_count} lines", components.join(", "))
    };

    let mut details = Vec::new();
    if !imports.is_empty() {
        details.push(format!(
            "uses: {}",
            imports.into_iter().take(3).collect::<Vec<_>>().join(", ")
        ));
    }
    if !patterns.is_empty() {
        details.push(format!("patterns: {}", patterns.join(", ")));
    }
    if details.is_empty() && !functions.is_empty() {
        details.push(format!(
            "defines: {}",
            functions.into_iter().take(3).collect::<Vec<_>>().join(", ")
        ));
    }
    SmartSourceSummary {
        line1,
        line2: if details.is_empty() {
            "General purpose code file".to_string()
        } else {
            details.join(" | ")
        },
    }
}

#[derive(Clone, Copy)]
enum SourceSymbolKind {
    Function,
    Type,
    Trait,
}

fn extract_source_symbols(
    content: &str,
    language: ReadLanguage,
    kind: SourceSymbolKind,
) -> Vec<String> {
    let pattern = match (language, kind) {
        (ReadLanguage::Rust, SourceSymbolKind::Function) => {
            r"(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"
        }
        (ReadLanguage::Rust, SourceSymbolKind::Type) => {
            r"(?:pub\s+)?(?:struct|enum)\s+([A-Za-z_][A-Za-z0-9_]*)"
        }
        (ReadLanguage::Rust, SourceSymbolKind::Trait) => {
            r"(?:pub\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)"
        }
        (ReadLanguage::Python, SourceSymbolKind::Function) => r"def\s+([A-Za-z_][A-Za-z0-9_]*)",
        (ReadLanguage::Python, SourceSymbolKind::Type) => r"class\s+([A-Za-z_][A-Za-z0-9_]*)",
        (ReadLanguage::TypeScript, SourceSymbolKind::Function)
        | (ReadLanguage::JavaScript, SourceSymbolKind::Function) => {
            r"(?:async\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)|(?:const|let|var)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(?:async\s+)?\("
        }
        (ReadLanguage::TypeScript, SourceSymbolKind::Type) => {
            r"(?:interface|class|type)\s+([A-Za-z_][A-Za-z0-9_]*)"
        }
        (ReadLanguage::JavaScript, SourceSymbolKind::Type) => r"class\s+([A-Za-z_][A-Za-z0-9_]*)",
        (ReadLanguage::Go, SourceSymbolKind::Function) => {
            r"func\s+(?:\([^)]+\)\s+)?([A-Za-z_][A-Za-z0-9_]*)"
        }
        (ReadLanguage::Go, SourceSymbolKind::Type) => r"type\s+([A-Za-z_][A-Za-z0-9_]*)\s+struct",
        _ => return Vec::new(),
    };
    let re = compile_smart_regex(pattern);
    re.captures_iter(content)
        .filter_map(|caps| caps.get(1).or_else(|| caps.get(2)))
        .map(|item| item.as_str().to_string())
        .filter(|name| !matches!(name.as_str(), "main" | "new") && !name.starts_with("test_"))
        .take(10)
        .collect()
}

fn extract_source_imports(content: &str, language: ReadLanguage) -> Vec<String> {
    let pattern = match language {
        ReadLanguage::Rust => r"^use\s+([A-Za-z_][A-Za-z0-9_]*)",
        ReadLanguage::Python => r"^(?:from\s+(\S+)|import\s+(\S+))",
        ReadLanguage::JavaScript | ReadLanguage::TypeScript => {
            r#"(?:import.*from\s+['"]([^'"]+)['"]|require\(['"]([^'"]+)['"]\))"#
        }
        ReadLanguage::Go => r#"^\s*"([^"]+)"$"#,
        _ => return Vec::new(),
    };
    let re = compile_smart_regex(pattern);
    let mut seen = HashSet::new();
    let mut imports = Vec::new();
    for caps in re.captures_iter(content) {
        let Some(raw) = caps.get(1).or_else(|| caps.get(2)) else {
            continue;
        };
        let base = raw.as_str().split("::").next().unwrap_or(raw.as_str());
        if is_standard_import(base, language) || !seen.insert(base.to_string()) {
            continue;
        }
        imports.push(base.to_string());
    }
    imports.into_iter().take(5).collect()
}

#[expect(
    clippy::expect_used,
    reason = "callers select only compile-time regex literals covered by smart-read tests"
)]
fn compile_smart_regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("hard-coded smart-read regex must compile")
}

fn is_standard_import(name: &str, language: ReadLanguage) -> bool {
    match language {
        ReadLanguage::Rust => matches!(name, "std" | "core" | "alloc"),
        ReadLanguage::Python => matches!(name, "os" | "sys" | "re" | "json" | "typing"),
        _ => false,
    }
}

fn detect_source_patterns(content: &str, language: ReadLanguage) -> Vec<String> {
    let mut patterns = Vec::new();
    if content.contains("async") && content.contains("await") {
        patterns.push("async");
    }
    match language {
        ReadLanguage::Rust => {
            if content.contains("impl") && content.contains(" for ") {
                patterns.push("trait impl");
            }
            if content.contains("#[derive") {
                patterns.push("derive");
            }
            if content.contains("Result<") || content.contains("anyhow::") {
                patterns.push("error handling");
            }
            if content.contains("#[test]") {
                patterns.push("tests");
            }
        }
        ReadLanguage::Python => {
            if content.contains("@dataclass") {
                patterns.push("dataclass");
            }
            if content.contains("def __init__") {
                patterns.push("OOP");
            }
        }
        ReadLanguage::JavaScript | ReadLanguage::TypeScript => {
            if content.contains("useState") || content.contains("useEffect") {
                patterns.push("React hooks");
            }
            if content.contains("export default") {
                patterns.push("ES modules");
            }
        }
        _ => {}
    }
    patterns.into_iter().take(3).map(str::to_string).collect()
}

fn language_display_name(language: ReadLanguage) -> &'static str {
    match language {
        ReadLanguage::Rust => "Rust",
        ReadLanguage::Python => "Python",
        ReadLanguage::JavaScript => "JavaScript",
        ReadLanguage::TypeScript => "TypeScript",
        ReadLanguage::Go => "Go",
        ReadLanguage::Shell => "Shell",
        ReadLanguage::Markdown => "Markdown",
        ReadLanguage::Json => "JSON",
        ReadLanguage::Toml => "TOML",
        ReadLanguage::Yaml => "YAML",
        ReadLanguage::Unknown => "Code",
    }
}

pub(super) fn format_with_line_numbers(content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let width = lines.len().max(1).to_string().len();
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        out.push_str(&format!(
            "{:>width$} | {}\n",
            index + 1,
            line,
            width = width
        ));
    }
    out
}
