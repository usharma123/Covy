//! Bounded indexed-search batching with one final workspace attestation.

use std::path::Path;

use packet28_reducer_core::{SearchRequest, SearchResult};

use crate::error::{Result, SearchError};
use crate::model::RegexIndexRuntime;

const MAX_BATCH_REQUESTS: usize = 32;
const MAX_BATCH_VERIFY_CANDIDATES: usize = 64;

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
    if requests.len() > MAX_BATCH_REQUESTS {
        return Err(SearchError::IndexNotReady {
            reason: format!(
                "indexed search batch contains {} requests; maximum is {MAX_BATCH_REQUESTS}",
                requests.len()
            ),
        });
    }
    if requests.is_empty() {
        return Ok(Vec::new());
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
    let results = requests
        .iter()
        .map(|request| {
            match crate::query::indexed_search_with_guard(
                root,
                runtime,
                request,
                true,
                false,
                Some(MAX_BATCH_VERIFY_CANDIDATES),
            ) {
                Ok(result) => Ok(Some(result)),
                Err(SearchError::IndexNotReady { .. }) => Ok(None),
                Err(error) => Err(error),
            }
        })
        .collect::<Result<Vec<_>>>()?;
    crate::query::ensure_workspace_fresh(root, runtime, loaded.as_ref())?;
    Ok(results)
}
