//! Persistent literal/regex indexing and indexed search verification.
//!
//! The crate builds a repository-local index, validates every persisted layer
//! before publication, and verifies indexed candidates against source files.
//! Full builds publish an immutable base; incremental updates append immutable
//! overlay segments plus owned path tombstones. A manifest switch is the only
//! publication point, so readers retain their `Arc`-owned generation while a
//! writer prepares the next one. Legacy schema-v3 aggregate overlays remain
//! readable and migrate to segmented generation records on their next update.
//! Public writers serialize on a repository-local lock and reject stale
//! handles by both generation and validated publication fingerprint. A durable
//! high-water mark outside the clearable index directory reserves each
//! generation before artifact construction and is never rolled back.
//! Every generation record carries BLAKE3 digests for its artifacts. New
//! manifests also bind the complete immutable generation record to a digest,
//! while separately binding overlay ownership/tombstone state.
//! Legacy digestless records remain readable only when their owner mapping
//! names the newest segment containing each live path.
//!
//! Best-effort pruning retains only the current and explicitly recoverable
//! previous generation under normal filesystem operation.
//!
//! Publication is process-crash atomic on filesystems that provide atomic
//! same-directory rename: immutable generation-specific artifacts are created
//! once and flushed before the mutable manifest is renamed last. The
//! implementation does not call `fsync` on files or parent directories and
//! therefore does not claim power-loss durability.
//! Fallible operations return [`SearchError`], allowing callers to distinguish
//! unavailable indexes, invalid queries, corruption, and typed dependency
//! failures without parsing an `anyhow` report.
//!
//! # Examples
//!
//! ```
//! use packet28_search_core::RegexIndexRuntime;
//!
//! let runtime = RegexIndexRuntime::default();
//! assert!(!runtime.is_loaded());
//! ```
//!
//! Implementation modules are deliberately private; callers use the root
//! facade (or the feature-gated [`shared_scan`] composition API):
//!
//! ```compile_fail
//! use packet28_search_core::query::indexed_search;
//! ```

extern crate packet28_binary_codec as wincode;

mod error;
mod generation;
mod layer;
mod model;
mod paths;
mod postings;
mod publication;
mod query;
#[cfg(feature = "shared-repository-scan")]
pub mod shared_scan;
mod support;
mod weights;

pub use crate::error::{Result, SearchError};
pub use crate::generation::{
    clear_index, load_runtime, rebuild_full_index, rebuild_full_index_with_progress,
    update_overlay_index,
};
pub use crate::model::{RegexIndexManifest, RegexIndexRuntime};
pub use crate::query::{guarded_fallback_reason, indexed_search};

#[cfg(test)]
mod tests;
