//! Bounded Git workspace attestation for full builds, incremental updates, and queries.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path};

use crate::error::{Result, SearchError};
use crate::git_process::run_git;
use crate::model::{RegexIndexManifest, MAX_INDEXED_FILE_BYTES};
use crate::paths::{inspect_workspace_path, WorkspacePathInspection, WorkspacePathKind};

const MAX_ATTESTED_WORKSPACE_ENTRIES: usize = 4_096;
const MAX_ATTESTED_WORKSPACE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspacePathAttestation {
    digest: String,
    indexable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitWorkspaceSnapshot {
    pub(crate) commit: String,
    clean: bool,
    pub(crate) entries: BTreeMap<String, String>,
    reported: BTreeMap<String, WorkspacePathAttestation>,
}

impl GitWorkspaceSnapshot {
    pub(crate) fn ensure_indexed_bytes(
        &self,
        indexed_fingerprints: &BTreeMap<String, String>,
    ) -> Result<()> {
        let matches = self.reported.iter().all(|(path, attestation)| {
            if attestation.indexable {
                indexed_fingerprints.get(path) == Some(&attestation.digest)
            } else {
                !indexed_fingerprints.contains_key(path)
            }
        });
        if matches
            && indexed_fingerprints.len() == self.reported.values().filter(|v| v.indexable).count()
        {
            return Ok(());
        }
        Err(SearchError::IndexNotReady {
            reason: "incremental regex update did not index the authenticated workspace bytes"
                .to_string(),
        })
    }
}

pub(crate) fn git_workspace_snapshot(
    root: &Path,
    reported_paths: &[String],
) -> std::result::Result<GitWorkspaceSnapshot, String> {
    let status = run_git(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    let mut records = status.stdout.split(|byte| *byte == 0).peekable();
    let commit = records
        .clone()
        .find_map(|record| record.strip_prefix(b"# branch.oid "))
        .and_then(|value| std::str::from_utf8(value).ok())
        .filter(|value| *value != "(initial)")
        .map(str::to_string)
        .ok_or_else(|| "git status did not report a HEAD commit".to_string())?;
    let mut dirty_paths = BTreeSet::new();
    while let Some(record) = records.next() {
        let path = match record.first().copied() {
            None | Some(b'#') | Some(b'!') => continue,
            Some(b'1') => status_record_path(record, 9)?,
            Some(b'2') => {
                dirty_paths.insert(status_record_path(record, 10)?.to_string());
                let original = records
                    .next()
                    .ok_or_else(|| "git rename status omitted its original path".to_string())?;
                std::str::from_utf8(original)
                    .map_err(|_| "git status reported a non-UTF-8 path".to_string())?
            }
            Some(b'u') => status_record_path(record, 11)?,
            Some(b'?') if record.get(1) == Some(&b' ') => std::str::from_utf8(&record[2..])
                .map_err(|_| "git status reported a non-UTF-8 path".to_string())?,
            _ => return Err("git status reported an unsupported record".to_string()),
        };
        dirty_paths.insert(path.to_string());
    }
    let reported = reported_paths.iter().cloned().collect::<BTreeSet<_>>();
    if dirty_paths.union(&reported).count() > MAX_ATTESTED_WORKSPACE_ENTRIES {
        return Err(format!(
            "workspace attestation exceeds the {MAX_ATTESTED_WORKSPACE_ENTRIES}-path safety limit"
        ));
    }
    let tracked_reported = validate_index_flags(root, &reported)?;
    let mut budget = 0_u64;
    let mut states = BTreeMap::new();
    for path in dirty_paths.union(&reported) {
        validate_workspace_path(path)?;
        if !dirty_paths.contains(path) && !tracked_reported.contains(path) {
            return Err(format!(
                "reported workspace path '{path}' is neither Git-dirty nor tracked"
            ));
        }
        states.insert(
            path.clone(),
            attest_workspace_path(root, path, &mut budget)?,
        );
    }
    let entries = dirty_paths
        .iter()
        .map(|path| (path.clone(), states[path].digest.clone()))
        .collect();
    let reported = reported
        .into_iter()
        .map(|path| (path.clone(), states[&path].clone()))
        .collect();
    Ok(GitWorkspaceSnapshot {
        commit,
        clean: dirty_paths.is_empty(),
        entries,
        reported,
    })
}

fn validate_index_flags(
    root: &Path,
    reported: &BTreeSet<String>,
) -> std::result::Result<BTreeSet<String>, String> {
    let output = run_git(root, &["ls-files", "-v", "-z"])?;
    let mut tracked_reported = BTreeSet::new();
    for record in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|r| !r.is_empty())
    {
        let Some((&tag, path)) = record.split_first() else {
            continue;
        };
        let Some(path) = path.strip_prefix(b" ") else {
            return Err("git ls-files reported a malformed record".to_string());
        };
        if tag == b'S' || tag.is_ascii_lowercase() {
            return Err(format!(
                "Git index flag on '{}' prevents workspace authentication",
                String::from_utf8_lossy(path)
            ));
        }
        if let Ok(path) = std::str::from_utf8(path) {
            if reported.contains(path) {
                tracked_reported.insert(path.to_string());
            }
        }
    }
    Ok(tracked_reported)
}

fn status_record_path(record: &[u8], fields: usize) -> std::result::Result<&str, String> {
    let path = record
        .splitn(fields, |byte| *byte == b' ')
        .nth(fields - 1)
        .ok_or_else(|| "git status record omitted its path".to_string())?;
    std::str::from_utf8(path).map_err(|_| "git status reported a non-UTF-8 path".to_string())
}

fn validate_workspace_path(path: &str) -> std::result::Result<(), String> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("git status reported unsafe path '{path}'"));
    }
    Ok(())
}

fn attest_workspace_path(
    root: &Path,
    path: &str,
    budget: &mut u64,
) -> std::result::Result<WorkspacePathAttestation, String> {
    let inspection = inspect_workspace_path(root, Path::new(path))
        .map_err(|error| format!("failed to inspect workspace path '{path}': {error}"))?;
    let WorkspacePathInspection { full_path, kind } = inspection;
    let metadata = match kind {
        WorkspacePathKind::Symlink => {
            return Err(format!(
                "workspace symlink '{path}' cannot be authenticated for indexed search"
            ))
        }
        WorkspacePathKind::File(metadata) => metadata,
        WorkspacePathKind::Missing => {
            return Ok(WorkspacePathAttestation {
                digest: "missing".to_string(),
                indexable: false,
            })
        }
        WorkspacePathKind::Directory | WorkspacePathKind::Other => {
            return Err(format!("workspace path '{path}' is not a regular file"))
        }
    };
    if metadata.len() > MAX_INDEXED_FILE_BYTES as u64 {
        return Err(format!(
            "workspace file '{path}' exceeds the {}-byte attestation limit",
            MAX_INDEXED_FILE_BYTES
        ));
    }
    let mut file = File::open(&full_path)
        .map_err(|error| format!("failed to read workspace file '{path}': {error}"))?;
    let mut hasher = blake3::Hasher::new();
    let mut bytes_read = 0_u64;
    let mut contains_zero = false;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("failed to read workspace file '{path}': {error}"))?;
        if read == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(read as u64);
        *budget = budget.saturating_add(read as u64);
        if bytes_read > MAX_INDEXED_FILE_BYTES as u64 || *budget > MAX_ATTESTED_WORKSPACE_BYTES {
            return Err("workspace content exceeds the bounded attestation byte limit".to_string());
        }
        contains_zero |= buffer[..read].contains(&0);
        hasher.update(&buffer[..read]);
    }
    Ok(WorkspacePathAttestation {
        digest: hasher.finalize().to_hex().to_string(),
        indexable: bytes_read > 0 && !contains_zero,
    })
}

pub(crate) fn stable_clean_commit(
    before: Option<&GitWorkspaceSnapshot>,
    after: Option<&GitWorkspaceSnapshot>,
) -> Option<String> {
    match (before, after) {
        (Some(before), Some(after))
            if before.clean && after.clean && before.commit == after.commit =>
        {
            Some(after.commit.clone())
        }
        _ => None,
    }
}

pub(crate) fn workspace_freshness_reason(
    root: &Path,
    manifest: &RegexIndexManifest,
    expected_entries: &BTreeMap<String, String>,
) -> Option<String> {
    let expected_base = manifest.base_commit.as_deref()?;
    let Some(expected_clean) = manifest.workspace_clean_commit.as_deref() else {
        return Some(
            "workspace freshness could not be authenticated; rebuild the regex index from a clean Git working tree"
                .to_string(),
        );
    };
    if expected_clean != expected_base {
        return Some(format!(
            "workspace freshness attestation does not match the indexed base commit (base={expected_base}, attested={expected_clean})"
        ));
    }
    let reported = expected_entries.keys().cloned().collect::<Vec<_>>();
    match git_workspace_snapshot(root, &reported) {
        Ok(workspace) if workspace.commit != expected_clean => Some(format!(
            "regex index base commit changed (indexed={expected_clean}, current={})",
            workspace.commit
        )),
        Ok(workspace) if workspace.entries != *expected_entries => Some(
            "workspace freshness could not be authenticated because the Git working tree changed after the indexed publication"
                .to_string(),
        ),
        Ok(_) => None,
        Err(error) => Some(format!(
            "workspace freshness could not be authenticated: {error}"
        )),
    }
}

pub(crate) fn authenticate_incremental_workspace(
    root: &Path,
    manifest: &RegexIndexManifest,
    previous: &BTreeMap<String, String>,
    changed_paths: &[String],
) -> Result<Option<GitWorkspaceSnapshot>> {
    let Some(base_commit) = manifest.base_commit.as_deref() else {
        return Ok(None);
    };
    let snapshot = git_workspace_snapshot(root, changed_paths)
        .map_err(|reason| SearchError::IndexNotReady { reason })?;
    let changes_are_reported = previous
        .keys()
        .chain(snapshot.entries.keys())
        .all(|candidate| {
            previous.get(candidate) == snapshot.entries.get(candidate)
                || changed_paths.iter().any(|path| candidate == path)
        });
    if manifest.workspace_clean_commit.as_deref() != Some(base_commit)
        || snapshot.commit != base_commit
        || !changes_are_reported
    {
        return Err(SearchError::IndexNotReady {
            reason: "incremental regex update could not authenticate every workspace change path"
                .to_string(),
        });
    }
    Ok(Some(snapshot))
}

pub(crate) fn authenticate_indexed_workspace_after(
    root: &Path,
    reported_paths: &[String],
    before: Option<GitWorkspaceSnapshot>,
    indexed_fingerprints: &BTreeMap<String, String>,
) -> Result<Option<BTreeMap<String, String>>> {
    let Some(before) = before else {
        return Ok(None);
    };
    let after = git_workspace_snapshot(root, reported_paths)
        .map_err(|reason| SearchError::IndexNotReady { reason })?;
    if after != before {
        return Err(SearchError::IndexNotReady {
            reason: "Git workspace changed while publishing the incremental regex index"
                .to_string(),
        });
    }
    after.ensure_indexed_bytes(indexed_fingerprints)?;
    Ok(Some(after.entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_bytes_must_match_the_attested_snapshot() {
        let path = "src/lib.rs".to_string();
        let snapshot = GitWorkspaceSnapshot {
            commit: "base".to_string(),
            clean: false,
            entries: BTreeMap::from([(path.clone(), "after".to_string())]),
            reported: BTreeMap::from([(
                path.clone(),
                WorkspacePathAttestation {
                    digest: "after".to_string(),
                    indexable: true,
                },
            )]),
        };

        let error = snapshot
            .ensure_indexed_bytes(&BTreeMap::from([(path, "intermediate".to_string())]))
            .expect_err("intermediate bytes matched the final workspace attestation");

        assert!(matches!(error, SearchError::IndexNotReady { .. }));
    }
}
