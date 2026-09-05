use std::path::PathBuf;

use serde::{Deserialize, Serialize};
pub use suite_packet_core::memory::{
    CachePacket, ContextStoreEntryDetail, ContextStoreEntrySummary, ContextStorePruneReport,
    ContextStoreStats, DeltaReuse, EvictionCounters, EvictionReason, PacketCacheEntry,
    RecallBudgetEstimate, RecallHit, RecallMode, RecallSourceTier,
};
pub use suite_packet_core::{MemoryKind, MemorySourceTier};

use crate::PacketCache;

pub const DEFAULT_PERSIST_TTL_SECS: u64 = 86_400;

pub(crate) fn add_evictions(counters: &mut EvictionCounters, reason: EvictionReason, count: usize) {
    match reason {
        EvictionReason::ExpiredTtl => {
            counters.expired_ttl = counters.expired_ttl.saturating_add(count)
        }
        EvictionReason::ManualPrune => {
            counters.manual_prune = counters.manual_prune.saturating_add(count)
        }
        EvictionReason::VersionMismatch => {
            counters.version_mismatch = counters.version_mismatch.saturating_add(count)
        }
        EvictionReason::CorruptLoadRecovery => {
            counters.corrupt_load_recovery = counters.corrupt_load_recovery.saturating_add(count)
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ContextStoreListFilter {
    pub target: Option<String>,
    pub contains_query: Option<String>,
    pub created_after_unix: Option<u64>,
    pub created_before_unix: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ContextStorePaging {
    pub offset: usize,
    pub limit: usize,
}

impl Default for ContextStorePaging {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 100,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ContextStorePruneRequest {
    pub all: bool,
    pub ttl_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct RecallOptions {
    pub limit: usize,
    pub since_unix: Option<u64>,
    pub until_unix: Option<u64>,
    pub target: Option<String>,
    pub task_id: Option<String>,
    pub scope: RecallScope,
    pub packet_types: Vec<String>,
    /// When nonempty, require at least one path filter match independently of
    /// query matches. Uses normalized paths, substrings, and unambiguous basenames.
    pub path_filters: Vec<String>,
    /// When nonempty, require at least one case-insensitive symbol substring
    /// match independently of query matches and any path filters.
    pub symbol_filters: Vec<String>,
    pub mode: RecallMode,
    pub include_debug: bool,
}

impl Default for RecallOptions {
    fn default() -> Self {
        Self {
            limit: 8,
            since_unix: None,
            until_unix: None,
            target: None,
            task_id: None,
            scope: RecallScope::Global,
            packet_types: Vec::new(),
            path_filters: Vec::new(),
            symbol_filters: Vec::new(),
            mode: RecallMode::Auto,
            include_debug: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecallScope {
    #[default]
    Global,
    TaskFirst,
    TaskOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct NormalizedPathRef {
    pub canonical: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basename: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RelatedEntryMatch {
    pub entry: PacketCacheEntry,
    pub canonical_path_matches: Vec<String>,
    pub basename_path_matches: Vec<String>,
    pub symbol_matches: Vec<String>,
    pub test_matches: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PersistConfig {
    pub root_dir: PathBuf,
    pub ttl_secs: u64,
}

impl PersistConfig {
    pub fn new(root_dir: PathBuf) -> Self {
        Self {
            root_dir,
            ttl_secs: DEFAULT_PERSIST_TTL_SECS,
        }
    }

    pub fn with_ttl_secs(mut self, ttl_secs: u64) -> Self {
        self.ttl_secs = ttl_secs;
        self
    }
}

#[derive(Debug, Clone)]
pub struct CacheLookup {
    pub cache_key: String,
    pub input_hash: String,
    pub entry: Option<PacketCacheEntry>,
    pub suggested_reuse_base: Option<String>,
}

pub trait DeltaReuseHooks {
    fn select_reuse_base(
        &mut self,
        _target: &str,
        _input_hash: &str,
        _cache: &PacketCache,
    ) -> Option<String> {
        None
    }

    fn on_hit(&mut self, _entry: &PacketCacheEntry) {}

    fn on_put(&mut self, _entry: &PacketCacheEntry) {}
}

#[derive(Default)]
pub struct NoopDeltaReuseHooks;

impl DeltaReuseHooks for NoopDeltaReuseHooks {}
