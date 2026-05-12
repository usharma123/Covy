use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_manifest = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root")
        .join("Cargo.toml");

    println!("cargo:rerun-if-changed={}", workspace_manifest.display());

    let version = workspace_version(&workspace_manifest)
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").expect("package version"));
    println!("cargo:rustc-env=PACKET28_VERSION={version}");

    write_builtin_filters(&manifest_dir);
}

fn workspace_version(path: &PathBuf) -> Option<String> {
    let manifest = fs::read_to_string(path).ok()?;
    let mut in_workspace_package = false;

    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_workspace_package = line == "[workspace.package]";
            continue;
        }
        if !in_workspace_package || line.starts_with('#') || !line.starts_with("version") {
            continue;
        }
        let (_, value) = line.split_once('=')?;
        return Some(value.trim().trim_matches('"').to_string());
    }

    None
}

fn write_builtin_filters(manifest_dir: &Path) {
    let filters_dir = manifest_dir.join("src").join("filters");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    let dest = out_dir.join("builtin_filters.toml");

    println!("cargo:rerun-if-changed={}", filters_dir.display());

    let mut files = fs::read_dir(&filters_dir)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", filters_dir.display()))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
        .collect::<Vec<_>>();
    files.sort_by_key(|entry| entry.file_name());

    let mut combined = String::from("schema_version = 1\n\n");
    for entry in files {
        let path = entry.path();
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        combined.push_str(&format!(
            "# --- {} ---\n",
            entry.file_name().to_string_lossy()
        ));
        combined.push_str(&content);
        combined.push_str("\n\n");
    }

    let parsed = combined
        .parse::<toml::Value>()
        .unwrap_or_else(|err| panic!("invalid built-in filters TOML: {err}"));
    let mut seen = HashSet::new();
    if let Some(filters) = parsed.get("filters").and_then(|value| value.as_table()) {
        for name in filters.keys() {
            if !seen.insert(name.clone()) {
                panic!("duplicate built-in filter name: {name}");
            }
        }
    }

    fs::write(&dest, combined)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", dest.display()));
}
