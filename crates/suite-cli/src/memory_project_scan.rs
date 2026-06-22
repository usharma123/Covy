use std::collections::BTreeSet;
use std::path::Path;

pub(crate) fn project_identity(root: &Path, fallback_name: &str) -> String {
    for file in ["Cargo.toml", "package.json", "pyproject.toml", "go.mod"] {
        let path = root.join(file);
        if path.exists() {
            return format!("Project {fallback_name} identified by {file}");
        }
    }
    format!("Project: {fallback_name}")
}

pub(crate) fn collect_project_dependencies(root: &Path) -> Vec<(String, String)> {
    let mut deps = BTreeSet::new();
    collect_manifest_dependencies(&root.join("Cargo.toml"), &mut deps);
    collect_manifest_dependencies(&root.join("package.json"), &mut deps);
    collect_manifest_dependencies(&root.join("pyproject.toml"), &mut deps);
    collect_go_dependencies(&root.join("go.mod"), &mut deps);
    deps.into_iter()
        .map(|dep| (dep.clone(), format!("Dependency: {dep}")))
        .collect()
}

fn collect_manifest_dependencies(path: &Path, deps: &mut BTreeSet<String>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('"') || line.starts_with('\'') || line.starts_with('[') {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            let name = name.trim().trim_matches('"').trim_matches('\'');
            if is_dependency_name(name) {
                deps.insert(name.to_string());
            }
        }
        if line.contains("\":") {
            let name = line
                .split_once(':')
                .map(|(name, _)| name.trim().trim_matches('"'))
                .unwrap_or_default();
            if is_dependency_name(name) {
                deps.insert(name.to_string());
            }
        }
    }
}

fn collect_go_dependencies(path: &Path, deps: &mut BTreeSet<String>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("require ") {
            if let Some(name) = rest.split_whitespace().next() {
                if is_dependency_name(name) {
                    deps.insert(name.to_string());
                }
            }
        }
    }
}

fn is_dependency_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(
            name,
            "package"
                | "dependencies"
                | "devDependencies"
                | "scripts"
                | "workspace"
                | "features"
                | "lib"
                | "bin"
                | "name"
                | "version"
                | "edition"
        )
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '@'))
}

pub(crate) fn collect_project_modules(root: &Path) -> Vec<(String, String)> {
    let mut modules = BTreeSet::new();
    for rel in ["src", "crates", "packages", "apps"] {
        let dir = root.join(rel);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                    modules.insert(format!("{rel}/{name}"));
                }
            }
        }
    }
    modules
        .into_iter()
        .map(|module| (module.clone(), format!("Project module: {module}")))
        .collect()
}

pub(crate) fn collect_project_entrypoints(root: &Path) -> Vec<(String, String)> {
    let mut entrypoints = BTreeSet::new();
    for rel in [
        "src/main.rs",
        "src/lib.rs",
        "src/index.ts",
        "src/index.tsx",
        "src/main.ts",
        "src/main.tsx",
        "main.py",
        "app.py",
    ] {
        if root.join(rel).exists() {
            entrypoints.insert(rel.to_string());
        }
    }
    entrypoints
        .into_iter()
        .map(|entry| (entry.clone(), format!("Project entrypoint: {entry}")))
        .collect()
}

pub(crate) fn collect_project_configs(root: &Path) -> Vec<(String, String)> {
    let mut configs = BTreeSet::new();
    for rel in [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "tsconfig.json",
        ".github/workflows",
        ".mcp.json",
        ".mcp.proxy.json",
    ] {
        if root.join(rel).exists() {
            configs.insert(rel.to_string());
        }
    }
    configs
        .into_iter()
        .map(|config| (config.clone(), format!("Project config: {config}")))
        .collect()
}
