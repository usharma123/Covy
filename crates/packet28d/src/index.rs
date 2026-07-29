use super::*;

const INDEX_WAKE_QUEUE_CAPACITY: usize = 1;
const MAX_PENDING_INDEX_PATHS: usize = 4_096;
const MAX_INDEX_PATH_BYTES: usize = 4_096;

#[derive(Clone)]
pub(crate) struct IndexIngress {
    pending: Arc<Mutex<PendingIndexWork>>,
    wake: std::sync::mpsc::SyncSender<()>,
}

pub(crate) struct IndexWorkReceiver {
    pending: Arc<Mutex<PendingIndexWork>>,
    wake: std::sync::mpsc::Receiver<()>,
}

#[derive(Default)]
struct PendingIndexWork {
    next_epoch: u64,
    clear_epoch: Option<u64>,
    full_rebuild_epoch: Option<u64>,
    paths: BTreeMap<String, u64>,
    shutdown_epoch: Option<u64>,
}

struct IndexWorkBatch {
    clear: bool,
    full_rebuild: bool,
    paths: Vec<String>,
    shutdown: bool,
    epoch: u64,
}

#[derive(Default)]
struct IndexFollowUp {
    full_rebuild: bool,
    paths: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexBatchStatus {
    Complete,
    Requeued,
}

#[derive(Debug)]
pub(crate) struct DaemonIndexSearchNotReady {
    reason: String,
}

impl std::fmt::Display for DaemonIndexSearchNotReady {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "indexed search is not ready: {}", self.reason)
    }
}

impl std::error::Error for DaemonIndexSearchNotReady {}

#[derive(Debug)]
pub(crate) struct IndexQueueOutcome {
    pub(crate) full: bool,
    pub(crate) queued_paths: Vec<String>,
}

impl IndexIngress {
    pub(crate) fn new() -> (Self, IndexWorkReceiver) {
        let (wake, receiver) = std::sync::mpsc::sync_channel(INDEX_WAKE_QUEUE_CAPACITY);
        let pending = Arc::new(Mutex::new(PendingIndexWork::default()));
        (
            Self {
                pending: pending.clone(),
                wake,
            },
            IndexWorkReceiver {
                pending,
                wake: receiver,
            },
        )
    }

    pub(crate) fn send(&self, command: IndexCommand) -> Result<()> {
        {
            let mut pending = self.pending.lock().map_err(lock_err)?;
            if pending.shutdown_epoch.is_some() && !matches!(command, IndexCommand::Shutdown) {
                anyhow::bail!("index worker is shutting down");
            }
            pending.next_epoch = pending
                .next_epoch
                .checked_add(1)
                .ok_or_else(|| anyhow!("index ingress epoch exhausted"))?;
            let epoch = pending.next_epoch;
            match command {
                IndexCommand::Clear => {
                    pending.clear_epoch = Some(epoch);
                    pending.full_rebuild_epoch = None;
                    pending.paths.clear();
                }
                IndexCommand::RebuildFull => {
                    pending.full_rebuild_epoch = Some(epoch);
                    pending.paths.clear();
                }
                IndexCommand::ReindexPaths(paths) if pending.full_rebuild_epoch.is_none() => {
                    for path in paths {
                        if path.len() > MAX_INDEX_PATH_BYTES {
                            pending.full_rebuild_epoch = Some(epoch);
                            pending.paths.clear();
                            break;
                        }
                        pending.paths.insert(path, epoch);
                        if pending.paths.len() > MAX_PENDING_INDEX_PATHS {
                            pending.full_rebuild_epoch = Some(epoch);
                            pending.paths.clear();
                            break;
                        }
                    }
                }
                IndexCommand::ReindexPaths(_) => {}
                IndexCommand::Shutdown => {
                    pending.shutdown_epoch = Some(epoch);
                }
            }
        }
        match self.wake.try_send(()) {
            Ok(()) | Err(std::sync::mpsc::TrySendError::Full(())) => Ok(()),
            Err(std::sync::mpsc::TrySendError::Disconnected(())) => {
                anyhow::bail!("index worker is not running")
            }
        }
    }

    #[cfg(test)]
    fn pending_counts(&self) -> (usize, bool) {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (pending.paths.len(), pending.full_rebuild_epoch.is_some())
    }

    fn follow_up_after(&self, epoch: u64) -> Result<IndexFollowUp> {
        let pending = self.pending.lock().map_err(lock_err)?;
        Ok(IndexFollowUp {
            full_rebuild: pending
                .full_rebuild_epoch
                .is_some_and(|command_epoch| command_epoch > epoch),
            paths: pending
                .paths
                .iter()
                .filter_map(|(path, command_epoch)| {
                    (*command_epoch > epoch).then_some(path.clone())
                })
                .collect(),
        })
    }
}

impl IndexWorkReceiver {
    fn recv_debounced(&self) -> Result<IndexWorkBatch> {
        self.wake
            .recv()
            .map_err(|_| anyhow!("index ingress disconnected"))?;
        loop {
            match self
                .wake
                .recv_timeout(Duration::from_millis(INDEX_BATCH_DEBOUNCE_MS))
            {
                Ok(()) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let mut pending = self.pending.lock().map_err(lock_err)?;
        let epoch = pending.next_epoch;
        Ok(IndexWorkBatch {
            clear: pending.clear_epoch.take().is_some(),
            full_rebuild: pending.full_rebuild_epoch.take().is_some(),
            paths: std::mem::take(&mut pending.paths).into_keys().collect(),
            shutdown: pending.shutdown_epoch.take().is_some(),
            epoch,
        })
    }

    #[cfg(test)]
    pub(crate) fn discard_until_shutdown(self) {
        while let Ok(batch) = self.recv_debounced() {
            if batch.shutdown {
                break;
            }
        }
    }
}

pub(crate) fn build_index_status(runtime: &InteractiveIndexRuntime) -> DaemonIndexStatusResponse {
    let mut manifest = runtime.manifest.clone();
    if let Some(regex_runtime) = runtime.regex_runtime.as_ref() {
        apply_regex_manifest_status(&mut manifest, regex_runtime);
    }
    let dirty_file_count = runtime.manifest.dirty_paths.len();
    let queued_file_count = runtime.manifest.queued_paths.len();
    let ready = runtime.repo_is_current()
        && runtime.regex_is_current()
        && manifest.status == DaemonIndexState::Ready
        && manifest.dirty_paths.is_empty()
        && manifest.queued_paths.is_empty();
    DaemonIndexStatusResponse {
        manifest,
        ready,
        fallback_mode: !ready,
        loaded_generation: runtime
            .regex_runtime
            .as_ref()
            .and_then(|runtime| runtime.is_loaded().then_some(runtime.manifest.generation)),
        dirty_file_count,
        queued_file_count,
    }
}

pub(crate) fn enqueue_index_command(
    state: &Arc<Mutex<DaemonState>>,
    command: IndexCommand,
) -> Result<()> {
    let tx = state.lock().map_err(lock_err)?.index_tx.clone();
    tx.send(command).context("failed to queue index work")
}

pub(crate) fn enqueue_full_index_rebuild(state: &Arc<Mutex<DaemonState>>) -> Result<()> {
    let mut guard = state.lock().map_err(lock_err)?;
    guard
        .interactive_index
        .manifest
        .status
        .transition_to(DaemonIndexState::Queued)?;
    guard.interactive_index.manifest.total_files = 0;
    guard.interactive_index.manifest.indexed_files = 0;
    guard.interactive_index.manifest.regex_status = Some("queued".to_string());
    guard.interactive_index.manifest.regex_total_files = 0;
    guard.interactive_index.manifest.regex_indexed_files = 0;
    guard.interactive_index.manifest.last_error = None;
    guard.interactive_index.manifest.regex_stale_reason = None;
    guard.interactive_index.manifest.queued_paths.clear();
    save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)?;
    guard.index_tx.send(IndexCommand::RebuildFull)
}

pub(crate) fn enqueue_incremental_index_paths(
    state: &Arc<Mutex<DaemonState>>,
    paths: &[String],
) -> Result<IndexQueueOutcome> {
    let mut normalized = BTreeSet::new();
    let mut input_requires_full = false;
    for path in paths {
        let path = path.replace('\\', "/");
        if path.trim().is_empty() {
            continue;
        }
        if path.len() > MAX_INDEX_PATH_BYTES {
            input_requires_full = true;
            break;
        }
        normalized.insert(path);
        if normalized.len() > MAX_PENDING_INDEX_PATHS {
            input_requires_full = true;
            break;
        }
    }
    let normalized = normalized.into_iter().collect::<Vec<_>>();
    if normalized.is_empty() && !input_requires_full {
        return Ok(IndexQueueOutcome {
            full: false,
            queued_paths: Vec::new(),
        });
    }
    let mut guard = state.lock().map_err(lock_err)?;
    let additional_paths = normalized
        .iter()
        .filter(|path| {
            guard
                .interactive_index
                .manifest
                .queued_paths
                .binary_search(path)
                .is_err()
        })
        .count();
    let promote_to_full = input_requires_full
        || guard
            .interactive_index
            .manifest
            .queued_paths
            .len()
            .saturating_add(additional_paths)
            > MAX_PENDING_INDEX_PATHS;
    if promote_to_full {
        guard
            .interactive_index
            .manifest
            .status
            .transition_to(DaemonIndexState::Queued)?;
        guard.interactive_index.manifest.total_files = 0;
        guard.interactive_index.manifest.indexed_files = 0;
        guard.interactive_index.manifest.regex_status = Some("queued".to_string());
        guard.interactive_index.manifest.regex_total_files = 0;
        guard.interactive_index.manifest.regex_indexed_files = 0;
        guard.interactive_index.manifest.last_error = None;
        guard.interactive_index.manifest.regex_stale_reason = None;
        guard.interactive_index.manifest.dirty_paths.clear();
        guard.interactive_index.manifest.queued_paths.clear();
    } else {
        for path in &normalized {
            insert_sorted_unique(
                &mut guard.interactive_index.manifest.dirty_paths,
                path.clone(),
            );
            insert_sorted_unique(
                &mut guard.interactive_index.manifest.queued_paths,
                path.clone(),
            );
        }
        if guard.interactive_index.manifest.status == DaemonIndexState::Missing {
            guard
                .interactive_index
                .manifest
                .status
                .transition_to(DaemonIndexState::Queued)?;
        }
    }
    save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)?;
    if promote_to_full {
        guard.index_tx.send(IndexCommand::RebuildFull)?;
        Ok(IndexQueueOutcome {
            full: true,
            queued_paths: Vec::new(),
        })
    } else {
        guard
            .index_tx
            .send(IndexCommand::ReindexPaths(normalized.clone()))?;
        Ok(IndexQueueOutcome {
            full: false,
            queued_paths: normalized,
        })
    }
}

pub(crate) fn run_index_worker(
    state: Arc<Mutex<DaemonState>>,
    index_rx: IndexWorkReceiver,
) -> Result<()> {
    let shutdown = state.lock().map_err(lock_err)?.shutdown.clone();
    loop {
        let batch = index_rx.recv_debounced()?;
        if batch.shutdown || shutdown.is_requested() {
            if batch.clear {
                perform_index_clear(&state).context("index clear failed during daemon shutdown")?;
            }
            return Ok(());
        }
        if process_index_batch_with_recovery(&state, &batch, Some(&shutdown))?
            == IndexBatchStatus::Requeued
        {
            std::thread::sleep(Duration::from_millis(INDEX_BATCH_DEBOUNCE_MS));
        }
    }
}

fn process_index_batch_with_recovery(
    state: &Arc<Mutex<DaemonState>>,
    batch: &IndexWorkBatch,
    shutdown: Option<&crate::runtime::ShutdownSignal>,
) -> Result<IndexBatchStatus> {
    if batch.clear {
        if let Err(error) = perform_index_clear(state) {
            daemon_log(&format!("index clear failed and was requeued: {error}"));
            requeue_index_batch(state, batch)?;
            return Ok(IndexBatchStatus::Requeued);
        }
    }
    if batch.full_rebuild {
        if let Err(error) = perform_full_index_rebuild(state, shutdown, Some(batch.epoch)) {
            daemon_log(&format!(
                "full index rebuild failed and was requeued: {error}"
            ));
            reconcile_failed_index_publication(state, &error)?;
            if shutdown.is_some_and(crate::runtime::ShutdownSignal::is_requested) {
                return Ok(IndexBatchStatus::Complete);
            }
            enqueue_index_command(state, IndexCommand::RebuildFull)?;
            return Ok(IndexBatchStatus::Requeued);
        }
        return Ok(IndexBatchStatus::Complete);
    }
    if !batch.paths.is_empty() {
        if let Err(error) =
            perform_incremental_index_update(state, &batch.paths, shutdown, Some(batch.epoch))
        {
            daemon_log(&format!(
                "incremental index update failed; forcing a reconciled full rebuild: {error}"
            ));
            reconcile_failed_index_publication(state, &error)?;
            if shutdown.is_some_and(crate::runtime::ShutdownSignal::is_requested) {
                return Ok(IndexBatchStatus::Complete);
            }
            enqueue_index_command(state, IndexCommand::RebuildFull)?;
            return Ok(IndexBatchStatus::Requeued);
        }
    }
    Ok(IndexBatchStatus::Complete)
}

fn requeue_index_batch(state: &Arc<Mutex<DaemonState>>, batch: &IndexWorkBatch) -> Result<()> {
    if batch.clear {
        enqueue_index_command(state, IndexCommand::Clear)?;
    }
    if batch.full_rebuild {
        enqueue_index_command(state, IndexCommand::RebuildFull)?;
    } else if !batch.paths.is_empty() {
        enqueue_index_command(state, IndexCommand::ReindexPaths(batch.paths.clone()))?;
    }
    Ok(())
}

fn reconcile_failed_index_publication(
    state: &Arc<Mutex<DaemonState>>,
    error: &anyhow::Error,
) -> Result<()> {
    let root = state.lock().map_err(lock_err)?.root.clone();
    let repo_runtime = mapy_core::load_repo_index_runtime(&root)
        .ok()
        .filter(mapy_core::RepoIndexRuntime::is_loaded);
    let regex_runtime = packet28_search_core::load_runtime(&root)
        .ok()
        .filter(packet28_search_core::RegexIndexRuntime::is_loaded);
    let mut guard = state.lock().map_err(lock_err)?;
    if let Some(repo_runtime) = repo_runtime {
        guard.interactive_index.repo_runtime = Some(repo_runtime);
    }
    if let Some(regex_runtime) = regex_runtime {
        guard.interactive_index.regex_runtime = Some(regex_runtime);
    }
    guard
        .interactive_index
        .manifest
        .status
        .transition_to(DaemonIndexState::Queued)?;
    let recovery_reason = format!("index publication failed; queued full retry: {error}");
    guard.interactive_index.manifest.last_error = Some(recovery_reason.clone());
    guard.interactive_index.manifest.regex_stale_reason = Some(recovery_reason);
    save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)
}

fn perform_full_index_rebuild(
    state: &Arc<Mutex<DaemonState>>,
    shutdown: Option<&crate::runtime::ShutdownSignal>,
    batch_epoch: Option<u64>,
) -> Result<()> {
    perform_full_index_rebuild_after_start(state, shutdown, batch_epoch, || Ok(()))
}

fn perform_full_index_rebuild_after_start(
    state: &Arc<Mutex<DaemonState>>,
    shutdown: Option<&crate::runtime::ShutdownSignal>,
    batch_epoch: Option<u64>,
    after_start: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if shutdown.is_some_and(crate::runtime::ShutdownSignal::is_requested) {
        return Ok(());
    }
    let root = {
        let mut guard = state.lock().map_err(lock_err)?;
        guard
            .interactive_index
            .manifest
            .status
            .transition_to(DaemonIndexState::Building)?;
        guard.interactive_index.manifest.total_files = 0;
        guard.interactive_index.manifest.indexed_files = 0;
        guard.interactive_index.manifest.regex_status = Some("queued".to_string());
        guard.interactive_index.manifest.regex_total_files = 0;
        guard.interactive_index.manifest.regex_indexed_files = 0;
        guard.interactive_index.manifest.last_build_started_at_unix = Some(now_unix());
        guard.interactive_index.manifest.last_error = None;
        guard.interactive_index.manifest.regex_stale_reason = None;
        guard.interactive_index.manifest.queued_paths.clear();
        save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)?;
        guard.root.clone()
    };
    after_start()?;
    let mut last_repo_progress = None::<(usize, std::time::Instant)>;
    let repo_runtime =
        mapy_core::rebuild_repo_index_runtime_with_progress(&root, true, |indexed, total| {
            if should_persist_progress(indexed, total, &mut last_repo_progress) {
                let _ = update_repo_build_progress(state, indexed, total);
            }
        })
        .map_err(|err| anyhow!("failed to build repo index: {err}"))?;
    if shutdown.is_some_and(crate::runtime::ShutdownSignal::is_requested) {
        return requeue_interrupted_index_build(state);
    }
    update_repo_build_progress(
        state,
        repo_runtime.manifest.total_files,
        repo_runtime.manifest.total_files,
    )?;
    mark_regex_build_started(state)?;
    let mut last_regex_progress = None::<(usize, std::time::Instant)>;
    let regex_runtime =
        packet28_search_core::rebuild_full_index_with_progress(&root, true, |indexed, total| {
            if should_persist_progress(indexed, total, &mut last_regex_progress) {
                let _ = update_regex_build_progress(state, "building", indexed, total);
            }
        })
        .map_err(|err| anyhow!("failed to build regex search index: {err}"))?;
    if shutdown.is_some_and(crate::runtime::ShutdownSignal::is_requested) {
        return requeue_interrupted_index_build(state);
    }
    let mut guard = state.lock().map_err(lock_err)?;
    let follow_up = batch_epoch
        .map(|epoch| guard.index_tx.follow_up_after(epoch))
        .transpose()?
        .unwrap_or_default();
    guard.interactive_index.repo_runtime = Some(repo_runtime.clone());
    guard.interactive_index.regex_runtime = Some(regex_runtime.clone());
    guard.interactive_index.manifest.generation = guard
        .interactive_index
        .manifest
        .generation
        .saturating_add(1);
    guard
        .interactive_index
        .manifest
        .status
        .transition_to(if follow_up.full_rebuild {
            DaemonIndexState::Queued
        } else {
            DaemonIndexState::Ready
        })?;
    guard.interactive_index.manifest.dirty_paths = follow_up.paths.iter().cloned().collect();
    guard.interactive_index.manifest.queued_paths = follow_up.paths.into_iter().collect();
    guard.interactive_index.manifest.total_files = repo_runtime.manifest.total_files;
    guard.interactive_index.manifest.indexed_files = repo_runtime.manifest.total_files;
    apply_regex_manifest_status(&mut guard.interactive_index.manifest, &regex_runtime);
    if follow_up.full_rebuild {
        guard.interactive_index.manifest.regex_status = Some("queued".to_string());
    }
    guard
        .interactive_index
        .manifest
        .last_build_completed_at_unix = Some(now_unix());
    guard.interactive_index.manifest.last_error = None;
    save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)?;
    Ok(())
}

fn perform_incremental_index_update(
    state: &Arc<Mutex<DaemonState>>,
    paths: &[String],
    shutdown: Option<&crate::runtime::ShutdownSignal>,
    batch_epoch: Option<u64>,
) -> Result<()> {
    perform_incremental_index_update_after_start(state, paths, shutdown, batch_epoch, || Ok(()))
}

fn perform_incremental_index_update_after_start(
    state: &Arc<Mutex<DaemonState>>,
    paths: &[String],
    shutdown: Option<&crate::runtime::ShutdownSignal>,
    batch_epoch: Option<u64>,
    after_start: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if paths.is_empty() || shutdown.is_some_and(crate::runtime::ShutdownSignal::is_requested) {
        return Ok(());
    }
    let (root, repo_runtime_opt, regex_runtime_opt) = {
        let mut guard = state.lock().map_err(lock_err)?;
        if !guard.interactive_index.repo_is_current() || !guard.interactive_index.regex_is_current()
        {
            drop(guard);
            return perform_full_index_rebuild(state, shutdown, batch_epoch);
        }
        guard
            .interactive_index
            .manifest
            .status
            .transition_to(DaemonIndexState::Building)?;
        guard.interactive_index.manifest.last_build_started_at_unix = Some(now_unix());
        guard.interactive_index.manifest.regex_stale_reason = None;
        for path in paths {
            insert_sorted_unique(
                &mut guard.interactive_index.manifest.dirty_paths,
                path.clone(),
            );
        }
        save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)?;
        (
            guard.root.clone(),
            guard.interactive_index.repo_runtime.clone(),
            guard.interactive_index.regex_runtime.clone(),
        )
    };
    after_start()?;
    let (Some(repo_runtime), Some(regex_runtime)) = (repo_runtime_opt, regex_runtime_opt) else {
        return perform_full_index_rebuild(state, shutdown, batch_epoch);
    };
    let (repo_runtime, _summary) =
        mapy_core::update_repo_index_runtime(&root, &repo_runtime, paths, true)
            .map_err(|err| anyhow!("failed to update repo index: {err}"))?;
    if shutdown.is_some_and(crate::runtime::ShutdownSignal::is_requested) {
        return requeue_interrupted_index_build(state);
    }
    let regex_runtime =
        packet28_search_core::update_overlay_index(&root, Some(&regex_runtime), paths)
            .map_err(|err| anyhow!("failed to update regex search overlay: {err}"))?;
    if shutdown.is_some_and(crate::runtime::ShutdownSignal::is_requested) {
        return requeue_interrupted_index_build(state);
    }
    let mut guard = state.lock().map_err(lock_err)?;
    let follow_up = batch_epoch
        .map(|epoch| guard.index_tx.follow_up_after(epoch))
        .transpose()?
        .unwrap_or_default();
    guard.interactive_index.repo_runtime = Some(repo_runtime.clone());
    guard.interactive_index.regex_runtime = Some(regex_runtime.clone());
    guard.interactive_index.manifest.generation = guard
        .interactive_index
        .manifest
        .generation
        .saturating_add(1);
    guard
        .interactive_index
        .manifest
        .status
        .transition_to(if follow_up.full_rebuild {
            DaemonIndexState::Queued
        } else {
            DaemonIndexState::Ready
        })?;
    for path in paths {
        if follow_up.paths.contains(path) {
            continue;
        }
        guard
            .interactive_index
            .manifest
            .dirty_paths
            .retain(|candidate| candidate != path);
        guard
            .interactive_index
            .manifest
            .queued_paths
            .retain(|candidate| candidate != path);
    }
    guard.interactive_index.manifest.total_files = repo_runtime.manifest.total_files;
    guard.interactive_index.manifest.indexed_files = repo_runtime.manifest.total_files;
    apply_regex_manifest_status(&mut guard.interactive_index.manifest, &regex_runtime);
    if follow_up.full_rebuild {
        guard.interactive_index.manifest.regex_status = Some("queued".to_string());
    }
    guard
        .interactive_index
        .manifest
        .last_build_completed_at_unix = Some(now_unix());
    guard.interactive_index.manifest.last_error = None;
    save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)?;
    Ok(())
}

fn requeue_interrupted_index_build(state: &Arc<Mutex<DaemonState>>) -> Result<()> {
    let mut guard = state.lock().map_err(lock_err)?;
    guard
        .interactive_index
        .manifest
        .status
        .transition_to(DaemonIndexState::Queued)?;
    guard.interactive_index.manifest.regex_status = Some("queued".to_string());
    guard.interactive_index.manifest.regex_stale_reason =
        Some("daemon shutdown interrupted index publication".to_string());
    save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)
}

fn perform_index_clear(state: &Arc<Mutex<DaemonState>>) -> Result<()> {
    let mut guard = state.lock().map_err(lock_err)?;
    clear_index_files(&guard.root)?;
    packet28_search_core::clear_index(&guard.root)?;
    guard.interactive_index = InteractiveIndexRuntime {
        manifest: default_index_manifest(&guard.root),
        repo_runtime: None,
        regex_runtime: None,
    };
    save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)?;
    Ok(())
}

pub(crate) fn daemon_index_status(
    state: Arc<Mutex<DaemonState>>,
) -> Result<DaemonIndexStatusResponse> {
    let guard = state.lock().map_err(lock_err)?;
    Ok(build_index_status(&guard.interactive_index))
}

pub(crate) fn daemon_index_rebuild(
    state: Arc<Mutex<DaemonState>>,
    request: DaemonIndexRebuildRequest,
) -> Result<DaemonIndexRebuildResponse> {
    let outcome = if request.full || request.paths.is_empty() {
        enqueue_full_index_rebuild(&state)?;
        IndexQueueOutcome {
            full: true,
            queued_paths: Vec::new(),
        }
    } else {
        enqueue_incremental_index_paths(&state, &request.paths)?
    };
    let generation = state
        .lock()
        .map_err(lock_err)?
        .interactive_index
        .manifest
        .generation;
    Ok(DaemonIndexRebuildResponse {
        accepted: true,
        full: outcome.full,
        generation: Some(generation),
        queued_paths: outcome.queued_paths,
    })
}

pub(crate) fn daemon_index_clear(
    state: Arc<Mutex<DaemonState>>,
) -> Result<DaemonIndexClearResponse> {
    enqueue_index_command(&state, IndexCommand::Clear)?;
    Ok(DaemonIndexClearResponse { cleared: true })
}

pub(crate) fn daemon_packet28_search(
    state: Arc<Mutex<DaemonState>>,
    request: packet28_daemon_protocol::message::Packet28SearchRequest,
) -> Result<packet28_reducer_core::SearchResult> {
    let packet28_daemon_protocol::message::Packet28SearchRequest {
        request,
        force_indexed,
    } = request;
    let (root, runtime, daemon_fallback_reason) = {
        let guard = state.lock().map_err(lock_err)?;
        (
            guard.root.clone(),
            guard.interactive_index.regex_runtime.clone(),
            daemon_manifest_search_fallback_reason(&guard.interactive_index.manifest, &request),
        )
    };
    if let Some(reason) = daemon_fallback_reason {
        if force_indexed {
            return Err(DaemonIndexSearchNotReady { reason }.into());
        }
        return live_search_with_reason(&root, &request, reason);
    }
    let Some(runtime) = runtime else {
        let reason = "regex search index is not ready".to_string();
        if force_indexed {
            return Err(DaemonIndexSearchNotReady { reason }.into());
        }
        return live_search_with_reason(&root, &request, reason);
    };
    if let Some(reason) = packet28_search_core::guarded_fallback_reason(&root, &runtime, &request)?
    {
        if force_indexed {
            return Err(DaemonIndexSearchNotReady { reason }.into());
        }
        return live_search_with_reason(&root, &request, reason);
    }
    Ok(packet28_search_core::indexed_search(
        &root, &runtime, &request,
    )?)
}

pub(crate) fn daemon_packet28_search_guard(
    state: Arc<Mutex<DaemonState>>,
    request: packet28_daemon_protocol::message::Packet28SearchRequest,
) -> Result<packet28_daemon_protocol::message::Packet28SearchGuardResponse> {
    let packet28_daemon_protocol::message::Packet28SearchRequest {
        request,
        force_indexed,
    } = request;
    let (root, runtime, daemon_fallback_reason) = {
        let guard = state.lock().map_err(lock_err)?;
        (
            guard.root.clone(),
            guard.interactive_index.regex_runtime.clone(),
            daemon_manifest_search_fallback_reason(&guard.interactive_index.manifest, &request),
        )
    };
    let fallback_reason = match daemon_fallback_reason {
        Some(reason) => Some(reason),
        None => match runtime {
            Some(runtime) => {
                if force_indexed {
                    None
                } else {
                    packet28_search_core::guarded_fallback_reason(&root, &runtime, &request)?
                }
            }
            None if force_indexed => None,
            None => Some("regex search index is not ready".to_string()),
        },
    };
    Ok(packet28_daemon_protocol::message::Packet28SearchGuardResponse { fallback_reason })
}

fn live_search_with_reason(
    root: &Path,
    request: &packet28_reducer_core::SearchRequest,
    reason: String,
) -> Result<packet28_reducer_core::SearchResult> {
    let mut fallback = packet28_reducer_core::search(root, request)?;
    if let Some(engine) = fallback.engine.as_mut() {
        engine.fallback_reason = Some(reason);
    }
    Ok(fallback)
}

fn daemon_manifest_search_fallback_reason(
    manifest: &DaemonIndexManifest,
    request: &packet28_reducer_core::SearchRequest,
) -> Option<String> {
    if manifest.status != DaemonIndexState::Ready {
        return Some(format!(
            "daemon index manifest is {:?}; indexed search is not current",
            manifest.status
        ));
    }
    let pending_paths = manifest
        .dirty_paths
        .iter()
        .chain(&manifest.queued_paths)
        .collect::<BTreeSet<_>>();
    if pending_paths.is_empty() {
        return None;
    }
    let relevant = request.requested_paths.is_empty()
        || pending_paths.iter().any(|dirty| {
            request.requested_paths.iter().any(|requested| {
                if Path::new(requested).is_absolute() {
                    true
                } else {
                    let requested =
                        packet28_reducer_core::normalize_capture_path(Path::new(""), requested);
                    paths_overlap(&requested, dirty)
                }
            })
        });
    relevant.then(|| {
        format!(
            "daemon index has {} queued or dirty path(s) relevant to this search",
            pending_paths.len()
        )
    })
}

fn paths_overlap(left: &str, right: &str) -> bool {
    let left = left.trim_end_matches('/');
    let right = right.trim_end_matches('/');
    if left.is_empty() || right.is_empty() || left == "." || right == "." {
        return true;
    }
    left == right
        || right.starts_with(&format!("{left}/"))
        || left.starts_with(&format!("{right}/"))
}

fn apply_regex_manifest_status(
    manifest: &mut DaemonIndexManifest,
    runtime: &packet28_search_core::RegexIndexRuntime,
) {
    manifest.regex_generation = Some(runtime.manifest.generation);
    manifest.regex_status = Some(runtime.manifest.status.clone());
    manifest.regex_total_files = runtime.manifest.total_files;
    manifest.regex_base_commit = runtime.manifest.base_commit.clone();
    manifest.regex_weight_table_version = Some(runtime.manifest.weight_table_version);
    manifest.regex_stale_reason = runtime.manifest.stale_reason.clone();
    manifest.regex_indexed_files = runtime.manifest.indexed_files;
}

fn should_persist_progress(
    indexed: usize,
    total: usize,
    checkpoint: &mut Option<(usize, std::time::Instant)>,
) -> bool {
    let now = std::time::Instant::now();
    let should_emit = match checkpoint {
        None => true,
        Some((last_indexed, last_at)) => {
            indexed == total
                || indexed == 0
                || indexed >= last_indexed.saturating_add(32)
                || now.duration_since(*last_at) >= std::time::Duration::from_millis(250)
        }
    };
    if should_emit {
        *checkpoint = Some((indexed, now));
    }
    should_emit
}

fn update_repo_build_progress(
    state: &Arc<Mutex<DaemonState>>,
    indexed_files: usize,
    total_files: usize,
) -> Result<()> {
    let mut guard = state.lock().map_err(lock_err)?;
    guard
        .interactive_index
        .manifest
        .status
        .transition_to(DaemonIndexState::Building)?;
    guard.interactive_index.manifest.total_files = total_files;
    guard.interactive_index.manifest.indexed_files = indexed_files.min(total_files);
    save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)
}

fn mark_regex_build_started(state: &Arc<Mutex<DaemonState>>) -> Result<()> {
    let mut guard = state.lock().map_err(lock_err)?;
    guard.interactive_index.manifest.regex_status = Some("building".to_string());
    guard.interactive_index.manifest.regex_total_files = 0;
    guard.interactive_index.manifest.regex_indexed_files = 0;
    save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)
}

fn update_regex_build_progress(
    state: &Arc<Mutex<DaemonState>>,
    status: &str,
    indexed_files: usize,
    total_files: usize,
) -> Result<()> {
    let mut guard = state.lock().map_err(lock_err)?;
    guard.interactive_index.manifest.regex_status = Some(status.to_string());
    guard.interactive_index.manifest.regex_total_files = total_files;
    guard.interactive_index.manifest.regex_indexed_files = indexed_files.min(total_files);
    save_index_manifest_file(&guard.root, &guard.interactive_index.manifest)
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
