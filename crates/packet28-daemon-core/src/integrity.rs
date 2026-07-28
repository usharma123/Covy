//! Hook integrity verification via BLAKE3 hashing.
//!
//! When a hook script is installed, its BLAKE3 hash is stored in a
//! hidden `.packet28-hash` sidecar file. On subsequent runs, the
//! hash is re-computed and compared to detect tampering.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{DaemonCoreError, Result};

/// Result of an integrity check on a hook file.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityStatus {
    /// Hash matches the stored baseline.
    Verified,
    /// Hash does NOT match the stored baseline.
    Tampered,
    /// No baseline hash has been stored yet.
    NoBaseline,
    /// The hook file itself does not exist.
    NotInstalled,
    /// A hash sidecar exists but the hook file is missing.
    OrphanedHash,
}

/// Compute the BLAKE3 hash of a file.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if `path` cannot be read.
pub fn compute_hash(path: &Path) -> Result<String> {
    let contents =
        fs::read(path).map_err(|source| DaemonCoreError::io("failed to read", path, source))?;
    let hash = blake3::hash(&contents);
    Ok(hash.to_hex().to_string())
}

/// Get the path to the sidecar hash file for a hook.
fn hash_sidecar_path(hook_path: &Path) -> std::path::PathBuf {
    let mut sidecar = hook_path.to_path_buf();
    let name = sidecar
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("hook")
        .to_string();
    sidecar.set_file_name(format!(".{name}.packet28-hash"));
    sidecar
}

/// Store the current hash of a hook file as its baseline.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the hook cannot be read or its sidecar
/// cannot be written.
pub fn store_hash(hook_path: &Path) -> Result<()> {
    let hash = compute_hash(hook_path)?;
    let sidecar = hash_sidecar_path(hook_path);
    fs::write(&sidecar, &hash).map_err(|source| {
        DaemonCoreError::io("failed to write hook hash sidecar", &sidecar, source)
    })?;
    Ok(())
}

/// Verify a hook file against its stored baseline hash.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the installed hook or existing sidecar
/// cannot be read.
pub fn verify_hook(hook_path: &Path) -> Result<IntegrityStatus> {
    let sidecar = hash_sidecar_path(hook_path);
    let hook_exists = hook_path.exists();
    let sidecar_exists = sidecar.exists();

    match (hook_exists, sidecar_exists) {
        (false, false) => Ok(IntegrityStatus::NotInstalled),
        (false, true) => Ok(IntegrityStatus::OrphanedHash),
        (true, false) => Ok(IntegrityStatus::NoBaseline),
        (true, true) => {
            let current = compute_hash(hook_path)?;
            let stored = fs::read_to_string(&sidecar).map_err(|source| {
                DaemonCoreError::io("failed to read hook hash sidecar", &sidecar, source)
            })?;
            if current.trim() == stored.trim() {
                Ok(IntegrityStatus::Verified)
            } else {
                Ok(IntegrityStatus::Tampered)
            }
        }
    }
}

/// Verify all hook files in a directory.
///
/// A missing path or a path that is not a directory produces an empty result.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the directory cannot be enumerated or a
/// hook or sidecar discovered within it cannot be read.
pub fn verify_hooks_in_dir(dir: &Path) -> Result<Vec<(String, IntegrityStatus)>> {
    let mut results = Vec::new();
    if !dir.is_dir() {
        return Ok(results);
    }
    let entries = fs::read_dir(dir)
        .map_err(|source| DaemonCoreError::io("failed to read directory", dir, source))?;
    for entry in entries {
        let entry = entry.map_err(|source| {
            DaemonCoreError::io("failed to read directory entry from", dir, source)
        })?;
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        // Skip sidecar files and hidden files
        if name.starts_with('.') || name.ends_with(".packet28-hash") {
            continue;
        }
        if path.is_file() {
            let status = verify_hook(&path)?;
            results.push((name, status));
        }
    }
    results.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_hash_returns_hex_string() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-hook.sh");
        fs::write(&path, "#!/bin/bash\necho hello").unwrap();
        let hash = compute_hash(&path).unwrap();
        assert!(!hash.is_empty());
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn store_and_verify_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook.sh");
        fs::write(&path, "#!/bin/bash\necho test").unwrap();

        store_hash(&path).unwrap();
        assert_eq!(verify_hook(&path).unwrap(), IntegrityStatus::Verified);
    }

    #[test]
    fn verify_detects_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook.sh");
        fs::write(&path, "#!/bin/bash\necho original").unwrap();

        store_hash(&path).unwrap();
        fs::write(&path, "#!/bin/bash\necho tampered").unwrap();

        assert_eq!(verify_hook(&path).unwrap(), IntegrityStatus::Tampered);
    }

    #[test]
    fn verify_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.sh");
        assert_eq!(verify_hook(&path).unwrap(), IntegrityStatus::NotInstalled);
    }

    #[test]
    fn verify_no_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook.sh");
        fs::write(&path, "#!/bin/bash").unwrap();
        assert_eq!(verify_hook(&path).unwrap(), IntegrityStatus::NoBaseline);
    }

    #[test]
    fn verify_orphaned_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook.sh");
        fs::write(&path, "content").unwrap();
        store_hash(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(verify_hook(&path).unwrap(), IntegrityStatus::OrphanedHash);
    }
}
