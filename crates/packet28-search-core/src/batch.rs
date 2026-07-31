//! Bounded indexed-search batching with final workspace attestation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use packet28_reducer_core::{SearchRequest, SearchResult};

use crate::error::{Result, SearchError};
use crate::model::{RegexIndexManifest, RegexIndexRuntime};
use crate::query::ResolvedRequestScope;

const MAX_BATCH_REQUESTS: usize = 32;
const MAX_BATCH_VERIFY_CANDIDATES: usize = 64;
const MAX_BATCH_REQUESTED_PATHS: usize = 16;
const MAX_REQUESTED_PATH_BYTES: usize = 4 * 1024;
const MAX_BATCH_REQUESTED_PATH_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionBinding {
    root: PathBuf,
    manifest: RegexIndexManifest,
}

/// Opaque broker state that shares bounded work across fully attested batches.
///
/// The broker uses one session for a primary batch and, only when needed, one
/// deferred batch. Candidate and request ceilings are cumulative, and repeated
/// requested scopes are resolved only once. A session cannot cross repository
/// roots or runtime generations. Any failed batch poisons the session so a
/// cached scope can never outlive a failed freshness attestation.
#[doc(hidden)]
#[derive(Debug)]
pub struct BrokerInternalGuardedIndexedSearchSession {
    poisoned: bool,
    binding: Option<SessionBinding>,
    remaining_requests: usize,
    remaining_candidates: usize,
    resolved_scopes: BTreeMap<Vec<String>, ResolvedRequestScope>,
    requested_path_count: usize,
    requested_path_bytes: usize,
    #[cfg(test)]
    scope_resolution_count: usize,
}

impl BrokerInternalGuardedIndexedSearchSession {
    /// Creates a broker batch session with the fixed safety ceilings.
    pub fn new() -> Self {
        Self {
            poisoned: false,
            binding: None,
            remaining_requests: MAX_BATCH_REQUESTS,
            remaining_candidates: MAX_BATCH_VERIFY_CANDIDATES,
            resolved_scopes: BTreeMap::new(),
            requested_path_count: 0,
            requested_path_bytes: 0,
            #[cfg(test)]
            scope_resolution_count: 0,
        }
    }

    fn bind(&mut self, root: &Path, runtime: &RegexIndexRuntime) -> Result<()> {
        let requested = SessionBinding {
            root: root.to_path_buf(),
            manifest: runtime.manifest.clone(),
        };
        match self.binding.as_ref() {
            Some(bound) if bound != &requested => Err(SearchError::IndexNotReady {
                reason: "indexed search batch session cannot cross roots or runtime generations"
                    .to_string(),
            }),
            Some(_) => Ok(()),
            None => {
                self.binding = Some(requested);
                Ok(())
            }
        }
    }

    fn reserve_requests(&mut self, request_count: usize) -> Result<()> {
        if request_count > self.remaining_requests {
            let total = MAX_BATCH_REQUESTS
                .saturating_sub(self.remaining_requests)
                .saturating_add(request_count);
            return Err(SearchError::IndexNotReady {
                reason: format!(
                    "indexed search session contains {total} requests; maximum is {MAX_BATCH_REQUESTS}"
                ),
            });
        }
        self.remaining_requests -= request_count;
        Ok(())
    }

    fn prepare_scopes(&mut self, root: &Path, requests: &[SearchRequest]) -> Result<()> {
        for request in requests {
            if self.resolved_scopes.contains_key(&request.requested_paths) {
                continue;
            }
            let scope_path_count = request.requested_paths.len();
            let requested_path_count = self
                .requested_path_count
                .checked_add(scope_path_count)
                .ok_or_else(requested_scope_limit_error)?;
            if requested_path_count > MAX_BATCH_REQUESTED_PATHS {
                return Err(requested_scope_limit_error());
            }
            let mut scope_bytes = 0usize;
            for path in &request.requested_paths {
                if path.len() > MAX_REQUESTED_PATH_BYTES {
                    return Err(SearchError::IndexNotReady {
                        reason: format!(
                            "indexed search requested path exceeds {MAX_REQUESTED_PATH_BYTES} bytes"
                        ),
                    });
                }
                scope_bytes = scope_bytes
                    .checked_add(path.len())
                    .ok_or_else(requested_scope_bytes_limit_error)?;
            }
            let requested_path_bytes = self
                .requested_path_bytes
                .checked_add(scope_bytes)
                .ok_or_else(requested_scope_bytes_limit_error)?;
            if requested_path_bytes > MAX_BATCH_REQUESTED_PATH_BYTES {
                return Err(requested_scope_bytes_limit_error());
            }
            let resolved = crate::query::resolve_request_scope(root, &request.requested_paths);
            self.requested_path_count = requested_path_count;
            self.requested_path_bytes = requested_path_bytes;
            #[cfg(test)]
            {
                self.scope_resolution_count += 1;
            }
            self.resolved_scopes
                .insert(request.requested_paths.clone(), resolved);
        }
        Ok(())
    }

    #[cfg(test)]
    fn resolved_scope_count(&self) -> usize {
        self.scope_resolution_count
    }
}

impl Default for BrokerInternalGuardedIndexedSearchSession {
    fn default() -> Self {
        Self::new()
    }
}

fn requested_scope_limit_error() -> SearchError {
    SearchError::IndexNotReady {
        reason: format!(
            "indexed search session contains too many requested paths; maximum is {MAX_BATCH_REQUESTED_PATHS}"
        ),
    }
}

fn requested_scope_bytes_limit_error() -> SearchError {
    SearchError::IndexNotReady {
        reason: format!(
            "indexed search session requested-path input exceeds {MAX_BATCH_REQUESTED_PATH_BYTES} bytes"
        ),
    }
}

/// Executes a bounded set of selective indexed searches behind one final
/// workspace freshness attestation.
///
/// Requests whose index plans are too broad are represented by `None`; they
/// are never verified against every repository file. Successful results are
/// returned atomically only after the complete batch passes the shared
/// freshness check.
///
/// # Errors
///
/// Returns typed loading, query, corruption, scope-limit, and freshness
/// failures.
pub fn guarded_indexed_search_batch(
    root: &Path,
    runtime: &RegexIndexRuntime,
    requests: &[SearchRequest],
) -> Result<Vec<Option<SearchResult>>> {
    let mut session = BrokerInternalGuardedIndexedSearchSession::new();
    broker_internal_guarded_indexed_search_batch(root, runtime, requests, &mut session)
}

/// Executes one fully attested broker batch while sharing cumulative ceilings
/// and resolved scopes with later batches in the same session.
///
/// No result is returned before candidate authentication and the final
/// whole-workspace freshness attestation succeed. The broker may therefore
/// inspect a primary result to decide whether to issue a deferred batch without
/// receiving provisional results.
///
/// # Errors
///
/// Returns typed loading, query, candidate-authentication, corruption,
/// scope-limit, session-binding, and freshness failures.
#[doc(hidden)]
pub fn broker_internal_guarded_indexed_search_batch(
    root: &Path,
    runtime: &RegexIndexRuntime,
    requests: &[SearchRequest],
    session: &mut BrokerInternalGuardedIndexedSearchSession,
) -> Result<Vec<Option<SearchResult>>> {
    if session.poisoned {
        return Err(SearchError::IndexNotReady {
            reason: "indexed search batch session cannot be reused after a failed batch"
                .to_string(),
        });
    }
    let result =
        broker_internal_guarded_indexed_search_batch_once(root, runtime, requests, session);
    if result.is_err() {
        session.poisoned = true;
    }
    result
}

fn broker_internal_guarded_indexed_search_batch_once(
    root: &Path,
    runtime: &RegexIndexRuntime,
    requests: &[SearchRequest],
    session: &mut BrokerInternalGuardedIndexedSearchSession,
) -> Result<Vec<Option<SearchResult>>> {
    let loaded = runtime.loaded.as_ref().ok_or(SearchError::IndexNotLoaded)?;
    if runtime.manifest.status != "ready" {
        return Err(SearchError::IndexNotReady {
            reason: runtime
                .manifest
                .stale_reason
                .clone()
                .or_else(|| runtime.manifest.last_error.clone())
                .unwrap_or_else(|| "regex search index is not ready".to_string()),
        });
    }
    session.bind(root, runtime)?;
    session.reserve_requests(requests.len())?;
    session.prepare_scopes(root, requests)?;
    let results = run_requests(
        root,
        runtime,
        requests,
        &session.resolved_scopes,
        &mut session.remaining_candidates,
    )?;
    crate::query::ensure_workspace_fresh(root, runtime, loaded.as_ref())?;
    Ok(results)
}

fn run_requests(
    root: &Path,
    runtime: &RegexIndexRuntime,
    requests: &[SearchRequest],
    scopes: &BTreeMap<Vec<String>, ResolvedRequestScope>,
    remaining_candidates: &mut usize,
) -> Result<Vec<Option<SearchResult>>> {
    requests
        .iter()
        .map(|request| {
            let scope = scopes.get(&request.requested_paths).ok_or_else(|| {
                SearchError::corrupt("indexed search batch scope cache is incomplete")
            })?;
            match crate::query::indexed_search_with_resolved_scope(
                root,
                runtime,
                request,
                true,
                false,
                Some(&mut *remaining_candidates),
                scope,
            ) {
                Ok(result) => Ok(Some(result)),
                Err(SearchError::IndexNotReady { .. }) => Ok(None),
                Err(error) => Err(error),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_batch_scope_is_resolved_once() {
        let root = tempfile::tempdir().unwrap();
        let request = SearchRequest {
            query: "needle".to_string(),
            fixed_string: true,
            requested_paths: vec!["missing.rs".to_string()],
            ..SearchRequest::default()
        };
        let mut session = BrokerInternalGuardedIndexedSearchSession::new();

        session
            .prepare_scopes(root.path(), &[request.clone(), request.clone()])
            .unwrap();
        session.prepare_scopes(root.path(), &[request]).unwrap();

        assert_eq!(session.resolved_scope_count(), 1);
    }
}
