use std::fs;
use std::path::Path;

use anyhow::Result;
use regex::Regex;
use serde_json::Value;

pub(super) fn summarize_deps_dir(dir: &Path) -> Result<String> {
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
