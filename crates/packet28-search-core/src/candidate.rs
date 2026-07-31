//! Source-candidate authentication and exact match verification.

use std::fs::{File, Metadata, OpenOptions};
use std::io::Read;
use std::path::Path;

use packet28_reducer_core::SearchMatch;

use crate::error::{Result, SearchError};
use crate::model::{DocRecord, LoadedIndex, Verifier, MAX_INDEXED_FILE_BYTES};
use crate::paths::{inspect_workspace_path, WorkspacePathInspection, WorkspacePathKind};
use crate::postings::normalize_for_index;

#[derive(Clone, Copy)]
enum CandidateReadStage {
    AfterInspection,
    AfterOpen,
}

pub(crate) fn verify_path(
    root: &Path,
    loaded: &LoadedIndex,
    path: &str,
    verifier: &Verifier,
    max_matches_per_file: Option<usize>,
) -> Result<Vec<SearchMatch>> {
    let document = active_document(loaded, path).ok_or_else(|| {
        SearchError::corrupt(format!(
            "indexed candidate '{path}' has no active document record"
        ))
    })?;
    let bytes = read_authenticated_candidate(root, path, document)?;
    match verifier {
        Verifier::Regex {
            regex,
            whole_file_prefilter,
        } => {
            let text = String::from_utf8_lossy(&bytes);
            if *whole_file_prefilter && !regex.is_match(text.as_ref()) {
                return Ok(Vec::new());
            }
            collect_line_matches(path, &text, max_matches_per_file, |line| {
                regex.is_match(line)
            })
        }
        Verifier::FixedBytes {
            needle,
            case_insensitive,
        } => {
            if !contains_fixed_bytes(&bytes, needle, *case_insensitive) {
                return Ok(Vec::new());
            }
            let text = String::from_utf8_lossy(&bytes);
            let normalized_needle = case_insensitive.then(|| normalize_for_index(needle));
            collect_line_matches(path, &text, max_matches_per_file, |line| {
                if *case_insensitive {
                    normalize_for_index(line.as_bytes())
                        .windows(needle.len())
                        .any(|window| window == normalized_needle.as_deref().unwrap_or_default())
                } else {
                    line.as_bytes()
                        .windows(needle.len())
                        .any(|window| window == needle.as_slice())
                }
            })
        }
    }
}

fn active_document<'a>(loaded: &'a LoadedIndex, path: &str) -> Option<&'a DocRecord> {
    if loaded.overlay_state.deleted_paths.contains(path) {
        return None;
    }
    if let Some(owner) = loaded.overlay_state.owners.get(path) {
        let segment = loaded
            .overlays
            .iter()
            .find(|segment| segment.generation == *owner)?;
        let doc_id = segment.layer.doc_ids_by_path.get(path)?;
        return segment.layer.docs.get(*doc_id as usize);
    }
    if loaded.overlay_state.shadowed_paths.contains(path) {
        return None;
    }
    let doc_id = loaded.base.doc_ids_by_path.get(path)?;
    loaded.base.docs.get(*doc_id as usize)
}

fn read_authenticated_candidate(root: &Path, path: &str, document: &DocRecord) -> Result<Vec<u8>> {
    read_authenticated_candidate_with_hook(root, path, document, |_| {})
}

fn read_authenticated_candidate_with_hook(
    root: &Path,
    path: &str,
    document: &DocRecord,
    mut hook: impl FnMut(CandidateReadStage),
) -> Result<Vec<u8>> {
    if document.path != path || document.size > MAX_INDEXED_FILE_BYTES as u64 {
        return Err(candidate_authentication_failure(
            path,
            "active document metadata does not match the bounded candidate path",
        ));
    }
    let WorkspacePathInspection {
        full_path,
        kind: WorkspacePathKind::File(inspected),
    } = inspect_workspace_path(root, Path::new(path))
        .map_err(|error| candidate_authentication_failure(path, error.to_string()))?
    else {
        return Err(candidate_authentication_failure(
            path,
            "candidate path is missing, non-regular, or traverses a symlink",
        ));
    };
    hook(CandidateReadStage::AfterInspection);
    let mut file = open_candidate(&full_path)
        .map_err(|error| candidate_authentication_failure(path, error.to_string()))?;
    hook(CandidateReadStage::AfterOpen);
    let opened_before = file
        .metadata()
        .map_err(|error| candidate_authentication_failure(path, error.to_string()))?;
    if !opened_before.is_file() || !metadata_unchanged(&inspected, &opened_before) {
        return Err(candidate_authentication_failure(
            path,
            "candidate path identity changed before it was opened",
        ));
    }
    let mut bytes = Vec::with_capacity(document.size as usize);
    (&mut file)
        .take(MAX_INDEXED_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| candidate_authentication_failure(path, error.to_string()))?;
    let opened_after = file
        .metadata()
        .map_err(|error| candidate_authentication_failure(path, error.to_string()))?;
    let WorkspacePathInspection {
        kind: WorkspacePathKind::File(current),
        ..
    } = inspect_workspace_path(root, Path::new(path))
        .map_err(|error| candidate_authentication_failure(path, error.to_string()))?
    else {
        return Err(candidate_authentication_failure(
            path,
            "candidate path identity changed while it was read",
        ));
    };
    if !metadata_unchanged(&opened_before, &opened_after)
        || !metadata_unchanged(&opened_after, &current)
    {
        return Err(candidate_authentication_failure(
            path,
            "candidate path identity changed while it was read",
        ));
    }
    let fingerprint = blake3::hash(&bytes).to_hex().to_string();
    if bytes.len() as u64 != document.size || fingerprint != document.fingerprint {
        return Err(candidate_authentication_failure(
            path,
            "candidate bytes do not match the active indexed document",
        ));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn open_candidate(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_candidate(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

#[cfg(unix)]
fn metadata_unchanged(left: &Metadata, right: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn metadata_unchanged(left: &Metadata, right: &Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

fn candidate_authentication_failure(path: &str, reason: impl Into<String>) -> SearchError {
    SearchError::CandidateAuthentication {
        path: path.to_string(),
        reason: reason.into(),
    }
}

fn collect_line_matches(
    path: &str,
    text: &str,
    max_matches_per_file: Option<usize>,
    mut predicate: impl FnMut(&str) -> bool,
) -> Result<Vec<SearchMatch>> {
    let mut matches = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if predicate(line) {
            matches.push(SearchMatch {
                path: path.to_string(),
                line: idx + 1,
                text: line.to_string(),
            });
            if max_matches_per_file.is_some_and(|limit| matches.len() >= limit) {
                break;
            }
        }
    }
    Ok(matches)
}

fn contains_fixed_bytes(bytes: &[u8], needle: &[u8], case_insensitive: bool) -> bool {
    if needle.is_empty() || bytes.len() < needle.len() {
        return false;
    }
    if case_insensitive {
        let haystack = normalize_for_index(bytes);
        let normalized_needle = normalize_for_index(needle);
        haystack
            .windows(normalized_needle.len())
            .any(|window| window == normalized_needle.as_slice())
    } else {
        bytes.windows(needle.len()).any(|window| window == needle)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;

    use crate::candidate::{
        active_document, read_authenticated_candidate_with_hook, CandidateReadStage,
    };
    use crate::error::SearchError;
    use crate::generation::rebuild_full_index;

    #[test]
    fn authenticated_read_rejects_a_restored_path_swap() {
        let root = tempfile::tempdir().unwrap();
        let outside_root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        let candidate = root.path().join("src/lib.rs");
        let parked = root.path().join("src/lib.parked");
        let outside = outside_root.path().join("outside.rs");
        fs::write(&candidate, "const SAFE_MARKER: &str = \"indexed\";\n").unwrap();
        fs::write(&outside, "const SAFE_MARKER: &str = \"transient\";\n").unwrap();
        let runtime = rebuild_full_index(root.path(), true).unwrap();
        let loaded = runtime.loaded.as_ref().unwrap();
        let document = active_document(loaded, "src/lib.rs").unwrap();

        let error =
            read_authenticated_candidate_with_hook(root.path(), "src/lib.rs", document, |stage| {
                match stage {
                    CandidateReadStage::AfterInspection => {
                        fs::rename(&candidate, &parked).unwrap();
                        fs::rename(&outside, &candidate).unwrap();
                    }
                    CandidateReadStage::AfterOpen => {
                        fs::rename(&candidate, &outside).unwrap();
                        fs::rename(&parked, &candidate).unwrap();
                    }
                }
            })
            .unwrap_err();

        assert!(matches!(error, SearchError::CandidateAuthentication { .. }));
    }
}
