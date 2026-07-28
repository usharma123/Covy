use serde_json::Value;

pub(crate) fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn json_nested_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn json_array_strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(ToOwned::to_owned))
        .collect()
}

pub(crate) fn json_array_len(value: &Value, key: &str) -> Option<usize> {
    value.get(key).and_then(Value::as_array).map(Vec::len)
}

pub(crate) fn hook_output_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    for key in ["stdout", "stderr", "output", "text", "content", "error"] {
        if let Some(text) = json_string(value, key) {
            return text;
        }
    }
    serde_json::to_string(value).unwrap_or_else(|_| String::new())
}

pub(crate) fn first_nonempty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| compact_text(line, 160))
}

pub(crate) fn compact_text(value: &str, limit: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let char_count = compact.chars().count();
    if char_count <= limit {
        compact
    } else if limit <= 3 {
        "...".to_string()
    } else {
        let shortened = compact
            .chars()
            .take(limit.saturating_sub(3))
            .collect::<String>();
        format!("{shortened}...")
    }
}

pub(crate) fn response_failed(response: &Value) -> bool {
    response
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || response.get("error").is_some()
}

pub(crate) fn extract_command_paths(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .filter(|part| {
            part.contains('/')
                || part.ends_with(".rs")
                || part.ends_with(".md")
                || part.ends_with(".json")
                || part.ends_with(".toml")
        })
        .map(|part| {
            part.trim_matches(|ch| ch == '"' || ch == '\'' || ch == ',')
                .to_string()
        })
        .collect()
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

pub(crate) fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn payload_text_len(text: &str) -> usize {
    text.len()
}

pub(crate) fn estimate_text_tokens(text: &str) -> u64 {
    let bytes = text.len() as u64;
    if bytes == 0 {
        0
    } else {
        bytes.div_ceil(4)
    }
}

pub(crate) fn reduction_pct(raw_tokens: u64, reduced_tokens: u64) -> f64 {
    if raw_tokens == 0 {
        0.0
    } else {
        ((raw_tokens.saturating_sub(reduced_tokens)) as f64 * 100.0 / raw_tokens as f64 * 10.0)
            .round()
            / 10.0
    }
}

pub(crate) fn now_unix_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
