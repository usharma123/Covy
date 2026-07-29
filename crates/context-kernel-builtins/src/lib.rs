//! Packet28's version-one reducer catalog and concrete kernel composition.
//!
//! Applications that need the supported Packet28 tool families should use
//! [`Kernel::with_v1_reducers`]. Applications defining a different catalog can
//! use [`context_kernel_mechanism::KernelMechanism`] directly.

use std::collections::{BTreeSet, HashMap};
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use context_memory_core::{
    basename_alias, normalize_context_path, CachePersistenceMetrics, ContextStoreEntryDetail,
    ContextStoreEntrySummary, ContextStoreListFilter, ContextStorePaging, ContextStorePruneReport,
    ContextStorePruneRequest, ContextStoreStats, DeltaReuseHooks, PacketCache, RecallHit,
    RecallMode, RecallOptions, RecallScope,
};

pub use context_kernel_mechanism::*;

mod agenty_runtime;
mod broker_memory_runtime;
mod contextq_runtime;
mod correlation_runtime;
mod diff_runtime;
mod governance_runtime;
mod instruction_runtime;
mod kernel_registry;
mod reactive_runtime;
mod tool_reducers_runtime;

pub(crate) use agenty_runtime::*;
pub(crate) use broker_memory_runtime::*;
pub(crate) use contextq_runtime::*;
pub(crate) use correlation_runtime::*;
pub use diff_runtime::*;
pub(crate) use governance_runtime::*;
pub use instruction_runtime::*;
pub use kernel_registry::*;
pub(crate) use reactive_runtime::*;
pub(crate) use tool_reducers_runtime::*;

/// Packet28's compatibility composition of the generic kernel mechanism and
/// the version-one built-in reducer catalog.
pub struct Kernel {
    inner: KernelMechanism,
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel {
    /// Creates an empty kernel with Packet28 governance and replanning
    /// services configured.
    pub fn new() -> Self {
        Self {
            inner: KernelMechanism::with_services(builtin_services()),
        }
    }

    /// Creates a kernel with the complete version-one built-in catalog.
    pub fn with_v1_reducers() -> Self {
        let mut kernel = Self::new();
        register_v1_reducers(&mut kernel);
        kernel
    }

    /// Creates an empty persistent kernel with Packet28 services configured.
    pub fn with_persistence(config: PersistConfig) -> Self {
        Self {
            inner: KernelMechanism::with_persistence_and_services(config, builtin_services()),
        }
    }

    /// Creates a persistent kernel with the complete version-one built-in
    /// catalog.
    pub fn with_v1_reducers_and_persistence(config: PersistConfig) -> Self {
        let mut kernel = Self::with_persistence(config);
        register_v1_reducers(&mut kernel);
        kernel
    }

    pub fn flush_cache_persistence(
        &self,
        timeout: Duration,
    ) -> Result<CachePersistenceMetrics, KernelError> {
        self.inner.flush_cache_persistence(timeout)
    }

    pub fn shutdown_cache_persistence(
        &self,
        timeout: Duration,
    ) -> Result<CachePersistenceMetrics, KernelError> {
        self.inner.shutdown_cache_persistence(timeout)
    }

    pub fn context_store_list(
        &self,
        filter: &ContextStoreListFilter,
        paging: &ContextStorePaging,
    ) -> Result<Vec<ContextStoreEntrySummary>, KernelError> {
        self.inner.context_store_list(filter, paging)
    }

    pub fn context_store_get(
        &self,
        cache_key: &str,
    ) -> Result<Option<ContextStoreEntryDetail>, KernelError> {
        self.inner.context_store_get(cache_key)
    }

    pub fn context_store_stats(&self) -> Result<ContextStoreStats, KernelError> {
        self.inner.context_store_stats()
    }

    pub fn context_store_recall(
        &self,
        query: &str,
        options: &RecallOptions,
    ) -> Result<Vec<RecallHit>, KernelError> {
        self.inner.context_store_recall(query, options)
    }

    pub fn context_store_prune(
        &self,
        request: ContextStorePruneRequest,
        timeout: Duration,
    ) -> Result<ContextStorePruneReport, KernelError> {
        self.inner.context_store_prune(request, timeout)
    }

    pub fn cache_runtime_metrics(&self) -> CacheRuntimeMetrics {
        self.inner.cache_runtime_metrics()
    }

    pub fn register_reducer<F>(&mut self, target: impl Into<String>, reducer: F)
    where
        F: Fn(&mut ExecutionContext, &[KernelPacket]) -> Result<ReducerResult, KernelError>
            + Send
            + Sync
            + 'static,
    {
        self.inner.register_reducer(target, reducer);
    }

    pub fn reducer_names(&self) -> Vec<String> {
        self.inner.reducer_names()
    }

    pub fn execute(&self, request: KernelRequest) -> Result<KernelResponse, KernelError> {
        self.inner.execute(request)
    }

    pub fn execute_with_hooks(
        &self,
        request: KernelRequest,
        hooks: &mut dyn DeltaReuseHooks,
    ) -> Result<KernelResponse, KernelError> {
        self.inner.execute_with_hooks(request, hooks)
    }

    pub fn execute_sequence(
        &self,
        request: KernelSequenceRequest,
    ) -> Result<KernelSequenceResponse, KernelError> {
        self.inner.execute_sequence(request)
    }

    pub fn execute_sequence_with_observer(
        &self,
        request: KernelSequenceRequest,
        observer: &mut dyn SequenceObserver,
    ) -> Result<KernelSequenceResponse, KernelError> {
        self.inner.execute_sequence_with_observer(request, observer)
    }
}

impl Deref for Kernel {
    type Target = KernelMechanism;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Kernel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

fn builtin_services() -> KernelServices {
    KernelServices::new(
        Arc::new(BuiltinExecutionPolicy),
        Arc::new(BuiltinReactivePlanner),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct ContextAssembleEnvelopePayload {
    sources: Vec<String>,
    sections: Vec<contextq_core::ContextSection>,
    refs: Vec<contextq_core::ContextRef>,
    truncated: bool,
    assembly: contextq_core::AssemblySummary,
    tool_invocations: Vec<contextq_core::ToolInvocation>,
    reducer_invocations: Vec<contextq_core::ReducerInvocation>,
    text_blobs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    debug: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct ContextManageRequest {
    task_id: String,
    query: Option<String>,
    budget_tokens: u64,
    budget_bytes: usize,
    scope: RecallScope,
    mode: RecallMode,
    include_debug: bool,
    checkpoint_id: Option<String>,
    focus_paths: Vec<String>,
    focus_symbols: Vec<String>,
}

impl Default for ContextManageRequest {
    fn default() -> Self {
        Self {
            task_id: String::new(),
            query: None,
            budget_tokens: contextq_core::DEFAULT_BUDGET_TOKENS,
            budget_bytes: contextq_core::DEFAULT_BUDGET_BYTES,
            scope: RecallScope::TaskFirst,
            mode: RecallMode::Conceptual,
            include_debug: false,
            checkpoint_id: None,
            focus_paths: Vec::new(),
            focus_symbols: Vec::new(),
        }
    }
}

fn path_matches_any(patterns: &[String], candidate: &str) -> bool {
    patterns.iter().any(|pattern| {
        let pattern = pattern.trim();
        !pattern.is_empty()
            && (candidate == pattern
                || candidate.starts_with(pattern)
                || pattern.starts_with(candidate)
                || candidate.contains(pattern))
    })
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn merge_json(left: Value, right: Value) -> Value {
    match (left, right) {
        (Value::Object(mut left), Value::Object(right)) => {
            for (key, value) in right {
                left.insert(key, value);
            }
            Value::Object(left)
        }
        (value, Value::Null) => value,
        (_, value) => value,
    }
}

#[cfg(test)]
mod tests;
