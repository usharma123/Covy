extern crate packet28_binary_codec as wincode;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{BufReader, BufWriter, ErrorKind};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use context_kernel_core::{
    normalize_sequence_request, Kernel, KernelFailure, KernelRequest, KernelResponse,
    KernelStepRequest, PersistConfig, SequenceObserver,
};
use context_memory_core::{
    ContextStoreListFilter, ContextStorePaging, ContextStorePruneRequest, PacketCache,
    PersistConfig as MemoryPersistConfig, RecallOptions,
};
use diffy_core::model::CoverageFormat;
use notify::{Config, Event, PollWatcher, RecursiveMode, Watcher};
use packet28_daemon_core::storage::{
    append_task_event, ensure_daemon_dir, load_task_events, load_task_registry,
    load_watch_registry, now_unix, remove_runtime_files, save_task_registry, save_watch_registry,
    write_runtime_info,
};
use packet28_daemon_protocol::broker::{
    BrokerAction, BrokerDecision, BrokerDecomposeIntent, BrokerDecomposeRequest,
    BrokerDecomposeResponse, BrokerDecomposedStep, BrokerDeltaResponse,
    BrokerEstimateContextRequest, BrokerEstimateContextResponse, BrokerEvictionCandidate,
    BrokerGetContextRequest, BrokerGetContextResponse, BrokerPacketRef, BrokerPlanStep,
    BrokerPlanViolation, BrokerPrepareHandoffRequest, BrokerPrepareHandoffResponse, BrokerQuestion,
    BrokerRecommendedAction, BrokerResolvedQuestion, BrokerResponseMode, BrokerSection,
    BrokerSectionEstimate, BrokerSourceKind, BrokerSupersessionMode, BrokerTaskStatusRequest,
    BrokerTaskStatusResponse, BrokerToolResultKind, BrokerValidatePlanRequest,
    BrokerValidatePlanResponse, BrokerVerbosity, BrokerWriteOp, BrokerWriteStateBatchRequest,
    BrokerWriteStateBatchResponse, BrokerWriteStateRequest, BrokerWriteStateResponse,
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
use packet28_daemon_protocol::frame::{read_frame, write_frame};
use packet28_daemon_protocol::index::{
    DaemonIndexClearResponse, DaemonIndexManifest, DaemonIndexRebuildRequest,
    DaemonIndexRebuildResponse, DaemonIndexState, DaemonIndexStatusResponse,
};
use packet28_daemon_protocol::message::{
    DaemonEvent, DaemonEventFrame, DaemonRequest, DaemonResponse, DaemonRuntimeInfo, DaemonStatus,
};
use packet28_daemon_protocol::paths::{
    index_dir, index_manifest_path, index_snapshot_path, log_path, ready_path, socket_path,
    task_artifact_dir, task_brief_json_path, task_brief_markdown_path, task_event_log_path,
    task_state_json_path, task_version_json_path, workspace_socket_path,
};
use packet28_daemon_protocol::task::{
    TaskAwaitHandoffRequest, TaskAwaitHandoffResponse, TaskLaunchAgentRequest,
    TaskLaunchAgentResponse, TaskRecord, TaskRegistry, WatchRegistration, WatchRegistry,
};
use serde_json::{json, Value};

mod broker_context;
mod broker_handoff;
mod broker_limits;
mod broker_ops;
mod broker_render;
mod broker_search;
mod broker_search_plan;
mod broker_snapshot;
mod broker_support;
mod commands;
mod hooks;
mod index;
mod instruction_files;
mod launch;
mod planning;
mod runtime_files;
mod server;
mod state;
mod watch;

use crate::broker_context::{
    broker_decompose, broker_estimate_context, broker_get_context, broker_validate_plan,
    refresh_broker_context_for_task,
};
use crate::broker_handoff::{broker_prepare_handoff, compute_handoff_state};
use crate::broker_limits::*;
use crate::broker_ops::{broker_task_status, broker_write_state, broker_write_state_batch};
use crate::broker_render::*;
use crate::broker_search::*;
use crate::broker_search_plan::*;
use crate::broker_snapshot::*;
use crate::broker_support::*;
use crate::commands::{
    run_context_recall, run_context_store_get, run_context_store_list, run_context_store_prune,
    run_context_store_stats, run_cover_check, run_test_map, run_test_shard,
};
use crate::hooks::hook_ingest;
use crate::index::{
    build_index_status, daemon_index_clear, daemon_index_rebuild, daemon_index_status,
    daemon_packet28_search, enqueue_full_index_rebuild, enqueue_incremental_index_paths,
    spawn_index_worker,
};
use crate::instruction_files::resolve_instruction_file;
use crate::launch::{task_await_handoff, task_launch_agent};
use crate::planning::*;
use crate::runtime_files::{
    clear_index_files, default_index_manifest, load_index_manifest_file, load_index_snapshot_file,
    save_index_manifest_file, save_index_snapshot_file,
};
use crate::server::{handle_connection, handle_tcp_connection};
use crate::state::{
    CachedSourceFile, DaemonState, IndexCommand, InteractiveIndexRuntime, OwnedChildProcess,
    PendingWatchEvent, TaskGenerationId, TaskGenerationRegistry, TaskGenerationToken,
    TaskSequenceObserver, WatchEventMsg,
};
use crate::watch::{
    cancel_task, register_task_and_watches, remove_watch, restore_watchers, run_sequence_for_task,
    spawn_watch_processor,
};

#[derive(Parser)]
#[command(name = "packet28d", version, about = "Packet28 local daemon")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the daemon server for one workspace root
    Serve {
        #[arg(long, default_value = ".")]
        root: String,
    },
}

const DEFAULT_CONTEXT_MANAGE_BUDGET_TOKENS: u64 = 5_000;
const DEFAULT_CONTEXT_MANAGE_BUDGET_BYTES: usize = 32_000;
const INTERACTIVE_INDEX_SCHEMA_VERSION: u32 = 2;
const INDEX_BATCH_DEBOUNCE_MS: u64 = 150;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Serve { root } => serve(resolve_root(Path::new(&root))),
    }
}

fn serve(root: PathBuf) -> Result<()> {
    std::env::set_current_dir(&root)
        .with_context(|| format!("failed to set daemon cwd to '{}'", root.display()))?;
    ensure_daemon_dir(&root)?;
    let daemon_log_path = log_path(&root);
    let listener = bind_daemon_listener(&root)?;

    let runtime = DaemonRuntimeInfo {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        started_at_unix: now_unix(),
        ready_at_unix: None,
        socket_path: listener.endpoint(),
        workspace_root: root.to_string_lossy().to_string(),
        log_path: daemon_log_path.to_string_lossy().to_string(),
    };
    write_runtime_info(&root, &runtime)?;
    daemon_log(&format!(
        "starting packet28d pid={} root={} log={}",
        runtime.pid,
        root.display(),
        daemon_log_path.display()
    ));

    let kernel = Arc::new(Kernel::with_v1_reducers_and_persistence(
        PersistConfig::new(root.clone()),
    ));
    let tasks = load_task_registry(&root)?;
    let watches = load_watch_registry(&root)?;
    let manifest = load_index_manifest_file(&root);
    let snapshot = load_index_snapshot_file(&root, &manifest);
    let regex_runtime = packet28_search_core::load_runtime(&root).unwrap_or_default();
    let (index_tx, index_rx) = mpsc::channel();
    let state = Arc::new(Mutex::new(DaemonState {
        root: root.clone(),
        kernel,
        runtime,
        tasks,
        task_generations: TaskGenerationRegistry::default(),
        agent_snapshots: BTreeMap::new(),
        watches,
        watcher_handles: HashMap::new(),
        subscribers: HashMap::new(),
        source_file_cache: BTreeMap::new(),
        interactive_index: InteractiveIndexRuntime {
            manifest,
            snapshot,
            regex_runtime: Some(regex_runtime),
        },
        index_tx,
        shutting_down: false,
    }));

    let (watch_tx, watch_rx) = mpsc::channel();
    restore_watchers(&state, &watch_tx)?;
    spawn_watch_processor(state.clone(), watch_rx);
    spawn_index_worker(state.clone(), index_rx);
    {
        let should_queue = {
            let guard = state.lock().map_err(lock_err)?;
            guard.interactive_index.snapshot.is_none()
                || guard.interactive_index.regex_runtime.is_none()
                || guard
                    .interactive_index
                    .regex_runtime
                    .as_ref()
                    .is_some_and(|runtime| {
                        !runtime.is_loaded() || runtime.manifest.status != "ready"
                    })
                || guard.interactive_index.manifest.status != DaemonIndexState::Ready
        };
        if should_queue {
            let _ = enqueue_full_index_rebuild(&state);
        }
    }
    mark_ready(&state)?;

    loop {
        if state.lock().map_err(lock_err)?.shutting_down {
            break;
        }
        match listener.accept() {
            Ok(DaemonAcceptedStream::Unix(stream)) => {
                let state = state.clone();
                let watch_tx = watch_tx.clone();
                thread::spawn(move || {
                    if let Err(err) = handle_connection(state, watch_tx, stream) {
                        daemon_log(&format!("request handling failed: {err}"));
                    }
                });
            }
            Ok(DaemonAcceptedStream::Tcp(stream)) => {
                let state = state.clone();
                let watch_tx = watch_tx.clone();
                thread::spawn(move || {
                    if let Err(err) = handle_tcp_connection(state, watch_tx, stream) {
                        daemon_log(&format!("request handling failed: {err}"));
                    }
                });
            }
            Err(err) => {
                daemon_log(&format!("listener accept failed: {err}"));
                return Err(err.into());
            }
        }
    }

    daemon_log("shutting down packet28d");
    remove_runtime_files(&root)?;
    Ok(())
}

fn persist_state(state: &DaemonState) -> Result<()> {
    save_watch_registry(&state.root, &state.watches)?;
    save_task_registry(&state.root, &state.tasks)?;
    Ok(())
}

fn mark_ready(state: &Arc<Mutex<DaemonState>>) -> Result<()> {
    let (root, runtime) = {
        let mut guard = state.lock().map_err(lock_err)?;
        guard.runtime.ready_at_unix = Some(now_unix());
        (guard.root.clone(), guard.runtime.clone())
    };
    write_runtime_info(&root, &runtime)?;
    fs::write(
        ready_path(&root),
        format!("{}\n", runtime.ready_at_unix.unwrap_or_default()),
    )
    .with_context(|| format!("failed to write ready file for '{}'", root.display()))?;
    daemon_log(&format!(
        "daemon ready root={} socket={}",
        root.display(),
        runtime.socket_path
    ));
    Ok(())
}

fn wake_listener(root: &Path) {
    let _ = UnixStream::connect(socket_path(root))
        .or_else(|_| UnixStream::connect(workspace_socket_path(root)));
}

fn daemon_log(message: &str) {
    eprintln!("[packet28d {}] {message}", now_unix());
}

fn cleanup_socket_before_bind(socket: &Path) -> Result<()> {
    if socket.exists() {
        match UnixStream::connect(socket) {
            Ok(_) => {
                anyhow::bail!(
                    "packet28d is already running for '{}'; refusing to replace a live socket",
                    socket.display()
                );
            }
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::ConnectionRefused | ErrorKind::NotFound
                ) =>
            {
                daemon_log(&format!(
                    "removing stale socket '{}' after probe failure: {}",
                    socket.display(),
                    err
                ));
                fs::remove_file(socket).with_context(|| {
                    format!("failed to remove stale socket '{}'", socket.display())
                })?;
            }
            Err(err) => {
                daemon_log(&format!(
                    "removing unreachable socket '{}' after probe failure: {}",
                    socket.display(),
                    err
                ));
                fs::remove_file(socket).with_context(|| {
                    format!(
                        "failed to remove unreachable socket '{}' after probe failure",
                        socket.display()
                    )
                })?;
            }
        }
    }
    Ok(())
}

enum DaemonListener {
    Unix {
        endpoint: PathBuf,
        listener: UnixListener,
    },
    Tcp {
        endpoint: String,
        listener: TcpListener,
    },
}

enum DaemonAcceptedStream {
    Unix(UnixStream),
    Tcp(std::net::TcpStream),
}

impl DaemonListener {
    fn endpoint(&self) -> String {
        match self {
            DaemonListener::Unix { endpoint, .. } => endpoint.to_string_lossy().to_string(),
            DaemonListener::Tcp { endpoint, .. } => endpoint.clone(),
        }
    }

    fn accept(&self) -> std::io::Result<DaemonAcceptedStream> {
        match self {
            DaemonListener::Unix { listener, .. } => {
                let (stream, _) = listener.accept()?;
                Ok(DaemonAcceptedStream::Unix(stream))
            }
            DaemonListener::Tcp { listener, .. } => {
                let (stream, _) = listener.accept()?;
                Ok(DaemonAcceptedStream::Tcp(stream))
            }
        }
    }
}

fn bind_daemon_listener(root: &Path) -> Result<DaemonListener> {
    if std::env::var("PACKET28D_FORCE_TCP")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
    {
        return bind_tcp_listener("PACKET28D_FORCE_TCP requested TCP daemon transport");
    }
    let primary = socket_path(root);
    cleanup_socket_before_bind(&primary)?;
    match UnixListener::bind(&primary) {
        Ok(listener) => Ok(DaemonListener::Unix {
            endpoint: primary,
            listener,
        }),
        Err(primary_err) if bind_io_error_is_permission_denied(&primary_err) => {
            let fallback = workspace_socket_path(root);
            daemon_log(&format!(
                "falling back to workspace socket '{}' after temp socket bind failed: {primary_err}",
                fallback.display()
            ));
            cleanup_socket_before_bind(&fallback)?;
            match UnixListener::bind(&fallback) {
                Ok(listener) => Ok(DaemonListener::Unix {
                    endpoint: fallback,
                    listener,
                }),
                Err(fallback_err) => {
                    daemon_log(&format!(
                        "falling back to TCP after workspace socket bind failed: {fallback_err}"
                    ));
                    bind_tcp_listener(&format!(
                        "Unix sockets '{}' and '{}' were denied",
                        primary.display(),
                        fallback.display()
                    ))
                }
            }
        }
        Err(err) => Err(err).with_context(|| format!("failed to bind '{}'", primary.display())),
    }
}

fn bind_io_error_is_permission_denied(err: &std::io::Error) -> bool {
    err.kind() == ErrorKind::PermissionDenied
        || err.raw_os_error() == Some(1)
        || err.to_string().contains("Operation not permitted")
}

fn bind_tcp_listener(reason: &str) -> Result<DaemonListener> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .with_context(|| format!("failed to bind TCP fallback after {reason}"))?;
    let endpoint = format!("tcp://{}", listener.local_addr()?);
    daemon_log(&format!(
        "using TCP daemon endpoint '{endpoint}' ({reason})"
    ));
    Ok(DaemonListener::Tcp { endpoint, listener })
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

fn lock_err<T>(err: std::sync::PoisonError<T>) -> anyhow::Error {
    anyhow!("daemon state lock poisoned: {err}")
}

#[cfg(test)]
mod tests;
