use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
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
    ContextStoreListFilter, ContextStorePaging, ContextStorePruneRequest, RecallOptions,
};
use diffy_core::model::CoverageFormat;
use notify::{Config, Event, PollWatcher, RecursiveMode, Watcher};
use packet28_daemon_core::retention::recover_task_store_quarantine_and_acquire_daemon_lease;
use packet28_daemon_core::storage::{
    ensure_daemon_dir, load_task_events, load_task_registry_with_event_tails, load_watch_registry,
    now_unix, remove_runtime_files, write_runtime_info,
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
use crate::kernel_registry::PersistentKernelRegistry;
use crate::launch::task_launch_agent;
use crate::persistence::PersistenceOwner;
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
const TASK_PERSISTENCE_DEBOUNCE_MS: u64 = 20;

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
    let (recovery, task_store_lease) =
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
    let kernel_registry = Arc::new(PersistentKernelRegistry::new(
        &root,
        kernel.clone(),
        config.max_persistent_roots,
    )?);
    let (mut tasks, event_tails) = load_task_registry_with_event_tails(&root)?;
    let watches = load_watch_registry(&root)?;
    let (persistence_owner, persistence) = PersistenceOwner::start(
        root.clone(),
        task_store_lease.clone(),
        Duration::from_millis(TASK_PERSISTENCE_DEBOUNCE_MS),
        &tasks,
    )?;
    if reconcile_task_event_high_waters(&mut tasks, &event_tails)? {
        persistence.checkpoint(Arc::new(tasks.clone()), Arc::new(watches.clone()))?;
    }
    let manifest = load_index_manifest_file(&root);
    let interactive_index = load_index_runtime_files(&root, manifest);
    let (index_tx, index_rx) = IndexIngress::new();
    let (background_tx, background_rx) =
        tokio::sync::mpsc::channel(config.background_queue_capacity);
    let shutdown = ShutdownSignal::new();
    let state = Arc::new(Mutex::new(DaemonState {
        root: root.clone(),
        kernel,
        kernel_registry,
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
        persistence,
        #[cfg(test)]
        _persistence_owner: None,
        shutdown: shutdown.clone(),
        changes: StateChangeSignal::new(),
        shutting_down: false,
    }));

    let (watch_tx, watch_rx) = WatchIngress::new(config.watch_queue_capacity);
    restore_watchers(&state, &watch_tx)?;
    enqueue_initial_index_work(&state)?;
    mark_ready(&state)?;

    let blocking_pool = BlockingPool::with_lifecycle_leases(
        config.max_blocking_operations,
        daemon_instance_lease.clone(),
        task_store_lease.clone(),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("packet28d-runtime")
        .build()
        .context("failed to create packet28d Tokio runtime")?;
    let runtime_outcome = runtime.block_on(run_daemon_runtime(DaemonRuntimeInputs {
        listener,
        state: state.clone(),
        watch_tx,
        watch_rx,
        background_rx,
        index_rx,
        blocking_pool,
        daemon_instance_lease: daemon_instance_lease.clone(),
        task_store_lease: task_store_lease.clone(),
        config: config.clone(),
    }));
    shutdown.request();
    let shutdown_deadline = runtime_outcome.deadline;
    let mut lifecycle_result = runtime_outcome.result;
    record_runtime_result(
        &mut lifecycle_result,
        shutdown_persistent_kernels(&state, remaining_until(shutdown_deadline)),
    );
    record_runtime_result(
        &mut lifecycle_result,
        persistence_owner
            .shutdown(remaining_until(shutdown_deadline))
            .map(|_| ())
            .context("failed to shut down daemon task persistence"),
    );
    runtime.shutdown_timeout(remaining_until(shutdown_deadline));

    daemon_log("shutting down packet28d");
    let cleanup_result = remove_runtime_files(&root).map_err(anyhow::Error::from);
    // The supervisor has joined uncancellable index work and waited for every
    // admitted blocking mutation before the task-store lease can be released.
    // Runtime-file cleanup is part of the same lifecycle ownership window.
    drop(task_store_lease);
    drop(daemon_instance_lease);
    record_runtime_result(&mut lifecycle_result, cleanup_result);
    lifecycle_result
}

fn shutdown_persistent_kernels(state: &Arc<Mutex<DaemonState>>, timeout: Duration) -> Result<()> {
    let registry = state.lock().map_err(lock_err)?.kernel_registry.clone();
    let kernels = registry.kernels()?;
    let started = Instant::now();
    let mut result = Ok(());
    let kernel_count = kernels.len();
    for (index, kernel) in kernels.into_iter().enumerate() {
        let remaining = timeout.saturating_sub(started.elapsed());
        let roots_remaining = u32::try_from(kernel_count.saturating_sub(index)).unwrap_or(u32::MAX);
        let root_budget = remaining / roots_remaining.max(1);
        record_runtime_result(
            &mut result,
            kernel
                .shutdown_cache_persistence(root_budget)
                .map(|_| ())
                .context("failed to shut down daemon cache persistence"),
        );
    }
    result
}

struct DaemonRuntimeInputs {
    listener: DaemonListener,
    state: Arc<Mutex<DaemonState>>,
    watch_tx: WatchIngress,
    watch_rx: tokio::sync::mpsc::Receiver<WatchEventMsg>,
    background_rx: tokio::sync::mpsc::Receiver<BackgroundCommand>,
    index_rx: IndexWorkReceiver,
    blocking_pool: BlockingPool,
    daemon_instance_lease: packet28_daemon_core::task_store_lease::TaskStoreLease,
    task_store_lease: packet28_daemon_core::task_store_lease::TaskStoreLease,
    config: DaemonRuntimeConfig,
}

struct DaemonRuntimeOutcome {
    result: Result<()>,
    deadline: Instant,
}

impl DaemonRuntimeOutcome {
    fn with_new_deadline(result: Result<()>, grace: Duration) -> Self {
        Self {
            result,
            deadline: Instant::now() + grace,
        }
    }
}

fn remaining_until(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

async fn run_daemon_runtime(inputs: DaemonRuntimeInputs) -> DaemonRuntimeOutcome {
    let DaemonRuntimeInputs {
        listener,
        state,
        watch_tx,
        watch_rx,
        background_rx,
        index_rx,
        blocking_pool,
        daemon_instance_lease,
        task_store_lease,
        config,
    } = inputs;
    let shutdown = match state.lock().map_err(lock_err) {
        Ok(guard) => guard.shutdown.clone(),
        Err(error) => {
            return DaemonRuntimeOutcome::with_new_deadline(Err(error), config.shutdown_grace)
        }
    };
    let watch_task = tokio::spawn(run_watch_processor(
        state.clone(),
        watch_tx.clone(),
        watch_rx,
        blocking_pool.clone(),
    ));
    let background_task = tokio::spawn(run_background_tasks(
        state.clone(),
        background_rx,
        blocking_pool.clone(),
    ));
    let index_state = state.clone();
    let index_task = tokio::task::spawn_blocking(move || {
        let _daemon_instance_lease = daemon_instance_lease;
        let _task_store_lease = task_store_lease;
        run_index_worker(index_state, index_rx)
    });
    let transport_task = tokio::spawn(run_transport(
        listener,
        state.clone(),
        watch_tx,
        blocking_pool.clone(),
        config.clone(),
    ));

    supervise_daemon_tasks(
        state,
        shutdown,
        blocking_pool,
        config.shutdown_grace,
        DaemonRuntimeTasks {
            transport: transport_task,
            watch: watch_task,
            background: background_task,
            index: index_task,
        },
    )
    .await
}

struct DaemonRuntimeTasks {
    transport: tokio::task::JoinHandle<Result<()>>,
    watch: tokio::task::JoinHandle<Result<()>>,
    background: tokio::task::JoinHandle<Result<()>>,
    index: tokio::task::JoinHandle<Result<()>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DaemonRuntimeTask {
    Transport,
    Watch,
    Background,
    Index,
}

impl DaemonRuntimeTask {
    const fn name(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::Watch => "watch processor",
            Self::Background => "background processor",
            Self::Index => "index worker",
        }
    }
}

async fn supervise_daemon_tasks(
    state: Arc<Mutex<DaemonState>>,
    shutdown: ShutdownSignal,
    blocking_pool: BlockingPool,
    grace: Duration,
    tasks: DaemonRuntimeTasks,
) -> DaemonRuntimeOutcome {
    let DaemonRuntimeTasks {
        mut transport,
        mut watch,
        mut background,
        mut index,
    } = tasks;
    let mut shutdown_receiver = shutdown.subscribe();
    let trigger = tokio::select! {
        biased;
        () = wait_for_shutdown_request(&mut shutdown_receiver) => DaemonRuntimeTrigger::Shutdown,
        result = &mut transport => {
            DaemonRuntimeTrigger::TaskExit(DaemonRuntimeTask::Transport, result)
        }
        result = &mut watch => {
            DaemonRuntimeTrigger::TaskExit(DaemonRuntimeTask::Watch, result)
        }
        result = &mut background => {
            DaemonRuntimeTrigger::TaskExit(DaemonRuntimeTask::Background, result)
        }
        result = &mut index => {
            DaemonRuntimeTrigger::TaskExit(DaemonRuntimeTask::Index, result)
        }
    };
    let deadline = Instant::now() + grace;
    let (first_task, mut result) = match trigger {
        DaemonRuntimeTrigger::Shutdown => (None, Ok(())),
        DaemonRuntimeTrigger::TaskExit(task, exit) => {
            (Some(task), classify_first_runtime_exit(task, exit))
        }
    };

    shutdown.request();
    blocking_pool.request_shutdown();
    let shutdown_start = begin_daemon_shutdown(&state);
    record_runtime_result(&mut result, shutdown_start.result);

    if first_task != Some(DaemonRuntimeTask::Transport) {
        record_runtime_result(
            &mut result,
            join_abortable_runtime_task(
                DaemonRuntimeTask::Transport.name(),
                deadline,
                &mut transport,
            )
            .await,
        );
    }
    if first_task != Some(DaemonRuntimeTask::Watch) {
        record_runtime_result(
            &mut result,
            join_abortable_runtime_task(DaemonRuntimeTask::Watch.name(), deadline, &mut watch)
                .await,
        );
    }
    if first_task != Some(DaemonRuntimeTask::Background) {
        record_runtime_result(
            &mut result,
            join_abortable_runtime_task(
                DaemonRuntimeTask::Background.name(),
                deadline,
                &mut background,
            )
            .await,
        );
    }
    if first_task != Some(DaemonRuntimeTask::Index) {
        record_runtime_result(
            &mut result,
            join_blocking_runtime_task(DaemonRuntimeTask::Index.name(), deadline, &mut index).await,
        );
    }
    if tokio::time::timeout_at(
        tokio::time::Instant::from_std(deadline),
        blocking_pool.wait_for_idle(),
    )
    .await
    .is_err()
    {
        record_runtime_result(
            &mut result,
            Err(anyhow!(
                "{} blocking operation(s) exceeded shutdown grace and remain lease-owned",
                blocking_pool.active_operations()
            )),
        );
    }
    DaemonRuntimeOutcome { result, deadline }
}

enum DaemonRuntimeTrigger {
    Shutdown,
    TaskExit(
        DaemonRuntimeTask,
        std::result::Result<Result<()>, tokio::task::JoinError>,
    ),
}

async fn wait_for_shutdown_request(receiver: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

struct DaemonShutdownStart {
    result: Result<()>,
}

fn begin_daemon_shutdown(state: &Arc<Mutex<DaemonState>>) -> DaemonShutdownStart {
    let (root, cancelled_generations, mut result) = {
        let mut guard = match state.lock().map_err(lock_err) {
            Ok(guard) => guard,
            Err(error) => {
                return DaemonShutdownStart { result: Err(error) };
            }
        };
        guard.shutting_down = true;
        let cancelled_generations = guard.task_generations.request_cancel_all();
        guard.watcher_handles.clear();
        (
            guard.root.clone(),
            cancelled_generations,
            guard.index_tx.send(IndexCommand::Shutdown),
        )
    };
    if cancelled_generations > 0 {
        daemon_log(&format!(
            "requested cancellation for {} active task generation(s)",
            cancelled_generations
        ));
    }
    match fs::remove_file(ready_path(&root)) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            record_runtime_result(
                &mut result,
                Err(error).with_context(|| {
                    format!("failed to withdraw readiness for '{}'", root.display())
                }),
            );
        }
    }
    DaemonShutdownStart { result }
}

fn classify_first_runtime_exit(
    task: DaemonRuntimeTask,
    exit: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    classify_runtime_join(task.name(), exit)?;
    anyhow::bail!("{} exited before daemon shutdown", task.name())
}

fn classify_runtime_join(
    name: &str,
    joined: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    joined
        .map_err(|error| anyhow!("{name} failed to join: {error}"))?
        .with_context(|| format!("{name} failed"))
}

fn record_runtime_result(primary: &mut Result<()>, candidate: Result<()>) {
    let Err(error) = candidate else {
        return;
    };
    if primary.is_ok() {
        *primary = Err(error);
    } else {
        daemon_log(&format!("additional daemon shutdown failure: {error:#}"));
    }
}

async fn join_abortable_runtime_task(
    name: &str,
    deadline: Instant,
    task: &mut tokio::task::JoinHandle<Result<()>>,
) -> Result<()> {
    match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), &mut *task).await {
        Ok(joined) => classify_runtime_join(name, joined),
        Err(_) => {
            daemon_log(&format!(
                "{name} exceeded shutdown grace; aborting async owner"
            ));
            task.abort();
            if let Err(error) = task.await {
                if !error.is_cancelled() {
                    daemon_log(&format!("{name} failed while being reaped: {error}"));
                }
            }
            anyhow::bail!("{name} exceeded shutdown grace")
        }
    }
}

async fn join_blocking_runtime_task(
    name: &str,
    deadline: Instant,
    task: &mut tokio::task::JoinHandle<Result<()>>,
) -> Result<()> {
    match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), &mut *task).await {
        Ok(joined) => classify_runtime_join(name, joined),
        Err(_) => {
            daemon_log(&format!(
                "{name} exceeded shutdown grace; detaching lease-owned blocking work"
            ));
            task.abort();
            anyhow::bail!("{name} exceeded shutdown grace and remains lease-owned")
        }
    }
}

async fn run_background_tasks(
    state: Arc<Mutex<DaemonState>>,
    mut receiver: tokio::sync::mpsc::Receiver<BackgroundCommand>,
    blocking_pool: BlockingPool,
) -> Result<()> {
    let mut shutdown = state.lock().map_err(lock_err)?.shutdown.subscribe();
    let max_pending = blocking_pool.max_operations();
    let mut pending = VecDeque::<(tokio::time::Instant, BackgroundCommand)>::new();
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        let next_ready = pending.front().map(|(ready_at, _)| *ready_at);
        let command_is_ready =
            next_ready.is_some_and(|ready_at| ready_at <= tokio::time::Instant::now());
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            joined = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(Err(error)) = joined {
                    if error.is_cancelled() {
                        continue;
                    }
                    return Err(anyhow!("background task failed to join: {error}"));
                }
            }
            admission = blocking_pool.admit(),
                if command_is_ready && tasks.len() < max_pending => {
                let admission = admission?;
                let Some((_, command)) = pending.pop_front() else {
                    drop(admission);
                    continue;
                };
                let task_state = state.clone();
                tasks.spawn(async move {
                    let BackgroundCommand::RelaunchAgent { task_id, command } =
                        command;
                    let log_task_id = task_id.clone();
                    let result = admission
                        .run_cancellable(move |_| {
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
            command = receiver.recv(), if pending.len() < max_pending => {
                let Some(command) = command else {
                    break;
                };
                pending.push_back((
                    tokio::time::Instant::now() + Duration::from_millis(500),
                    command,
                ));
            }
            () = sleep_until_optional(next_ready), if next_ready.is_some() && !command_is_ready => {}
        }
    }
    tasks.abort_all();
    while let Some(joined) = tasks.join_next().await {
        if let Err(error) = joined {
            if !error.is_cancelled() {
                return Err(anyhow!("background task failed to join: {error}"));
            }
        }
    }
    Ok(())
}

async fn sleep_until_optional(deadline: Option<tokio::time::Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    }
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
        while let Some(joined) = connections.try_join_next() {
            log_connection_join(Some(joined));
        }
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                log_connection_join(joined);
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
        }
    }
    drop(listener);

    while let Some(joined) = connections.join_next().await {
        log_connection_join(Some(joined));
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
