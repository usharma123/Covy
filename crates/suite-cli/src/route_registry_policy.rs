use std::path::{Path, PathBuf};

pub(super) fn policy_excludes_command(command: &str, argv: &[String], root: Option<&Path>) -> bool {
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

pub(super) fn configured_wrapper_prefix(
    argv: &[String],
    root: Option<&Path>,
) -> Option<Vec<String>> {
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

fn policy_config_paths(root: &Path) -> Vec<PathBuf> {
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

fn config_home_dir() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("XDG_CONFIG_HOME") {
        if !value.is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
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
