//! Stable, implementation-free wire contract for Packet28 daemon clients.
//!
//! Runtime, persistence, kernel, memory, and search implementations deliberately
//! live outside this crate. Clients can depend on these request/response types,
//! framing helpers, and deterministic endpoint paths without pulling in the
//! daemon implementation graph.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use suite_packet_core::kernel::{
    KernelRequest, KernelResponse, KernelSequenceRequest, KernelSequenceResponse,
};
use suite_packet_core::memory::{
    ContextStoreEntryDetail, ContextStoreEntrySummary, ContextStorePruneReport, ContextStoreStats,
    RecallHit,
};

pub mod broker;
pub mod commands;
pub mod context_store;
pub mod frame;
pub mod hooks;
pub mod index;
pub mod message;
pub mod paths;
pub mod registry;
pub mod task;

use broker::*;
use commands::*;
use context_store::*;
use hooks::*;
use index::*;
use task::*;

pub use message::{DaemonRequest, DaemonResponse};
