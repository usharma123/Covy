//! Packet28 daemon application and composition root.
//!
//! [`serve`] owns one workspace daemon from recovery and state construction
//! through transport orchestration, cancellation, persistence flush, and
//! runtime-file cleanup. The `packet28d` executable remains a shallow CLI and
//! process-exit adapter.
//!
//! Wire DTOs, framing, and endpoint paths belong to
//! `packet28-daemon-protocol`; storage, integrity, leases, and recovery belong
//! to `packet28-daemon-core`. Broker implementation modules are private behind
//! one explicit crate-internal facade.
//!
//! `serve` is intentionally not a doctest target: it changes the process
//! working directory, acquires workspace leases, binds a listener, and blocks
//! until shutdown. Hermetic public happy-path examples live with the protocol
//! framing and daemon-core storage APIs.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
#[cfg(test)]
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
#[cfg(test)]
use context_kernel_core::PersistConfig;
use context_kernel_core::{
    normalize_sequence_request, Kernel, KernelFailure, KernelRequest, KernelResponse,
    KernelStepRequest, SequenceObserver,
};
use context_memory_core::{
    ContextStoreListFilter, ContextStorePaging, ContextStorePruneRequest, RecallOptions,
};
use diffy_core::model::CoverageFormat;
use notify::{Config, Event, PollWatcher, RecursiveMode, Watcher};
#[cfg(test)]
use packet28_daemon_core::storage::ensure_daemon_dir;
use packet28_daemon_core::storage::{load_task_events, now_unix};
#[cfg(test)]
use packet28_daemon_protocol::broker::{
    BrokerAction, BrokerEvictionCandidate, BrokerGetContextRequest, BrokerSection,
    BrokerSourceKind, BrokerValidatePlanRequest, BrokerVerbosity,
};
use packet28_daemon_protocol::broker::{
    BrokerGetContextResponse, BrokerPlanStep, BrokerPrepareHandoffRequest, BrokerResponseMode,
    BrokerTaskStatusRequest, BrokerTaskStatusResponse, BrokerWriteOp, BrokerWriteStateBatchRequest,
    BrokerWriteStateRequest,
};
use packet28_daemon_protocol::commands::{
    CoverCheckRequest, CoverCheckResponse, PacketFetchResponse, TaskSubmitSpec, TestMapRequest,
    TestMapResponse, TestMapSummary, TestShardRequest, TestShardResponse, WatchKind, WatchSpec,
};
use packet28_daemon_protocol::context_store::{
    ContextRecallRequest, ContextRecallResponse, ContextStoreGetRequest, ContextStoreGetResponse,
    ContextStoreListRequest, ContextStoreListResponse, ContextStorePruneDaemonRequest,
    ContextStorePruneResponse, ContextStoreStatsRequest, ContextStoreStatsResponse,
};
use packet28_daemon_protocol::frame::write_frame;
use packet28_daemon_protocol::index::{
    DaemonIndexClearResponse, DaemonIndexManifest, DaemonIndexRebuildRequest,
    DaemonIndexRebuildResponse, DaemonIndexState, DaemonIndexStatusResponse,
};
#[cfg(test)]
use packet28_daemon_protocol::message::DaemonEvent;
use packet28_daemon_protocol::message::{
    DaemonEventFrame, DaemonRequest, DaemonResponse, DaemonRuntimeInfo,
};
use packet28_daemon_protocol::paths::{
    index_dir, index_manifest_path, index_snapshot_path, task_artifact_dir, task_brief_json_path,
    task_brief_markdown_path, task_state_json_path, ContextVersionStorageId, TaskStorageId,
};
#[cfg(test)]
use packet28_daemon_protocol::paths::{ready_path, task_event_log_path, task_version_json_path};
use packet28_daemon_protocol::task::{
    TaskAwaitHandoffRequest, TaskAwaitHandoffResponse, TaskLaunchAgentRequest,
    TaskLaunchAgentResponse, TaskRecord, TaskRegistry, WatchRegistration, WatchRegistry,
};
use serde_json::{json, Value};

mod application;
mod broker;
mod commands;
mod hooks;
mod index;
mod instruction_files;
mod kernel_registry;
mod launch;
mod persistence;
mod planning;
mod runtime;
mod runtime_files;
#[cfg(unix)]
mod runtime_files_unix;
mod server;
mod state;
mod watch;

use crate::commands::{
    run_context_recall, run_context_store_get, run_context_store_list, run_context_store_prune,
    run_context_store_stats, run_cover_check, run_test_map, run_test_shard,
};
use crate::hooks::hook_ingest;
#[cfg(test)]
use crate::index::IndexIngress;
use crate::index::{
    daemon_index_clear, daemon_index_rebuild, daemon_index_status, daemon_packet28_search,
};
use crate::instruction_files::resolve_instruction_file;
use crate::launch::task_launch_agent;
#[cfg(test)]
use crate::persistence::PersistenceOwner;
#[cfg(test)]
use crate::runtime::{BlockingPool, DaemonRuntimeConfig, ShutdownSignal, StateChangeSignal};
use crate::runtime_files::{default_index_manifest, save_index_manifest_file};
#[cfg(test)]
use crate::runtime_files::{load_index_manifest_file, load_index_runtime_files};
#[cfg(test)]
use crate::state::TaskGenerationRegistry;
use crate::state::{
    BackgroundCommand, DaemonState, IndexCommand, InteractiveIndexRuntime, OwnedChildProcess,
    PendingWatchEvent, TaskGenerationId, TaskGenerationToken, TaskSequenceObserver, WatchEventMsg,
};
#[cfg(test)]
use crate::watch::WatchIngress;
use crate::watch::{cancel_task, register_task_and_watches, remove_watch, run_sequence_for_task};

pub use application::serve;

#[cfg(feature = "shared-repository-scan")]
pub mod shared_repository_scan;

const INTERACTIVE_INDEX_SCHEMA_VERSION: u32 = 2;
const INDEX_BATCH_DEBOUNCE_MS: u64 = 150;
const TASK_PERSISTENCE_DEBOUNCE_MS: u64 = 20;

fn persist_state(state: &DaemonState) -> Result<()> {
    mark_state_dirty(state).map(|_| ())
}

fn mark_state_dirty(state: &DaemonState) -> Result<u64> {
    state.persistence.checkpoint_async(
        Arc::new(state.tasks.clone()),
        Arc::new(state.watches.clone()),
    )
}

fn flush_persistence(state: &Arc<Mutex<DaemonState>>) -> Result<()> {
    let persistence = state.lock().map_err(lock_err)?.persistence.clone();
    persistence.flush().map(|_| ())
}

fn fence_task_namespace_admission(state: &Arc<Mutex<DaemonState>>, task_id: &str) -> Result<()> {
    let admission = {
        let guard = state.lock().map_err(lock_err)?;
        if !guard.tasks.tasks.contains_key(task_id) {
            anyhow::bail!("task '{task_id}' must exist before writing its managed namespace");
        }
        if guard.persistence.task_is_durably_admitted(task_id) {
            None
        } else {
            let revision = mark_state_dirty(&guard)?;
            Some((guard.persistence.clone(), revision))
        }
    };
    if let Some((persistence, revision)) = admission {
        persistence.ensure_task_admitted(task_id, revision)?;
    }
    Ok(())
}

fn reconcile_task_event_high_waters(
    tasks: &mut TaskRegistry,
    event_tails: &BTreeMap<String, Option<u64>>,
) -> Result<bool> {
    if tasks.tasks.len() != event_tails.len() {
        anyhow::bail!(
            "task registry/event-tail snapshot cardinality mismatch: {} tasks, {} tails",
            tasks.tasks.len(),
            event_tails.len()
        );
    }
    let mut changed = false;
    for (task_id, task) in &mut tasks.tasks {
        let durable_sequence = *event_tails
            .get(task_id)
            .ok_or_else(|| anyhow!("task '{task_id}' is missing from the event-tail snapshot"))?;
        match durable_sequence {
            None if task.last_event_seq == 0 => {}
            None => {
                anyhow::bail!(
                    "task registry high-water {} for '{}' is ahead of its missing event log",
                    task.last_event_seq,
                    task_id
                );
            }
            Some(durable_sequence) if task.last_event_seq > durable_sequence => {
                anyhow::bail!(
                    "task registry high-water {} for '{}' is ahead of durable event sequence {}",
                    task.last_event_seq,
                    task_id,
                    durable_sequence
                );
            }
            Some(durable_sequence) if task.last_event_seq < durable_sequence => {
                task.last_event_seq = durable_sequence;
                changed = true;
            }
            Some(_) => {}
        }
    }
    Ok(changed)
}

fn daemon_log(message: &str) {
    eprintln!(
        "[packet28d {}] {message}",
        packet28_daemon_core::storage::now_unix()
    );
}

fn resolve_root(path: &Path) -> PathBuf {
    let mut current = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    loop {
        if current.join(".git").exists() {
            return current;
        }
        if !current.pop() {
            return path.to_path_buf();
        }
    }
}

fn task_storage_id(value: &str) -> Result<TaskStorageId> {
    TaskStorageId::try_from(value)
        .map_err(|error| anyhow!("invalid task storage identifier {value:?}: {error}"))
}

fn context_version_storage_id(value: &str) -> Result<ContextVersionStorageId> {
    ContextVersionStorageId::try_from(value)
        .map_err(|error| anyhow!("invalid context-version storage identifier {value:?}: {error}"))
}

fn lock_err<T>(err: std::sync::PoisonError<T>) -> anyhow::Error {
    anyhow!("daemon state lock poisoned: {err}")
}

#[cfg(test)]
mod tests;
