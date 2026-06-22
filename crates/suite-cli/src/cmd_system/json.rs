use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

pub(super) fn validate_json_extension(path: &Path) -> Result<()> {
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

pub(super) fn filter_json_compact(json_str: &str, max_depth: usize) -> Result<String> {
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

pub(super) fn filter_json_schema(json_str: &str, max_depth: usize) -> Result<String> {
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
