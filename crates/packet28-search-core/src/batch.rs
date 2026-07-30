//! Bounded indexed-search batching with one final workspace attestation.

use std::path::Path;

use packet28_reducer_core::{SearchRequest, SearchResult};

use crate::error::{Result, SearchError};
use crate::model::RegexIndexRuntime;

const MAX_BATCH_REQUESTS: usize = 32;
const MAX_BATCH_VERIFY_CANDIDATES: usize = 64;

/// Authenticated results for a primary search stage and an optional deferred stage.
#[doc(hidden)]
#[derive(Debug)]
pub struct BrokerInternalGuardedIndexedSearchBatch {
    /// Results for every primary request.
    pub primary: Vec<Option<SearchResult>>,
    /// Results for deferred requests when the selector requested that stage.
    pub deferred: Option<Vec<Option<SearchResult>>>,
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
/// Returns typed loading, query, corruption, and freshness failures. More than
/// 32 requests are rejected as an unbounded batch.
pub fn guarded_indexed_search_batch(
    root: &Path,
    runtime: &RegexIndexRuntime,
    requests: &[SearchRequest],
) -> Result<Vec<Option<SearchResult>>> {
    Ok(
        broker_internal_guarded_indexed_search_staged_batch(root, runtime, requests, &[], |_| {
            false
        })?
        .primary,
    )
}

/// Trusted broker integration for executing primary requests and conditionally
/// executing one deferred stage before one final workspace attestation.
///
/// The selector observes primary results after every candidate byte and path
/// has been authenticated against the active index. Those results are still
/// provisional with respect to the final whole-workspace attestation, so
/// this callback is restricted to trusted in-process stage selection and must
/// not persist or expose its input. Returned stage results remain withheld
/// until the selected work and final attestation succeed.
///
/// # Errors
///
/// Returns typed loading, query, candidate-authentication, corruption, and
/// freshness failures. More than 32 declared requests are rejected.
#[doc(hidden)]
pub fn broker_internal_guarded_indexed_search_staged_batch(
    root: &Path,
    runtime: &RegexIndexRuntime,
    primary_requests: &[SearchRequest],
    deferred_requests: &[SearchRequest],
    select_deferred: impl FnOnce(&[Option<SearchResult>]) -> bool,
) -> Result<BrokerInternalGuardedIndexedSearchBatch> {
    let request_count = primary_requests
        .len()
        .saturating_add(deferred_requests.len());
    if request_count > MAX_BATCH_REQUESTS {
        return Err(SearchError::IndexNotReady {
            reason: format!(
                "indexed search batch contains {request_count} requests; maximum is {MAX_BATCH_REQUESTS}"
            ),
        });
    }
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
    let mut remaining_candidates = MAX_BATCH_VERIFY_CANDIDATES;
    let primary = run_requests(root, runtime, primary_requests, &mut remaining_candidates)?;
    let deferred = if select_deferred(&primary) {
        Some(run_requests(
            root,
            runtime,
            deferred_requests,
            &mut remaining_candidates,
        )?)
    } else {
        None
    };
    crate::query::ensure_workspace_fresh(root, runtime, loaded.as_ref())?;
    Ok(BrokerInternalGuardedIndexedSearchBatch { primary, deferred })
}

fn run_requests(
    root: &Path,
    runtime: &RegexIndexRuntime,
    requests: &[SearchRequest],
    remaining_candidates: &mut usize,
) -> Result<Vec<Option<SearchResult>>> {
    requests
        .iter()
        .map(|request| {
            match crate::query::indexed_search_with_guard(
                root,
                runtime,
                request,
                true,
                false,
                Some(&mut *remaining_candidates),
            ) {
                Ok(result) => Ok(Some(result)),
                Err(SearchError::IndexNotReady { .. }) => Ok(None),
                Err(error) => Err(error),
            }
        })
        .collect()
}
