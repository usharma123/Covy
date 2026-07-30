//! Repository-relative input normalization and index artifact paths.

use std::collections::BTreeSet;
use std::fs;
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
        let candidate = root.join(&safe);
        if candidate.exists() {
            let canonical_candidate = fs::canonicalize(&candidate)?;
            if !canonical_candidate.starts_with(&canonical_root) {
                return Err(SearchError::InvalidChangedPath { path: raw.clone() });
            }
        }
        normalized.insert(safe.to_string_lossy().replace('\\', "/"));
    }
    Ok(normalized.into_iter().collect())
}
