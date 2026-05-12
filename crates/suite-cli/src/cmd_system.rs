use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

#[derive(Args)]
pub struct JsonArgs {
    /// JSON file to inspect. Reads stdin when omitted.
    pub file: Option<PathBuf>,

    /// Maximum nesting depth to render before eliding children
    #[arg(long, default_value_t = 5)]
    pub max_depth: usize,

    /// Render type/schema information without scalar values
    #[arg(long, alias = "keys-only")]
    pub schema_only: bool,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct DepsArgs {
    /// Project directory or file inside a project
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct EnvArgs {
    /// Only show variables whose names contain this text
    pub filter: Option<String>,

    /// Show sensitive values instead of masking them
    #[arg(long)]
    pub show_all: bool,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemOutput {
    command: String,
    summary: String,
    text: String,
}

pub fn run_json(args: JsonArgs) -> Result<i32> {
    let content = match &args.file {
        Some(path) => {
            validate_json_extension(path)?;
            fs::read_to_string(path)
                .with_context(|| format!("failed to read JSON file {}", path.display()))?
        }
        None => {
            let mut content = String::new();
            io::stdin()
                .lock()
                .read_to_string(&mut content)
                .context("failed to read JSON from stdin")?;
            content
        }
    };

    let text = if args.schema_only {
        filter_json_schema(&content, args.max_depth)?
    } else {
        filter_json_compact(&content, args.max_depth)?
    };
    let summary = summarize_rendered_lines("JSON", &text);
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 json".to_string(),
            summary,
            text,
        },
    )
}

pub fn run_deps(args: DepsArgs) -> Result<i32> {
    let dir = if args.path.is_file() {
        args.path.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        args.path.clone()
    };
    let text = summarize_deps_dir(&dir)?;
    let summary = summarize_rendered_lines("dependencies", &text);
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 deps".to_string(),
            summary,
            text,
        },
    )
}

pub fn run_env(args: EnvArgs) -> Result<i32> {
    let mut vars = env::vars().collect::<Vec<_>>();
    vars.sort_by(|a, b| a.0.cmp(&b.0));
    let text = render_env_vars(&vars, args.filter.as_deref(), args.show_all);
    let summary = summarize_rendered_lines("environment", &text);
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 env".to_string(),
            summary,
            text,
        },
    )
}

fn emit_system_output(json_output: bool, pretty: bool, output: SystemOutput) -> Result<i32> {
    if json_output {
        crate::cmd_common::emit_json(&serde_json::to_value(output)?, pretty)?;
    } else {
        print!("{}", output.text);
        if !output.text.ends_with('\n') {
            println!();
        }
    }
    Ok(0)
}

fn summarize_rendered_lines(label: &str, text: &str) -> String {
    format!("{label}: {} lines", text.lines().count())
}

fn validate_json_extension(path: &Path) -> Result<()> {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return Ok(());
    };
    let format_name = match ext {
        "toml" => Some("TOML"),
        "yaml" | "yml" => Some("YAML"),
        "xml" => Some("XML"),
        "csv" => Some("CSV"),
        "ini" => Some("INI"),
        "env" => Some("env"),
        "txt" => Some("plain text"),
        _ => None,
    };
    if let Some(format_name) = format_name {
        let mut message = format!(
            "{} is not a JSON file (detected {}). Use `Packet28 run cat` or `Packet28 deps` for non-JSON files.",
            path.display(),
            format_name
        );
        if ext == "toml" && path.file_name().is_some_and(|name| name == "Cargo.toml") {
            message.push_str(" Tip: use `Packet28 deps` for Cargo.toml.");
        }
        bail!("{message}");
    }
    Ok(())
}

pub fn filter_json_compact(json_str: &str, max_depth: usize) -> Result<String> {
    let value: Value = serde_json::from_str(json_str).context("failed to parse JSON")?;
    Ok(compact_json(&value, 0, max_depth))
}

fn compact_json(value: &Value, depth: usize, max_depth: usize) -> String {
    let indent = "  ".repeat(depth);
    if depth > max_depth {
        return format!("{indent}...");
    }
    match value {
        Value::Null => format!("{indent}null"),
        Value::Bool(value) => format!("{indent}{value}"),
        Value::Number(value) => format!("{indent}{value}"),
        Value::String(value) => {
            if value.chars().count() > 80 {
                let preview = value.chars().take(77).collect::<String>();
                format!("{indent}\"{preview}...\"")
            } else {
                format!("{indent}\"{value}\"")
            }
        }
        Value::Array(values) => compact_json_array(values, depth, max_depth),
        Value::Object(map) => {
            if map.is_empty() {
                return format!("{indent}{{}}");
            }
            let mut lines = vec![format!("{indent}{{")];
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            for (index, key) in keys.iter().enumerate() {
                let value = &map[*key];
                if is_scalar_json(value) {
                    let rendered = compact_json(value, 0, max_depth);
                    lines.push(format!("{indent}  {key}: {}", rendered.trim()));
                } else {
                    lines.push(format!("{indent}  {key}:"));
                    lines.push(compact_json(value, depth + 1, max_depth));
                }
                if index >= 20 {
                    lines.push(format!(
                        "{indent}  ... +{} more keys",
                        keys.len() - index - 1
                    ));
                    break;
                }
            }
            lines.push(format!("{indent}}}"));
            lines.join("\n")
        }
    }
}

fn compact_json_array(values: &[Value], depth: usize, max_depth: usize) -> String {
    let indent = "  ".repeat(depth);
    if values.is_empty() {
        return format!("{indent}[]");
    }
    if values.len() > 5 {
        let first = compact_json(&values[0], depth + 1, max_depth);
        return format!("{indent}[{}, ... +{} more]", first.trim(), values.len() - 1);
    }
    let rendered = values
        .iter()
        .map(|value| compact_json(value, depth + 1, max_depth))
        .collect::<Vec<_>>();
    if values.iter().all(is_scalar_json) {
        let inline = rendered
            .iter()
            .map(|value| value.trim())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{indent}[{inline}]")
    } else {
        let mut lines = vec![format!("{indent}[")];
        for value in rendered {
            lines.push(format!("{value},"));
        }
        lines.push(format!("{indent}]"));
        lines.join("\n")
    }
}

fn is_scalar_json(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

pub fn filter_json_schema(json_str: &str, max_depth: usize) -> Result<String> {
    let value: Value = serde_json::from_str(json_str).context("failed to parse JSON")?;
    Ok(extract_schema(&value, 0, max_depth))
}

fn extract_schema(value: &Value, depth: usize, max_depth: usize) -> String {
    let indent = "  ".repeat(depth);
    if depth > max_depth {
        return format!("{indent}...");
    }
    match value {
        Value::Null => format!("{indent}null"),
        Value::Bool(_) => format!("{indent}bool"),
        Value::Number(number) if number.is_i64() || number.is_u64() => format!("{indent}int"),
        Value::Number(_) => format!("{indent}float"),
        Value::String(value) => {
            if value.starts_with("http://") || value.starts_with("https://") {
                format!("{indent}url")
            } else if value.len() == 10 && value.chars().filter(|ch| *ch == '-').count() == 2 {
                format!("{indent}date?")
            } else if value.is_empty() {
                format!("{indent}string")
            } else {
                format!("{indent}string")
            }
        }
        Value::Array(values) => {
            if values.is_empty() {
                format!("{indent}[]")
            } else {
                let first = extract_schema(&values[0], depth + 1, max_depth);
                if values.len() == 1 {
                    format!("{indent}[\n{first}\n{indent}]")
                } else {
                    format!("{indent}[{}] ({})", first.trim(), values.len())
                }
            }
        }
        Value::Object(map) => {
            if map.is_empty() {
                return format!("{indent}{{}}");
            }
            let mut lines = vec![format!("{indent}{{")];
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            for (index, key) in keys.iter().enumerate() {
                let value = &map[*key];
                let rendered = extract_schema(value, depth + 1, max_depth);
                if is_scalar_json(value) {
                    lines.push(format!("{indent}  {key}: {}", rendered.trim()));
                } else {
                    lines.push(format!("{indent}  {key}:"));
                    lines.push(rendered);
                }
                if index >= 15 {
                    lines.push(format!(
                        "{indent}  ... +{} more keys",
                        keys.len() - index - 1
                    ));
                    break;
                }
            }
            lines.push(format!("{indent}}}"));
            lines.join("\n")
        }
    }
}

fn summarize_deps_dir(dir: &Path) -> Result<String> {
    let mut found = false;
    let mut out = String::new();

    let cargo = dir.join("Cargo.toml");
    if cargo.exists() {
        found = true;
        out.push_str("Rust (Cargo.toml):\n");
        out.push_str(&summarize_cargo(&cargo)?);
    }

    let package = dir.join("package.json");
    if package.exists() {
        found = true;
        out.push_str("Node.js (package.json):\n");
        out.push_str(&summarize_package_json(&package)?);
    }

    let requirements = dir.join("requirements.txt");
    if requirements.exists() {
        found = true;
        out.push_str("Python (requirements.txt):\n");
        out.push_str(&summarize_requirements(&requirements)?);
    }

    let pyproject = dir.join("pyproject.toml");
    if pyproject.exists() {
        found = true;
        out.push_str("Python (pyproject.toml):\n");
        out.push_str(&summarize_pyproject(&pyproject)?);
    }

    let gomod = dir.join("go.mod");
    if gomod.exists() {
        found = true;
        out.push_str("Go (go.mod):\n");
        out.push_str(&summarize_gomod(&gomod)?);
    }

    if !found {
        out.push_str(&format!("No dependency files found in {}\n", dir.display()));
    }
    Ok(out)
}

fn summarize_cargo(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let dep_re = Regex::new(r#"^([a-zA-Z0-9_-]+)\s*=\s*(?:"([^"]+)"|.*version\s*=\s*"([^"]+)")"#)?;
    let section_re = Regex::new(r"^\[([^\]]+)\]")?;
    let mut section = String::new();
    let mut deps = Vec::new();
    let mut dev_deps = Vec::new();
    for line in content.lines() {
        if let Some(caps) = section_re.captures(line) {
            section = caps
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
        } else if let Some(caps) = dep_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps
                .get(2)
                .or(caps.get(3))
                .map(|m| m.as_str())
                .unwrap_or("*");
            match section.as_str() {
                "dependencies" => deps.push(format!("{name} ({version})")),
                "dev-dependencies" => dev_deps.push(format!("{name} ({version})")),
                _ => {}
            }
        }
    }
    Ok(render_dep_sections(
        &[("Dependencies", deps, 10), ("Dev", dev_deps, 5)],
        false,
    ))
}

fn summarize_package_json(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&content)?;
    let mut out = String::new();
    if let Some(name) = value.get("name").and_then(Value::as_str) {
        let version = value.get("version").and_then(Value::as_str).unwrap_or("?");
        out.push_str(&format!("  {name} @ {version}\n"));
    }
    if let Some(deps) = value.get("dependencies").and_then(Value::as_object) {
        let items = deps
            .iter()
            .map(|(name, version)| format!("{} ({})", name, version.as_str().unwrap_or("*")))
            .collect::<Vec<_>>();
        out.push_str(&render_dep_section("Dependencies", &items, 10, true));
    }
    if let Some(dev_deps) = value.get("devDependencies").and_then(Value::as_object) {
        let items = dev_deps.keys().cloned().collect::<Vec<_>>();
        out.push_str(&render_dep_section("Dev Dependencies", &items, 5, true));
    }
    Ok(out)
}

fn summarize_requirements(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let dep_re = Regex::new(r"^([a-zA-Z0-9_-]+)([=<>!~]+.*)?$")?;
    let deps = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let caps = dep_re.captures(line)?;
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            Some(format!("{name}{version}"))
        })
        .collect::<Vec<_>>();
    Ok(render_dep_section("Packages", &deps, 15, true))
}

fn summarize_pyproject(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let mut deps = Vec::new();
    let mut in_deps = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("dependencies") && trimmed.contains('[') {
            in_deps = true;
            continue;
        }
        if in_deps {
            if trimmed == "]" {
                break;
            }
            let dep = trimmed.trim_matches(|ch| ch == '"' || ch == '\'' || ch == ',');
            if !dep.is_empty() {
                deps.push(dep.to_string());
            }
        }
    }
    Ok(render_dep_section("Dependencies", &deps, 10, false))
}

fn summarize_gomod(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let mut module = String::new();
    let mut go_version = String::new();
    let mut deps = Vec::new();
    let mut in_require = false;
    for line in content.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix("module ") {
            module = name.to_string();
        } else if let Some(version) = line.strip_prefix("go ") {
            go_version = version.to_string();
        } else if line == "require (" {
            in_require = true;
        } else if line == ")" {
            in_require = false;
        } else if in_require && !line.starts_with("//") {
            let parts = line.split_whitespace().collect::<Vec<_>>();
            if parts.len() >= 2 {
                deps.push(format!("{} {}", parts[0], parts[1]));
            }
        } else if let Some(dep) = line.strip_prefix("require ") {
            if !dep.contains('(') {
                deps.push(dep.to_string());
            }
        }
    }
    let mut out = String::new();
    if !module.is_empty() {
        out.push_str(&format!("  {module} (go {go_version})\n"));
    }
    out.push_str(&render_dep_section("Dependencies", &deps, 10, false));
    Ok(out)
}

fn render_dep_sections(sections: &[(&str, Vec<String>, usize)], blank_between: bool) -> String {
    let mut out = String::new();
    for (index, (label, items, limit)) in sections.iter().enumerate() {
        if blank_between && index > 0 && !items.is_empty() {
            out.push('\n');
        }
        out.push_str(&render_dep_section(label, items, *limit, false));
    }
    out
}

fn render_dep_section(label: &str, items: &[String], limit: usize, include_empty: bool) -> String {
    if items.is_empty() && !include_empty {
        return String::new();
    }
    let mut out = format!("  {label} ({}):\n", items.len());
    for item in items.iter().take(limit) {
        out.push_str(&format!("    {item}\n"));
    }
    if items.len() > limit {
        out.push_str(&format!("    ... +{} more\n", items.len() - limit));
    }
    out
}

fn render_env_vars(vars: &[(String, String)], filter: Option<&str>, show_all: bool) -> String {
    let sensitive_patterns = sensitive_patterns();
    let filter = filter.map(str::to_lowercase);
    let mut path_vars = Vec::new();
    let mut lang_vars = Vec::new();
    let mut cloud_vars = Vec::new();
    let mut tool_vars = Vec::new();
    let mut other_vars = Vec::new();

    for (key, value) in vars {
        if let Some(filter) = &filter {
            if !key.to_lowercase().contains(filter) {
                continue;
            }
        }
        let sensitive = sensitive_patterns
            .iter()
            .any(|pattern| key.to_lowercase().contains(pattern));
        let display_value = if sensitive && !show_all {
            mask_value(value)
        } else if value.chars().count() > 100 {
            let preview = value.chars().take(50).collect::<String>();
            format!("{preview}... ({} chars)", value.chars().count())
        } else {
            value.clone()
        };
        let entry = (key.clone(), display_value);
        if key.contains("PATH") {
            path_vars.push(entry);
        } else if is_lang_var(key) {
            lang_vars.push(entry);
        } else if is_cloud_var(key) {
            cloud_vars.push(entry);
        } else if is_tool_var(key) {
            tool_vars.push(entry);
        } else if filter.is_some() || is_interesting_var(key) {
            other_vars.push(entry);
        }
    }

    let mut out = String::new();
    append_path_vars(&mut out, &path_vars);
    append_env_section(&mut out, "Language/Runtime", &lang_vars, None);
    append_env_section(&mut out, "Cloud/Services", &cloud_vars, None);
    append_env_section(&mut out, "Tools", &tool_vars, None);
    append_env_section(&mut out, "Other", &other_vars, Some(20));
    if filter.is_none() {
        let shown = path_vars.len()
            + lang_vars.len()
            + cloud_vars.len()
            + tool_vars.len()
            + other_vars.len().min(20);
        out.push_str(&format!(
            "\nTotal: {} vars (showing {} relevant)\n",
            vars.len(),
            shown
        ));
    }
    out
}

fn append_path_vars(out: &mut String, vars: &[(String, String)]) {
    if vars.is_empty() {
        return;
    }
    out.push_str("PATH Variables:\n");
    for (key, value) in vars {
        if key == "PATH" {
            let paths = value.split(':').collect::<Vec<_>>();
            out.push_str(&format!("  PATH ({} entries):\n", paths.len()));
            for path in paths.iter().take(5) {
                out.push_str(&format!("    {path}\n"));
            }
            if paths.len() > 5 {
                out.push_str(&format!("    ... +{} more\n", paths.len() - 5));
            }
        } else {
            out.push_str(&format!("  {key}={value}\n"));
        }
    }
}

fn append_env_section(
    out: &mut String,
    label: &str,
    vars: &[(String, String)],
    limit: Option<usize>,
) {
    if vars.is_empty() {
        return;
    }
    out.push_str(&format!("\n{label}:\n"));
    let limit = limit.unwrap_or(vars.len());
    for (key, value) in vars.iter().take(limit) {
        out.push_str(&format!("  {key}={value}\n"));
    }
    if vars.len() > limit {
        out.push_str(&format!("  ... +{} more\n", vars.len() - limit));
    }
}

fn sensitive_patterns() -> HashSet<&'static str> {
    [
        "key",
        "secret",
        "password",
        "token",
        "credential",
        "auth",
        "private",
        "api_key",
        "apikey",
        "access_key",
        "jwt",
    ]
    .into_iter()
    .collect()
}

fn mask_value(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= 4 {
        "****".to_string()
    } else {
        let prefix = chars.iter().take(2).collect::<String>();
        let suffix = chars
            .iter()
            .skip(chars.len().saturating_sub(2))
            .collect::<String>();
        format!("{prefix}****{suffix}")
    }
}

fn is_lang_var(key: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "RUST", "CARGO", "PYTHON", "PIP", "NODE", "NPM", "YARN", "DENO", "BUN", "JAVA", "MAVEN",
        "GRADLE", "GO", "GOPATH", "GOROOT", "RUBY", "GEM", "PERL", "PHP", "DOTNET", "NUGET",
    ];
    let key = key.to_uppercase();
    PATTERNS.iter().any(|pattern| key.contains(pattern))
}

fn is_cloud_var(key: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "AWS",
        "AZURE",
        "GCP",
        "GOOGLE_CLOUD",
        "DOCKER",
        "KUBERNETES",
        "K8S",
        "HELM",
        "TERRAFORM",
        "VAULT",
        "CONSUL",
        "NOMAD",
    ];
    let key = key.to_uppercase();
    PATTERNS.iter().any(|pattern| key.contains(pattern))
}

fn is_tool_var(key: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "EDITOR",
        "VISUAL",
        "SHELL",
        "TERM",
        "GIT",
        "SSH",
        "GPG",
        "BREW",
        "HOMEBREW",
        "XDG",
        "CLAUDE",
        "ANTHROPIC",
    ];
    let key = key.to_uppercase();
    PATTERNS.iter().any(|pattern| key.contains(pattern))
}

fn is_interesting_var(key: &str) -> bool {
    const PATTERNS: &[&str] = &["HOME", "USER", "LANG", "LC_", "TZ", "PWD", "OLDPWD"];
    let key = key.to_uppercase();
    PATTERNS.iter().any(|pattern| key.starts_with(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_compact_truncates_values_and_summarizes_arrays() {
        let rendered = filter_json_compact(
            &serde_json::json!({
                "long": "x".repeat(100),
                "items": [1, 2, 3, 4, 5, 6],
            })
            .to_string(),
            5,
        )
        .unwrap();
        assert!(rendered.contains("long: \""));
        assert!(rendered.contains("...\""));
        assert!(rendered.contains("[1, ... +5 more]"));
    }

    #[test]
    fn json_schema_renders_types_without_values() {
        let rendered = filter_json_schema(
            r#"{"count":42,"items":[{"url":"https://example.com","enabled":true}]}"#,
            5,
        )
        .unwrap();
        assert!(rendered.contains("count: int"));
        assert!(rendered.contains("enabled: bool"));
        assert!(rendered.contains("url: url"));
        assert!(!rendered.contains("example.com"));
    }

    #[test]
    fn json_extension_rejects_cargo_toml_with_deps_hint() {
        let err = validate_json_extension(Path::new("Cargo.toml")).unwrap_err();
        assert!(err.to_string().contains("not a JSON file"));
        assert!(err.to_string().contains("Packet28 deps"));
    }

    #[test]
    fn env_masks_sensitive_values_and_limits_path() {
        let vars = vec![
            (
                "AWS_SECRET_ACCESS_KEY".to_string(),
                "supersecrettoken".to_string(),
            ),
            ("PATH".to_string(), "/a:/b:/c:/d:/e:/f".to_string()),
            ("RUST_LOG".to_string(), "debug".to_string()),
        ];
        let rendered = render_env_vars(&vars, None, false);
        assert!(rendered.contains("AWS_SECRET_ACCESS_KEY=su****en"));
        assert!(rendered.contains("PATH (6 entries)"));
        assert!(rendered.contains("... +1 more"));
        assert!(rendered.contains("RUST_LOG=debug"));
    }

    #[test]
    fn env_filter_includes_other_matching_vars() {
        let vars = vec![("CUSTOM_TOKEN".to_string(), "abcde".to_string())];
        let rendered = render_env_vars(&vars, Some("custom"), false);
        assert!(rendered.contains("CUSTOM_TOKEN=ab****de"));
        assert!(!rendered.contains("Total:"));
    }

    #[test]
    fn deps_summarizes_multiple_manifest_types() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[dependencies]\nanyhow = \"1\"\nserde = { version = \"1\" }\n[dev-dependencies]\ntempfile = \"3\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"demo","version":"0.1.0","dependencies":{"react":"18"},"devDependencies":{"vite":"5"}}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("requirements.txt"),
            "pytest==8\n# ignored\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/demo\ngo 1.22\nrequire github.com/acme/lib v1.2.3\n",
        )
        .unwrap();
        let rendered = summarize_deps_dir(dir.path()).unwrap();
        assert!(rendered.contains("Rust (Cargo.toml):"));
        assert!(rendered.contains("anyhow (1)"));
        assert!(rendered.contains("Node.js (package.json):"));
        assert!(rendered.contains("demo @ 0.1.0"));
        assert!(rendered.contains("pytest==8"));
        assert!(rendered.contains("example.com/demo (go 1.22)"));
    }
}
