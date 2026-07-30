//! Repository-relative input normalization and index artifact paths.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{Result, SearchError};
use crate::model::{
    GENERATION_HIGH_WATER_FILE_NAME, MANIFEST_FILE_NAME, OVERLAY_STATE_FILE_NAME,
    PREVIOUS_MANIFEST_FILE_NAME, REGEX_DIR_NAME,
};

pub(crate) fn regex_index_dir(root: &Path) -> PathBuf {
    root.join(".packet28").join("index").join(REGEX_DIR_NAME)
}

pub(crate) fn overlay_state_path(root: &Path) -> PathBuf {
    regex_index_dir(root).join(OVERLAY_STATE_FILE_NAME)
}

pub(crate) fn manifest_path(root: &Path) -> PathBuf {
    regex_index_dir(root).join(MANIFEST_FILE_NAME)
}

pub(crate) fn previous_manifest_path(root: &Path) -> PathBuf {
    regex_index_dir(root).join(PREVIOUS_MANIFEST_FILE_NAME)
}

pub(crate) fn generation_high_water_path(root: &Path) -> PathBuf {
    root.join(".packet28")
        .join("index")
        .join(GENERATION_HIGH_WATER_FILE_NAME)
}

pub(crate) fn generation_record_path(root: &Path, generation: u64) -> PathBuf {
    regex_index_dir(root).join(format!("generation-{generation:020}.json"))
}

#[derive(Debug)]
pub(crate) enum WorkspacePathKind {
    Missing,
    File(fs::Metadata),
    Directory,
    Symlink,
    Other,
}

#[derive(Debug)]
pub(crate) struct WorkspacePathInspection {
    pub(crate) full_path: PathBuf,
    pub(crate) kind: WorkspacePathKind,
}

pub(crate) fn inspect_workspace_path(
    root: &Path,
    relative: &Path,
) -> io::Result<WorkspacePathInspection> {
    let mut full_path = fs::canonicalize(root)?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace path is not repository-relative",
        ));
    }
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            unreachable!("components were validated above");
        };
        full_path.push(name);
        let last = index + 1 == components.len();
        match fs::symlink_metadata(&full_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Ok(WorkspacePathInspection {
                    full_path,
                    kind: WorkspacePathKind::Symlink,
                });
            }
            Ok(metadata) if !last && metadata.is_dir() => {}
            Ok(_) if !last => {
                return Ok(WorkspacePathInspection {
                    full_path,
                    kind: WorkspacePathKind::Other,
                });
            }
            Ok(metadata) if metadata.is_file() => {
                return Ok(WorkspacePathInspection {
                    full_path,
                    kind: WorkspacePathKind::File(metadata),
                });
            }
            Ok(metadata) if metadata.is_dir() => {
                return Ok(WorkspacePathInspection {
                    full_path,
                    kind: WorkspacePathKind::Directory,
                });
            }
            Ok(_) => {
                return Ok(WorkspacePathInspection {
                    full_path,
                    kind: WorkspacePathKind::Other,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(WorkspacePathInspection {
                    full_path,
                    kind: WorkspacePathKind::Missing,
                });
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("a non-empty path has a final component")
}

pub(crate) fn normalize_changed_paths(root: &Path, paths: &[String]) -> Result<Vec<String>> {
    let canonical_root = fs::canonicalize(root)?;
    let mut normalized = BTreeSet::new();
    for raw in paths {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.contains('\n') {
            return Err(SearchError::InvalidChangedPath { path: raw.clone() });
        }
        let input = Path::new(trimmed);
        let relative = if input.is_absolute() {
            if let Ok(stripped) = input.strip_prefix(root) {
                stripped.to_path_buf()
            } else if input.exists() {
                fs::canonicalize(input)
                    .ok()
                    .and_then(|path| {
                        path.strip_prefix(&canonical_root)
                            .ok()
                            .map(Path::to_path_buf)
                    })
                    .ok_or_else(|| SearchError::InvalidChangedPath { path: raw.clone() })?
            } else {
                return Err(SearchError::InvalidChangedPath { path: raw.clone() });
            }
        } else {
            input.to_path_buf()
        };
        let mut safe = PathBuf::new();
        for component in relative.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(part) => safe.push(part),
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(SearchError::InvalidChangedPath { path: raw.clone() });
                }
            }
        }
        if safe.as_os_str().is_empty() {
            continue;
        }
        let inspection = inspect_workspace_path(&canonical_root, &safe)?;
        if matches!(
            inspection.kind,
            WorkspacePathKind::Symlink | WorkspacePathKind::Other
        ) {
            return Err(SearchError::InvalidChangedPath { path: raw.clone() });
        }
        normalized.insert(safe.to_string_lossy().replace('\\', "/"));
    }
    Ok(normalized.into_iter().collect())
}

pub(crate) fn resolve_requested_paths(
    root: &Path,
    requested_paths: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut resolved = Vec::new();
    let mut diagnostics = Vec::new();
    let mut seen = BTreeSet::new();
    for original in requested_paths {
        let normalized = normalize_capture_path(root, original);
        if normalized.is_empty() {
            let trimmed = original.trim();
            if !matches!(trimmed, "." | "./") {
                diagnostics.push(format!("ignored invalid path input: {trimmed}"));
            }
            continue;
        }
        let direct = inspect_workspace_path(root, Path::new(&normalized));
        let final_path = match direct {
            Ok(WorkspacePathInspection {
                kind: WorkspacePathKind::File(_) | WorkspacePathKind::Directory,
                ..
            }) => normalized,
            Ok(WorkspacePathInspection {
                kind: WorkspacePathKind::Missing,
                ..
            }) => {
                let Some(candidate) = resolve_capture_path_suffix(root, &normalized) else {
                    diagnostics.push(format!(
                        "path '{}' does not exist under daemon root {}",
                        original.trim(),
                        root.display()
                    ));
                    continue;
                };
                diagnostics.push(format!(
                    "resolved missing path '{}' to '{}'",
                    original.trim(),
                    candidate
                ));
                candidate
            }
            Ok(_) => {
                diagnostics.push(format!("ignored unsafe path input: {}", original.trim()));
                continue;
            }
            Err(error) => {
                diagnostics.push(format!(
                    "failed to inspect path '{}': {error}",
                    original.trim()
                ));
                continue;
            }
        };
        if seen.insert(final_path.clone()) {
            resolved.push(final_path);
        }
    }
    (resolved, diagnostics)
}

fn normalize_capture_path(root: &Path, text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || matches!(trimmed, "." | "./") {
        return String::new();
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        if let Ok(stripped) = path.strip_prefix(root) {
            return stripped
                .to_string_lossy()
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_string();
        }
    }
    trimmed
        .trim_start_matches("./")
        .trim_start_matches('/')
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string()
}

fn resolve_capture_path_suffix(root: &Path, needle: &str) -> Option<String> {
    let mut matches = BTreeSet::new();
    collect_suffix_matches(root, root, needle, &mut matches);
    (matches.len() == 1)
        .then(|| matches.into_iter().next())
        .flatten()
}

fn collect_suffix_matches(
    root: &Path,
    current: &Path,
    needle: &str,
    matches: &mut BTreeSet<String>,
) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_suffix_matches(root, &path, needle, matches);
            if matches.len() > 1 {
                return;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Ok(stripped) = path.strip_prefix(root) else {
            continue;
        };
        let normalized = stripped.to_string_lossy().replace('\\', "/");
        if normalized == needle || normalized.ends_with(&format!("/{needle}")) {
            matches.insert(normalized);
            if matches.len() > 1 {
                return;
            }
        }
    }
}
