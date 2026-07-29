use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
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
use packet28_daemon_core::retention::recover_task_store_quarantine_and_acquire_daemon_lease;
use packet28_daemon_core::storage::{
    append_task_event, ensure_daemon_dir, load_task_events, load_task_registry,
    load_watch_registry, now_unix, remove_runtime_files, save_task_registry, save_watch_registry,
    write_runtime_info,
};
use packet28_daemon_core::task_store_lease::acquire_daemon_instance_lease;
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
use packet28_daemon_protocol::frame::write_frame;
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
    task_state_json_path, task_version_json_path, workspace_socket_path, ContextVersionStorageId,
    TaskStorageId,
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
mod runtime;
mod runtime_files;
#[cfg(unix)]
mod runtime_files_unix;
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
    enqueue_initial_index_work, run_index_worker, IndexIngress, IndexWorkReceiver,
};
use crate::instruction_files::resolve_instruction_file;
use crate::launch::task_launch_agent;
use crate::planning::*;
use crate::runtime::{BlockingPool, DaemonRuntimeConfig, ShutdownSignal, StateChangeSignal};
use crate::runtime_files::{
    default_index_manifest, load_index_manifest_file, load_index_runtime_files,
    save_index_manifest_file,
};
use crate::server::handle_connection;
use crate::state::{
    BackgroundCommand, CachedSourceFile, DaemonState, IndexCommand, InteractiveIndexRuntime,
    OwnedChildProcess, PendingWatchEvent, TaskGenerationId, TaskGenerationRegistry,
    TaskGenerationToken, TaskSequenceObserver, WatchEventMsg,
};
use crate::watch::{
    cancel_task, register_task_and_watches, remove_watch, restore_watchers, run_sequence_for_task,
    run_watch_processor, WatchIngress,
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
    let daemon_instance_lease = acquire_daemon_instance_lease(&root)?;
    let (recovery, _task_store_lease) =
        recover_task_store_quarantine_and_acquire_daemon_lease(&root, &daemon_instance_lease)?;
    if recovery.restored_precommit_groups > 0
        || recovery.completed_committed_groups > 0
        || !recovery.issues.is_empty()
    {
        daemon_log(&format!(
            "task-store recovery restored={} completed={} issues={}",
            recovery.restored_precommit_groups,
            recovery.completed_committed_groups,
            recovery.issues.len()
        ));
    }
    ensure_daemon_dir(&root)?;
    let config = DaemonRuntimeConfig::from_env()?;
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
    let interactive_index = load_index_runtime_files(&root, manifest);
    let (index_tx, index_rx) = IndexIngress::new();
    let (background_tx, background_rx) =
        tokio::sync::mpsc::channel(config.background_queue_capacity);
    let shutdown = ShutdownSignal::new();
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
        interactive_index,
        index_tx,
        background_tx,
        shutdown: shutdown.clone(),
        changes: StateChangeSignal::new(),
        shutting_down: false,
    }));

    let (watch_tx, watch_rx) = WatchIngress::new(config.watch_queue_capacity);
    restore_watchers(&state, &watch_tx)?;
    enqueue_initial_index_work(&state)?;
    mark_ready(&state)?;

    let blocking_pool = BlockingPool::new(config.max_blocking_operations);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("packet28d-runtime")
        .build()
        .context("failed to create packet28d Tokio runtime")?;
    let runtime_result = runtime.block_on(run_daemon_runtime(DaemonRuntimeInputs {
        listener,
        state: state.clone(),
        watch_tx,
        watch_rx,
        background_rx,
        index_rx,
        blocking_pool,
        config: config.clone(),
    }));
    shutdown.request();
    runtime.shutdown_timeout(config.shutdown_grace);

    daemon_log("shutting down packet28d");
    let cleanup_result = remove_runtime_files(&root);
    runtime_result?;
    cleanup_result?;
    Ok(())
}

struct DaemonRuntimeInputs {
    listener: DaemonListener,
    state: Arc<Mutex<DaemonState>>,
    watch_tx: WatchIngress,
    watch_rx: tokio::sync::mpsc::Receiver<WatchEventMsg>,
    background_rx: tokio::sync::mpsc::Receiver<BackgroundCommand>,
    index_rx: IndexWorkReceiver,
    blocking_pool: BlockingPool,
    config: DaemonRuntimeConfig,
}

async fn run_daemon_runtime(inputs: DaemonRuntimeInputs) -> Result<()> {
    let DaemonRuntimeInputs {
        listener,
        state,
        watch_tx,
        watch_rx,
        background_rx,
        index_rx,
        blocking_pool,
        config,
    } = inputs;
    let mut watch_task = tokio::spawn(run_watch_processor(
        state.clone(),
        watch_tx.clone(),
        watch_rx,
        blocking_pool.clone(),
    ));
    let mut background_task = tokio::spawn(run_background_tasks(
        state.clone(),
        background_rx,
        blocking_pool.clone(),
    ));
    let index_state = state.clone();
    let mut index_task = tokio::task::spawn_blocking(move || {
        if let Err(error) = run_index_worker(index_state, index_rx) {
            daemon_log(&format!("index worker stopped: {error:#}"));
        }
    });
    let transport_result = run_transport(
        listener,
        state.clone(),
        watch_tx,
        blocking_pool,
        config.clone(),
    )
    .await;

    let shutdown = {
        let mut guard = state.lock().map_err(lock_err)?;
        guard.shutting_down = true;
        guard.watcher_handles.clear();
        let _ = guard.index_tx.send(IndexCommand::Shutdown);
        guard.shutdown.clone()
    };
    shutdown.request();
    join_owned_runtime_task("watch processor", config.shutdown_grace, &mut watch_task).await;
    join_owned_runtime_task(
        "background processor",
        config.shutdown_grace,
        &mut background_task,
    )
    .await;
    join_owned_runtime_task("index worker", config.shutdown_grace, &mut index_task).await;
    transport_result
}

async fn join_owned_runtime_task(
    name: &str,
    grace: Duration,
    task: &mut tokio::task::JoinHandle<()>,
) {
    match tokio::time::timeout(grace, &mut *task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            daemon_log(&format!("{name} failed to join: {error}"));
        }
        Err(_) => {
            daemon_log(&format!("{name} exceeded bounded shutdown grace"));
            task.abort();
        }
    }
}

async fn run_background_tasks(
    state: Arc<Mutex<DaemonState>>,
    mut receiver: tokio::sync::mpsc::Receiver<BackgroundCommand>,
    blocking_pool: BlockingPool,
) {
    let mut shutdown = match state.lock().map_err(lock_err) {
        Ok(guard) => guard.shutdown.subscribe(),
        Err(error) => {
            daemon_log(&format!(
                "background processor could not subscribe to shutdown: {error}"
            ));
            return;
        }
    };
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            command = receiver.recv() => {
                let Some(command) = command else {
                    break;
                };
                let task_state = state.clone();
                let task_pool = blocking_pool.clone();
                tasks.spawn(async move {
                    let BackgroundCommand::RelaunchAgent { task_id, command } =
                        command;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let log_task_id = task_id.clone();
                    let result = task_pool
                        .run(move || {
                            task_launch_agent(
                                task_state,
                                TaskLaunchAgentRequest {
                                    task_id,
                                    task: None,
                                    wait_for_handoff: false,
                                    handoff_timeout_ms: None,
                                    handoff_poll_ms: None,
                                    command,
                                },
                            )
                        })
                        .await;
                    match result {
                        Ok(launched) => daemon_log(&format!(
                            "auto-relaunched agent pid={} task={log_task_id}",
                            launched.pid
                        )),
                        Err(error) => daemon_log(&format!(
                            "auto-relaunch failed for task {log_task_id}: {error:#}"
                        )),
                        }
                });
            }
            joined = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(Err(error)) = joined {
                    daemon_log(&format!("background task failed to join: {error}"));
                }
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}

async fn run_transport(
    listener: DaemonListener,
    state: Arc<Mutex<DaemonState>>,
    watch_tx: WatchIngress,
    blocking_pool: BlockingPool,
    config: DaemonRuntimeConfig,
) -> Result<()> {
    let listener = listener.into_async()?;
    let permits = Arc::new(tokio::sync::Semaphore::new(config.max_connections));
    let mut shutdown = state.lock().map_err(lock_err)?.shutdown.subscribe();
    let mut connections = tokio::task::JoinSet::new();

    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                let stream = accepted.context("daemon listener accept failed")?;
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    daemon_log(&format!(
                        "connection rejected: active connection cap {} reached",
                        config.max_connections
                    ));
                    continue;
                };
                let connection_state = state.clone();
                let connection_watch_tx = watch_tx.clone();
                let connection_pool = blocking_pool.clone();
                let connection_config = config.clone();
                match stream {
                    DaemonAcceptedStream::Unix(stream) => {
                        connections.spawn(async move {
                            let _permit = permit;
                            handle_connection(
                                connection_state,
                                connection_watch_tx,
                                stream,
                                connection_config,
                                connection_pool,
                            )
                            .await
                        });
                    }
                    DaemonAcceptedStream::Tcp(stream) => {
                        connections.spawn(async move {
                            let _permit = permit;
                            handle_connection(
                                connection_state,
                                connection_watch_tx,
                                stream,
                                connection_config,
                                connection_pool,
                            )
                            .await
                        });
                    }
                }
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                log_connection_join(joined);
            }
        }
    }
    drop(listener);

    let deadline = tokio::time::Instant::now() + config.shutdown_grace;
    while !connections.is_empty() {
        match tokio::time::timeout_at(deadline, connections.join_next()).await {
            Ok(joined) => log_connection_join(joined),
            Err(_) => {
                daemon_log(&format!(
                    "{} connection task(s) exceeded bounded shutdown grace",
                    connections.len()
                ));
                connections.abort_all();
                while connections.join_next().await.is_some() {}
                break;
            }
        }
    }
    Ok(())
}

fn log_connection_join(joined: Option<std::result::Result<Result<()>, tokio::task::JoinError>>) {
    match joined {
        Some(Ok(Err(error))) if !is_benign_connection_error(&error) => {
            daemon_log(&format!("request handling failed: {error}"));
        }
        Some(Ok(Err(_))) => {}
        Some(Err(error)) if !error.is_cancelled() => {
            daemon_log(&format!("connection task failed to join: {error}"));
        }
        Some(Ok(Ok(()))) | Some(Err(_)) | None => {}
    }
}

fn is_benign_connection_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| {
                matches!(
                    io_error.kind(),
                    ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::UnexpectedEof
                )
            })
    })
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
    Unix(tokio::net::UnixStream),
    Tcp(tokio::net::TcpStream),
}

enum AsyncDaemonListener {
    Unix(tokio::net::UnixListener),
    Tcp(tokio::net::TcpListener),
}

impl DaemonListener {
    fn endpoint(&self) -> String {
        match self {
            DaemonListener::Unix { endpoint, .. } => endpoint.to_string_lossy().to_string(),
            DaemonListener::Tcp { endpoint, .. } => endpoint.clone(),
        }
    }

    fn into_async(self) -> Result<AsyncDaemonListener> {
        match self {
            DaemonListener::Unix { listener, .. } => {
                listener.set_nonblocking(true)?;
                Ok(AsyncDaemonListener::Unix(
                    tokio::net::UnixListener::from_std(listener)?,
                ))
            }
            DaemonListener::Tcp { listener, .. } => {
                listener.set_nonblocking(true)?;
                Ok(AsyncDaemonListener::Tcp(tokio::net::TcpListener::from_std(
                    listener,
                )?))
            }
        }
    }
}

impl AsyncDaemonListener {
    async fn accept(&self) -> std::io::Result<DaemonAcceptedStream> {
        match self {
            Self::Unix(listener) => {
                let (stream, _) = listener.accept().await?;
                Ok(DaemonAcceptedStream::Unix(stream))
            }
            Self::Tcp(listener) => {
                let (stream, _) = listener.accept().await?;
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
