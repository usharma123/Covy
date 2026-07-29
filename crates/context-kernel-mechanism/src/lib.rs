//! Concrete-reducer-free execution, cache, governance, and scheduling
//! mechanisms for the Packet28 context kernel.
//!
//! Composition crates provide [`ExecutionPolicy`] and [`ReactivePlanner`]
//! implementations, then register reducer functions on [`KernelMechanism`].
//! This crate deliberately contains no built-in target catalog.
#![doc = include_str!("../PUBLIC_API.md")]

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use thiserror::Error;

use context_memory_core::{
    CachePacket, CachePersistence, CachePersistenceMetrics, ContextStoreEntryDetail,
    ContextStoreEntrySummary, ContextStoreListFilter, ContextStorePaging, ContextStorePruneReport,
    ContextStorePruneRequest, ContextStoreStats, DeltaReuseHooks, NoopDeltaReuseHooks, PacketCache,
    RecallHit, RecallOptions, RelatedEntryMatch,
};

pub use context_memory_core::PersistConfig;

mod governance_runtime;
mod kernel_registry;
mod kernel_runtime;
mod kernel_types;
mod reactive_runtime;
mod services;

pub(crate) use governance_runtime::*;
pub use kernel_registry::load_packet_file;
pub use kernel_runtime::{ExecutionContext, KernelMechanism};
pub use kernel_types::{
    normalize_sequence_request, BudgetMetric, BudgetStage, BudgetUsage, CacheRuntimeMetrics,
    ExecutionBudget, GovernanceAudit, KernelAudit, KernelError, KernelFailure, KernelPacket,
    KernelRequest, KernelResponse, KernelSequenceRequest, KernelSequenceResponse,
    KernelStepReactiveConfig, KernelStepRequest, KernelStepResponse, NoopSequenceObserver,
    ReactiveReplanMode, ReactiveSequenceConfig, ReducerExecutionAudit, ReducerResult,
    SequenceObserver,
};
pub(crate) use reactive_runtime::*;
pub use services::{
    ExecutionPolicy, ExecutionPolicyRun, KernelPlanMutation, KernelServices, ReactivePlan,
    ReactivePlanRequest, ReactivePlanner,
};

fn estimate_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
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
