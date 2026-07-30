use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use packet28_daemon_core::storage::{append_next_task_event, save_task_watch_registry_checkpoint};
use packet28_daemon_core::task_store_lease::TaskStoreLease;
use packet28_daemon_protocol::message::{DaemonEvent, DaemonEventFrame};
use packet28_daemon_protocol::paths::{task_registry_path, watch_registry_path};
use packet28_daemon_protocol::task::{TaskRecord, TaskRegistry, WatchRegistration, WatchRegistry};

const COMMAND_CAPACITY: usize = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PersistenceMetrics {
    pub(crate) deltas_submitted: u64,
    pub(crate) deltas_coalesced: u64,
    pub(crate) wakeups_coalesced: u64,
    pub(crate) wal_batches_appended: u64,
    pub(crate) wal_bytes_appended: u64,
    pub(crate) checkpoints_written: u64,
    pub(crate) checkpoint_bytes_written: u64,
    pub(crate) events_submitted: u64,
    pub(crate) events_started: u64,
    pub(crate) events_appended: u64,
    pub(crate) event_bytes_appended: u64,
    pub(crate) event_state_lock_sections: u64,
    pub(crate) event_state_lock_nanos: u64,
    pub(crate) max_event_state_lock_nanos: u64,
    pub(crate) barriers_completed: u64,
    pub(crate) failures: u64,
    pub(crate) max_pending_batches: u64,
    pub(crate) max_pending_task_keys: u64,
    pub(crate) max_pending_watch_keys: u64,
}

/// One atomic registry mutation staged while the daemon state lock is held.
///
/// The batch owns only records whose keys changed. Later changes to the same
/// key replace earlier pending values; distinct keys accumulate until the
/// persistence owner appends one contiguous revision range.
#[derive(Debug, Default)]
pub(crate) struct RegistryDelta {
    task_upserts: BTreeMap<String, TaskRecord>,
    task_removals: BTreeSet<String>,
    watch_upserts: BTreeMap<String, WatchRegistration>,
    watch_upsert_order: Vec<String>,
    watch_removals: BTreeSet<String>,
}

impl RegistryDelta {
    pub(crate) fn upsert_task(mut self, task: TaskRecord) -> Self {
        self.task_removals.remove(&task.task_id);
        self.task_upserts.insert(task.task_id.clone(), task);
        self
    }

    pub(crate) fn remove_task(mut self, task_id: impl Into<String>) -> Self {
        let task_id = task_id.into();
        self.task_upserts.remove(&task_id);
        self.task_removals.insert(task_id);
        self
    }

    pub(crate) fn upsert_watch(mut self, watch: WatchRegistration) -> Self {
        let watch_id = watch.watch_id.clone();
        if !self.watch_upserts.contains_key(&watch_id) {
            self.watch_upsert_order.push(watch_id.clone());
        }
        // An overlapping removal is intentional: applying removals before
        // upserts preserves remove-then-reinsert ordering.
        self.watch_upserts.insert(watch_id, watch);
        self
    }

    pub(crate) fn remove_watch(mut self, watch_id: impl Into<String>) -> Self {
        let watch_id = watch_id.into();
        self.watch_upserts.remove(&watch_id);
        self.watch_upsert_order
            .retain(|candidate| candidate != &watch_id);
        self.watch_removals.insert(watch_id);
        self
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.task_upserts.is_empty()
            && self.task_removals.is_empty()
            && self.watch_upserts.is_empty()
            && self.watch_removals.is_empty()
    }

    fn task_key_count(&self) -> usize {
        self.task_upserts
            .len()
            .saturating_add(self.task_removals.len())
    }

    fn watch_key_count(&self) -> usize {
        self.watch_upserts
            .len()
            .saturating_add(self.watch_removals.len())
    }

    fn merge_later(&mut self, later: Self) {
        for task_id in later.task_removals {
            self.task_upserts.remove(&task_id);
            self.task_removals.insert(task_id);
        }
        for (task_id, task) in later.task_upserts {
            self.task_removals.remove(&task_id);
            self.task_upserts.insert(task_id, task);
        }
        for watch_id in &later.watch_removals {
            self.watch_upserts.remove(watch_id);
            self.watch_upsert_order
                .retain(|candidate| candidate != watch_id);
            self.watch_removals.insert(watch_id.clone());
        }
        for watch_id in later.watch_upsert_order {
            let watch = later
                .watch_upserts
                .get(&watch_id)
                .expect("watch upsert order references a present record")
                .clone();
            let reinsertion =
                self.watch_removals.contains(&watch_id) || later.watch_removals.contains(&watch_id);
            if reinsertion {
                self.watch_upserts.remove(&watch_id);
                self.watch_upsert_order
                    .retain(|candidate| candidate != &watch_id);
                self.watch_removals.insert(watch_id.clone());
            }
            if !self.watch_upserts.contains_key(&watch_id) {
                self.watch_upsert_order.push(watch_id.clone());
            }
            self.watch_upserts.insert(watch_id, watch);
        }
    }

    fn apply_to(&self, tasks: &mut TaskRegistry, watches: &mut WatchRegistry) {
        for task_id in &self.task_removals {
            tasks.tasks.remove(task_id);
        }
        for (task_id, task) in &self.task_upserts {
            tasks.tasks.insert(task_id.clone(), task.clone());
        }
        for watch_id in &self.watch_removals {
            watches.watches.retain(|watch| watch.watch_id != *watch_id);
        }
        for watch_id in &self.watch_upsert_order {
            let replacement = self
                .watch_upserts
                .get(watch_id)
                .expect("watch upsert order references a present record");
            if let Some(existing) = watches
                .watches
                .iter_mut()
                .find(|watch| watch.watch_id == *watch_id)
            {
                existing.clone_from(replacement);
            } else {
                watches.watches.push(replacement.clone());
            }
        }
    }
}

struct PendingDelta {
    first_revision: u64,
    last_revision: u64,
    delta: RegistryDelta,
}

impl PendingDelta {
    fn merge_later(&mut self, revision: u64, delta: RegistryDelta) {
        debug_assert_eq!(revision, self.last_revision.saturating_add(1));
        self.last_revision = revision;
        self.delta.merge_later(delta);
    }
}

#[derive(Default)]
struct PendingState {
    next_revision: u64,
    durable_revision: u64,
    durable_task_ids: BTreeSet<String>,
    delta: Option<PendingDelta>,
    last_error: Option<String>,
    unsurfaced_error: Option<String>,
}

struct RegistryImage {
    tasks: TaskRegistry,
    watches: WatchRegistry,
}

trait PersistenceBackend: Send + Sync {
    fn save_checkpoint(
        &self,
        root: &Path,
        tasks: &TaskRegistry,
        watches: &WatchRegistry,
    ) -> Result<()>;

    fn append_event(
        &self,
        root: &Path,
        task_id: &str,
        event: &DaemonEvent,
    ) -> Result<DaemonEventFrame>;
}

struct FilesystemBackend;

impl PersistenceBackend for FilesystemBackend {
    fn save_checkpoint(
        &self,
        root: &Path,
        tasks: &TaskRegistry,
        watches: &WatchRegistry,
    ) -> Result<()> {
        Ok(save_task_watch_registry_checkpoint(root, tasks, watches)?)
    }

    fn append_event(
        &self,
        root: &Path,
        task_id: &str,
        event: &DaemonEvent,
    ) -> Result<DaemonEventFrame> {
        Ok(append_next_task_event(root, task_id, event)?)
    }
}

struct PersistenceState {
    root: PathBuf,
    debounce: Duration,
    pending: Mutex<PendingState>,
    metrics: Mutex<PersistenceMetrics>,
    admission_lane: Mutex<()>,
    event_lane: Mutex<()>,
    accepting: std::sync::atomic::AtomicBool,
    backend: Arc<dyn PersistenceBackend>,
    #[cfg(test)]
    checkpoint_gate: Mutex<Option<CheckpointTestGate>>,
}

#[cfg(test)]
struct CheckpointTestGate {
    remaining: usize,
    reached: SyncSender<()>,
    release: Receiver<()>,
}

type UnitReply = SyncSender<std::result::Result<(), String>>;
type EventReply = SyncSender<std::result::Result<DaemonEventFrame, String>>;
type ShutdownReply = SyncSender<std::result::Result<PersistenceMetrics, String>>;

enum PersistenceCommand {
    Wake,
    Barrier {
        target_revision: u64,
        reply: UnitReply,
    },
    Checkpoint {
        target_revision: u64,
        reply: UnitReply,
    },
    AppendEvent {
        required_revision: Option<u64>,
        task_id: String,
        event: DaemonEvent,
        reply: EventReply,
    },
    Shutdown {
        target_revision: u64,
        reply: ShutdownReply,
    },
}

#[derive(Clone)]
pub(crate) struct PersistenceHandle {
    state: Arc<PersistenceState>,
    sender: SyncSender<PersistenceCommand>,
}

pub(crate) struct PersistenceOwner {
    handle: Option<PersistenceHandle>,
    worker: Option<JoinHandle<()>>,
}

impl PersistenceOwner {
    pub(crate) fn start(
        root: PathBuf,
        task_store_lease: TaskStoreLease,
        debounce: Duration,
        durable_tasks: &TaskRegistry,
        durable_watches: &WatchRegistry,
    ) -> Result<(Self, PersistenceHandle)> {
        Self::start_with_backend(
            root,
            Some(task_store_lease),
            debounce,
            durable_tasks.clone(),
            durable_watches.clone(),
            Arc::new(FilesystemBackend),
        )
    }

    #[cfg(test)]
    pub(crate) fn start_unleased(
        root: PathBuf,
        debounce: Duration,
    ) -> Result<(Self, PersistenceHandle)> {
        Self::start_with_backend(
            root,
            None,
            debounce,
            TaskRegistry::default(),
            WatchRegistry::default(),
            Arc::new(FilesystemBackend),
        )
    }

    fn start_with_backend(
        root: PathBuf,
        task_store_lease: Option<TaskStoreLease>,
        debounce: Duration,
        durable_tasks: TaskRegistry,
        durable_watches: WatchRegistry,
        backend: Arc<dyn PersistenceBackend>,
    ) -> Result<(Self, PersistenceHandle)> {
        let (sender, receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let durable_task_ids = durable_tasks.tasks.keys().cloned().collect();
        let state = Arc::new(PersistenceState {
            root,
            debounce,
            pending: Mutex::new(PendingState {
                durable_task_ids,
                ..PendingState::default()
            }),
            metrics: Mutex::new(PersistenceMetrics::default()),
            admission_lane: Mutex::new(()),
            event_lane: Mutex::new(()),
            accepting: std::sync::atomic::AtomicBool::new(true),
            backend,
            #[cfg(test)]
            checkpoint_gate: Mutex::new(None),
        });
        let worker_state = state.clone();
        let worker = thread::Builder::new()
            .name("packet28d-persistence".to_string())
            .spawn(move || {
                run_worker(
                    worker_state,
                    receiver,
                    task_store_lease,
                    RegistryImage {
                        tasks: durable_tasks,
                        watches: durable_watches,
                    },
                );
            })
            .context("failed to start daemon persistence owner")?;
        let handle = PersistenceHandle { state, sender };
        Ok((
            Self {
                handle: Some(handle.clone()),
                worker: Some(worker),
            },
            handle,
        ))
    }

    pub(crate) fn shutdown(mut self, timeout: Duration) -> Result<PersistenceMetrics> {
        let started = Instant::now();
        let handle = self
            .handle
            .take()
            .ok_or_else(|| anyhow!("daemon persistence owner is already shut down"))?;
        handle
            .state
            .accepting
            .store(false, std::sync::atomic::Ordering::Release);
        let target_revision = handle.latest_revision();
        let (reply, completion) = mpsc::sync_channel(1);
        let command = PersistenceCommand::Shutdown {
            target_revision,
            reply,
        };
        send_until(&handle.sender, command, timeout)?;
        let remaining = timeout.saturating_sub(started.elapsed());
        let result = completion
            .recv_timeout(remaining)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => {
                    anyhow!("timed out waiting for daemon persistence shutdown")
                }
                RecvTimeoutError::Disconnected => {
                    anyhow!("daemon persistence worker stopped before shutdown acknowledgement")
                }
            })?;
        let worker = self
            .worker
            .take()
            .ok_or_else(|| anyhow!("daemon persistence worker handle is unavailable"))?;
        worker
            .join()
            .map_err(|_| anyhow!("daemon persistence worker panicked during shutdown"))?;
        result.map_err(anyhow::Error::msg)
    }
}

impl Drop for PersistenceOwner {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        handle
            .state
            .accepting
            .store(false, std::sync::atomic::Ordering::Release);
        let (reply, _completion) = mpsc::sync_channel(1);
        let _ = handle.sender.try_send(PersistenceCommand::Shutdown {
            target_revision: handle.latest_revision(),
            reply,
        });
        // Dropping a JoinHandle detaches instead of blocking an error path.
        // The worker retains the task-store lease until it actually exits.
        let _ = self.worker.take();
    }
}

impl PersistenceHandle {
    pub(crate) fn stage(&self, delta: RegistryDelta) -> Result<u64> {
        self.ensure_accepting()?;
        if delta.is_empty() {
            return Ok(self.latest_revision());
        }
        let revision = {
            let mut pending = lock_unpoisoned(&self.state.pending);
            self.ensure_accepting()?;
            pending.next_revision = pending
                .next_revision
                .checked_add(1)
                .ok_or_else(|| anyhow!("daemon persistence revision exhausted"))?;
            let revision = pending.next_revision;
            let coalesced = pending.delta.is_some();
            match pending.delta.as_mut() {
                Some(pending_delta) => pending_delta.merge_later(revision, delta),
                None => {
                    pending.delta = Some(PendingDelta {
                        first_revision: revision,
                        last_revision: revision,
                        delta,
                    });
                }
            }
            let staged = pending
                .delta
                .as_ref()
                .expect("pending delta exists after staging");
            let pending_task_keys =
                u64::try_from(staged.delta.task_key_count()).unwrap_or(u64::MAX);
            let pending_watch_keys =
                u64::try_from(staged.delta.watch_key_count()).unwrap_or(u64::MAX);
            let mut metrics = lock_unpoisoned(&self.state.metrics);
            metrics.deltas_submitted = metrics.deltas_submitted.saturating_add(1);
            if coalesced {
                metrics.deltas_coalesced = metrics.deltas_coalesced.saturating_add(1);
            }
            metrics.max_pending_batches = 1;
            metrics.max_pending_task_keys = metrics.max_pending_task_keys.max(pending_task_keys);
            metrics.max_pending_watch_keys = metrics.max_pending_watch_keys.max(pending_watch_keys);
            revision
        };
        match self.sender.try_send(PersistenceCommand::Wake) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                let mut metrics = lock_unpoisoned(&self.state.metrics);
                metrics.wakeups_coalesced = metrics.wakeups_coalesced.saturating_add(1);
            }
            Err(TrySendError::Disconnected(_)) => {
                return Err(anyhow!("daemon persistence worker is stopped"));
            }
        }
        Ok(revision)
    }

    pub(crate) fn stage_and_flush(&self, delta: RegistryDelta) -> Result<()> {
        let revision = self.stage(delta)?;
        self.barrier(revision)
    }

    pub(crate) fn flush(&self) -> Result<PersistenceMetrics> {
        let target_revision = self.latest_revision();
        self.barrier(target_revision)?;
        Ok(self.metrics())
    }

    pub(crate) fn flush_through(&self, revision: u64) -> Result<()> {
        self.barrier(revision)
    }

    pub(crate) fn checkpoint_current(&self) -> Result<()> {
        self.ensure_accepting()?;
        let target_revision = self.latest_revision();
        let (reply, completion) = mpsc::sync_channel(1);
        self.sender
            .send(PersistenceCommand::Checkpoint {
                target_revision,
                reply,
            })
            .map_err(|_| anyhow!("daemon persistence worker is stopped"))?;
        completion
            .recv()
            .map_err(|_| {
                anyhow!("daemon persistence worker stopped before checkpoint acknowledgement")
            })?
            .map_err(anyhow::Error::msg)
    }

    pub(crate) fn append_event(
        &self,
        task_id: &str,
        event: DaemonEvent,
        required_revision: Option<u64>,
    ) -> Result<DaemonEventFrame> {
        self.ensure_accepting()?;
        {
            let mut metrics = lock_unpoisoned(&self.state.metrics);
            metrics.events_submitted = metrics.events_submitted.saturating_add(1);
        }
        let (reply, completion) = mpsc::sync_channel(1);
        self.sender
            .send(PersistenceCommand::AppendEvent {
                required_revision,
                task_id: task_id.to_string(),
                event,
                reply,
            })
            .map_err(|_| anyhow!("daemon persistence worker is stopped"))?;
        completion
            .recv()
            .map_err(|_| anyhow!("daemon persistence worker stopped before event acknowledgement"))?
            .map_err(anyhow::Error::msg)
    }

    pub(crate) fn event_guard(&self) -> MutexGuard<'_, ()> {
        lock_unpoisoned(&self.state.event_lane)
    }

    pub(crate) fn task_is_durably_admitted(&self, task_id: &str) -> bool {
        lock_unpoisoned(&self.state.pending)
            .durable_task_ids
            .contains(task_id)
    }

    pub(crate) fn ensure_task_admitted(&self, task_id: &str, revision: u64) -> Result<()> {
        let _admission_guard = lock_unpoisoned(&self.state.admission_lane);
        if self.task_is_durably_admitted(task_id) {
            return Ok(());
        }
        self.barrier(revision)?;
        if !self.task_is_durably_admitted(task_id) {
            anyhow::bail!(
                "daemon persistence revision {revision} did not durably admit task '{task_id}'"
            );
        }
        Ok(())
    }

    pub(crate) fn metrics(&self) -> PersistenceMetrics {
        *lock_unpoisoned(&self.state.metrics)
    }

    #[cfg(test)]
    pub(crate) fn exhaust_revisions_for_test(&self) {
        lock_unpoisoned(&self.state.pending).next_revision = u64::MAX;
    }

    #[cfg(test)]
    pub(crate) fn gate_checkpoint_for_test(
        &self,
        ordinal: usize,
    ) -> (Receiver<()>, SyncSender<()>) {
        assert!(ordinal > 0, "checkpoint gate ordinal must be positive");
        let (reached, reached_rx) = mpsc::sync_channel(1);
        let (release_tx, release) = mpsc::sync_channel(1);
        let mut gate = lock_unpoisoned(&self.state.checkpoint_gate);
        assert!(gate.is_none(), "a checkpoint gate is already installed");
        *gate = Some(CheckpointTestGate {
            remaining: ordinal,
            reached,
            release,
        });
        (reached_rx, release_tx)
    }

    pub(crate) fn record_event_state_lock_hold(&self, duration: Duration) {
        let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        let mut metrics = lock_unpoisoned(&self.state.metrics);
        metrics.event_state_lock_sections = metrics.event_state_lock_sections.saturating_add(1);
        metrics.event_state_lock_nanos = metrics.event_state_lock_nanos.saturating_add(nanos);
        metrics.max_event_state_lock_nanos = metrics.max_event_state_lock_nanos.max(nanos);
    }

    fn barrier(&self, target_revision: u64) -> Result<()> {
        self.ensure_accepting()?;
        let (reply, completion) = mpsc::sync_channel(1);
        self.sender
            .send(PersistenceCommand::Barrier {
                target_revision,
                reply,
            })
            .map_err(|_| anyhow!("daemon persistence worker is stopped"))?;
        completion
            .recv()
            .map_err(|_| {
                anyhow!("daemon persistence worker stopped before checkpoint acknowledgement")
            })?
            .map_err(anyhow::Error::msg)
    }

    pub(crate) fn latest_revision(&self) -> u64 {
        lock_unpoisoned(&self.state.pending).next_revision
    }

    fn ensure_accepting(&self) -> Result<()> {
        if !self
            .state
            .accepting
            .load(std::sync::atomic::Ordering::Acquire)
        {
            anyhow::bail!("daemon persistence owner is shutting down");
        }
        Ok(())
    }
}

fn run_worker(
    state: Arc<PersistenceState>,
    receiver: Receiver<PersistenceCommand>,
    _task_store_lease: Option<TaskStoreLease>,
    mut image: RegistryImage,
) {
    let mut next = None;
    loop {
        let command = match next.take() {
            Some(command) => command,
            None => match receiver.recv() {
                Ok(command) => command,
                Err(_) => {
                    let _ = persist_pending(&state, &mut image, None, false);
                    return;
                }
            },
        };
        match command {
            PersistenceCommand::Wake => match command_after_debounce(&receiver, state.debounce) {
                DebounceOutcome::Command(command) => next = Some(command),
                DebounceOutcome::Elapsed => {
                    let _ = persist_pending(&state, &mut image, None, false);
                }
                DebounceOutcome::Disconnected => {
                    let _ = persist_pending(&state, &mut image, None, false);
                    return;
                }
            },
            PersistenceCommand::Barrier {
                target_revision,
                reply,
            } => {
                let result = persist_pending(&state, &mut image, Some(target_revision), true);
                if result.is_ok() {
                    let mut metrics = lock_unpoisoned(&state.metrics);
                    metrics.barriers_completed = metrics.barriers_completed.saturating_add(1);
                }
                let _ = reply.send(result.map_err(|error| error.to_string()));
            }
            PersistenceCommand::Checkpoint {
                target_revision,
                reply,
            } => {
                let result = persist_pending(&state, &mut image, Some(target_revision), true)
                    .and_then(|()| save_current_image(&state, &image));
                let _ = reply.send(result.map_err(|error| error.to_string()));
            }
            PersistenceCommand::AppendEvent {
                required_revision,
                task_id,
                event,
                reply,
            } => {
                let result = required_revision
                    .map_or_else(
                        || Ok(()),
                        |revision| persist_pending(&state, &mut image, Some(revision), true),
                    )
                    .and_then(|()| {
                        {
                            let mut metrics = lock_unpoisoned(&state.metrics);
                            metrics.events_started = metrics.events_started.saturating_add(1);
                        }
                        state
                            .backend
                            .append_event(&state.root, &task_id, &event)
                            .inspect(|frame| {
                                let encoded_bytes = serde_json::to_vec(frame)
                                    .map(|encoded| {
                                        u64::try_from(encoded.len())
                                            .unwrap_or(u64::MAX)
                                            .saturating_add(1)
                                    })
                                    .unwrap_or_default();
                                let mut metrics = lock_unpoisoned(&state.metrics);
                                metrics.events_appended = metrics.events_appended.saturating_add(1);
                                metrics.event_bytes_appended =
                                    metrics.event_bytes_appended.saturating_add(encoded_bytes);
                            })
                    });
                if result.is_err() {
                    let mut metrics = lock_unpoisoned(&state.metrics);
                    metrics.failures = metrics.failures.saturating_add(1);
                }
                let _ = reply.send(result.map_err(|error| error.to_string()));
            }
            PersistenceCommand::Shutdown {
                target_revision,
                reply,
            } => {
                let result = persist_pending(&state, &mut image, Some(target_revision), true)
                    .map(|()| *lock_unpoisoned(&state.metrics))
                    .map_err(|error| error.to_string());
                // This acknowledgement is the worker's final operation.
                let _ = reply.send(result);
                return;
            }
        }
    }
}

enum DebounceOutcome {
    Command(PersistenceCommand),
    Elapsed,
    Disconnected,
}

fn command_after_debounce(
    receiver: &Receiver<PersistenceCommand>,
    debounce: Duration,
) -> DebounceOutcome {
    let deadline = Instant::now() + debounce;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok(PersistenceCommand::Wake) => {}
            Ok(command) => return DebounceOutcome::Command(command),
            Err(RecvTimeoutError::Timeout) => return DebounceOutcome::Elapsed,
            Err(RecvTimeoutError::Disconnected) => return DebounceOutcome::Disconnected,
        }
    }
}

fn persist_pending(
    state: &PersistenceState,
    image: &mut RegistryImage,
    required_revision: Option<u64>,
    surface_prior_error: bool,
) -> Result<()> {
    loop {
        let pending_delta = {
            let mut pending = lock_unpoisoned(&state.pending);
            if required_revision.is_some_and(|required| pending.durable_revision >= required) {
                pending.last_error = None;
                return take_prior_error(&mut pending, surface_prior_error);
            }
            pending.delta.take()
        };
        let Some(pending_delta) = pending_delta else {
            let mut pending = lock_unpoisoned(&state.pending);
            return match required_revision {
                Some(required) if pending.durable_revision < required => Err(anyhow!(
                    "daemon persistence revision {} is unavailable; durable revision is {}",
                    required,
                    pending.durable_revision
                )),
                _ => take_prior_error(&mut pending, surface_prior_error),
            };
        };
        await_checkpoint_test_gate(state);
        pending_delta
            .delta
            .apply_to(&mut image.tasks, &mut image.watches);
        match state
            .backend
            .save_checkpoint(&state.root, &image.tasks, &image.watches)
        {
            Ok(()) => {
                {
                    let mut pending = lock_unpoisoned(&state.pending);
                    pending.durable_revision =
                        pending.durable_revision.max(pending_delta.last_revision);
                    for task_id in &pending_delta.delta.task_removals {
                        pending.durable_task_ids.remove(task_id);
                    }
                    for task_id in pending_delta.delta.task_upserts.keys() {
                        pending.durable_task_ids.insert(task_id.clone());
                    }
                    pending.last_error = None;
                }
                let mut metrics = lock_unpoisoned(&state.metrics);
                metrics.checkpoints_written = metrics.checkpoints_written.saturating_add(1);
                let checkpoint_bytes = [
                    task_registry_path(&state.root),
                    watch_registry_path(&state.root),
                ]
                .into_iter()
                .filter_map(|path| std::fs::metadata(path).ok())
                .map(|metadata| metadata.len())
                .sum::<u64>();
                metrics.checkpoint_bytes_written = metrics
                    .checkpoint_bytes_written
                    .saturating_add(checkpoint_bytes);
            }
            Err(error) => {
                let message = error.to_string();
                {
                    let mut pending = lock_unpoisoned(&state.pending);
                    if let Some(later) = pending.delta.take() {
                        let mut failed = pending_delta;
                        debug_assert_eq!(
                            failed.last_revision.saturating_add(1),
                            later.first_revision
                        );
                        failed.last_revision = later.last_revision;
                        failed.delta.merge_later(later.delta);
                        pending.delta = Some(failed);
                    } else {
                        pending.delta = Some(pending_delta);
                    }
                    pending.last_error = Some(message.clone());
                    pending.unsurfaced_error = Some(message.clone());
                    if surface_prior_error {
                        pending.unsurfaced_error = None;
                    }
                }
                let mut metrics = lock_unpoisoned(&state.metrics);
                metrics.failures = metrics.failures.saturating_add(1);
                return Err(anyhow!(message));
            }
        }
    }
}

fn save_current_image(state: &PersistenceState, image: &RegistryImage) -> Result<()> {
    await_checkpoint_test_gate(state);
    state
        .backend
        .save_checkpoint(&state.root, &image.tasks, &image.watches)?;
    let mut metrics = lock_unpoisoned(&state.metrics);
    metrics.checkpoints_written = metrics.checkpoints_written.saturating_add(1);
    let checkpoint_bytes = [
        task_registry_path(&state.root),
        watch_registry_path(&state.root),
    ]
    .into_iter()
    .filter_map(|path| std::fs::metadata(path).ok())
    .map(|metadata| metadata.len())
    .sum::<u64>();
    metrics.checkpoint_bytes_written = metrics
        .checkpoint_bytes_written
        .saturating_add(checkpoint_bytes);
    Ok(())
}

#[cfg(test)]
fn await_checkpoint_test_gate(state: &PersistenceState) {
    let gate = {
        let mut installed = lock_unpoisoned(&state.checkpoint_gate);
        let Some(gate) = installed.as_mut() else {
            return;
        };
        if gate.remaining > 1 {
            gate.remaining -= 1;
            return;
        }
        installed.take()
    };
    if let Some(gate) = gate {
        let _ = gate.reached.send(());
        let _ = gate.release.recv();
    }
}

#[cfg(not(test))]
fn await_checkpoint_test_gate(_state: &PersistenceState) {}

fn take_prior_error(pending: &mut PendingState, surface: bool) -> Result<()> {
    if surface {
        if let Some(error) = pending.unsurfaced_error.take() {
            return Err(anyhow!(error));
        }
    }
    Ok(())
}

fn send_until(
    sender: &SyncSender<PersistenceCommand>,
    mut command: PersistenceCommand,
    timeout: Duration,
) -> Result<()> {
    let started = Instant::now();
    loop {
        match sender.try_send(command) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => {
                return Err(anyhow!("daemon persistence worker is stopped"));
            }
            Err(TrySendError::Full(returned)) => {
                command = returned;
                if started.elapsed() >= timeout {
                    return Err(anyhow!("timed out admitting daemon persistence shutdown"));
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use packet28_daemon_core::storage::{
        ensure_daemon_dir, load_task_registry, load_watch_registry,
    };
    use packet28_daemon_core::task_store_lease::acquire_daemon_task_store_lease;
    use packet28_daemon_protocol::task::{TaskRecord, WatchRegistration};
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    fn owner(root: &Path, debounce: Duration) -> (PersistenceOwner, PersistenceHandle) {
        ensure_daemon_dir(root).unwrap();
        let lease = acquire_daemon_task_store_lease(root).unwrap();
        PersistenceOwner::start(
            root.to_path_buf(),
            lease,
            debounce,
            &TaskRegistry::default(),
            &WatchRegistry::default(),
        )
        .unwrap()
    }

    fn task_record(task_id: &str, marker: &str) -> TaskRecord {
        TaskRecord {
            task_id: task_id.to_string(),
            last_error: Some(marker.to_string()),
            ..TaskRecord::default()
        }
    }

    fn task_record_with_watch(task_id: &str, task_marker: &str, watch_id: &str) -> TaskRecord {
        let mut task = task_record(task_id, task_marker);
        task.watch_ids.push(watch_id.to_string());
        task
    }

    fn watch_record(task_id: &str, marker: &str) -> WatchRegistration {
        WatchRegistration {
            watch_id: marker.to_string(),
            spec: packet28_daemon_protocol::commands::WatchSpec {
                task_id: task_id.to_string(),
                ..packet28_daemon_protocol::commands::WatchSpec::default()
            },
            ..WatchRegistration::default()
        }
    }

    fn event(ordinal: u64) -> DaemonEvent {
        DaemonEvent {
            kind: "test".to_string(),
            occurred_at_unix: ordinal,
            data: json!({"ordinal": ordinal}),
        }
    }

    fn checkpoint_generation(path: &Path) -> Option<u64> {
        serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap())
            .unwrap()
            .get("task_watch_checkpoint_generation")
            .and_then(serde_json::Value::as_u64)
    }

    #[test]
    fn pending_slot_coalesces_to_the_latest_immutable_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let (owner, handle) = owner(root.path(), Duration::from_secs(1));
        for ordinal in 0..32 {
            let watch_id = format!("watch-{ordinal}");
            let mut delta = RegistryDelta::default()
                .upsert_task(task_record_with_watch(
                    "coalesced",
                    &format!("task-{ordinal}"),
                    &watch_id,
                ))
                .upsert_watch(watch_record("coalesced", &watch_id));
            if ordinal > 0 {
                delta = delta.remove_watch(format!("watch-{}", ordinal - 1));
            }
            handle.stage(delta).unwrap();
        }
        let metrics = handle.flush().unwrap();
        assert_eq!(
            load_task_registry(root.path()).unwrap().tasks["coalesced"].last_error,
            Some("task-31".to_string())
        );
        assert_eq!(
            load_watch_registry(root.path()).unwrap().watches[0].watch_id,
            "watch-31"
        );
        assert_eq!(metrics.max_pending_batches, 1);
        assert!(metrics.deltas_coalesced > 0);
        assert!(metrics.checkpoints_written < metrics.deltas_submitted);
        owner.shutdown(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn filesystem_barrier_publishes_one_nonzero_paired_generation() {
        let root = tempfile::tempdir().unwrap();
        let (owner, handle) = owner(root.path(), Duration::from_secs(1));

        handle
            .stage_and_flush(
                RegistryDelta::default().upsert_task(task_record("barrier-generation", "durable")),
            )
            .unwrap();

        let task_generation = checkpoint_generation(&task_registry_path(root.path())).unwrap();
        let watch_generation = checkpoint_generation(&watch_registry_path(root.path())).unwrap();
        assert!(task_generation > 0);
        assert_eq!(watch_generation, task_generation);
        owner.shutdown(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn filesystem_shutdown_flushes_one_nonzero_paired_generation() {
        let root = tempfile::tempdir().unwrap();
        let (owner, handle) = owner(root.path(), Duration::from_secs(1));
        handle
            .stage(
                RegistryDelta::default().upsert_task(task_record("shutdown-generation", "durable")),
            )
            .unwrap();

        owner.shutdown(Duration::from_secs(5)).unwrap();

        let task_generation = checkpoint_generation(&task_registry_path(root.path())).unwrap();
        let watch_generation = checkpoint_generation(&watch_registry_path(root.path())).unwrap();
        assert!(task_generation > 0);
        assert_eq!(watch_generation, task_generation);
        assert!(load_task_registry(root.path())
            .unwrap()
            .tasks
            .contains_key("shutdown-generation"));
    }

    #[test]
    fn task_admission_fence_waits_once_for_the_exact_checkpoint_revision() {
        let root = tempfile::tempdir().unwrap();
        let (owner, handle) = owner(root.path(), Duration::from_secs(1));
        let revision = handle
            .stage(RegistryDelta::default().upsert_task(task_record("artifact-owner", "admitted")))
            .unwrap();

        handle
            .ensure_task_admitted("artifact-owner", revision)
            .unwrap();
        let after_first_fence = handle.metrics();
        assert!(handle.task_is_durably_admitted("artifact-owner"));
        handle
            .ensure_task_admitted("artifact-owner", revision)
            .unwrap();
        assert_eq!(handle.metrics(), after_first_fence);
        assert_eq!(after_first_fence.checkpoints_written, 1);
        owner.shutdown(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn event_lane_allocates_contiguous_sequences_under_concurrent_callers() {
        let root = tempfile::tempdir().unwrap();
        let (owner, handle) = owner(root.path(), Duration::from_millis(20));
        handle
            .stage_and_flush(
                RegistryDelta::default().upsert_task(task_record("concurrent", "admitted")),
            )
            .unwrap();
        let mut workers = Vec::new();
        for ordinal in 0..16 {
            let handle = handle.clone();
            workers.push(thread::spawn(move || {
                let _event_guard = handle.event_guard();
                handle
                    .append_event("concurrent", event(ordinal), None)
                    .unwrap()
                    .seq
            }));
        }
        let mut sequences = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        sequences.sort_unstable();
        assert_eq!(sequences, (1..=16).collect::<Vec<_>>());
        let metrics = owner.shutdown(Duration::from_secs(5)).unwrap();
        assert_eq!(metrics.events_submitted, 16);
        assert_eq!(metrics.events_appended, 16);
    }

    struct FailOnceBackend {
        failures_remaining: AtomicUsize,
    }

    impl PersistenceBackend for FailOnceBackend {
        fn save_checkpoint(
            &self,
            _root: &Path,
            _tasks: &TaskRegistry,
            _watches: &WatchRegistry,
        ) -> Result<()> {
            if self.failures_remaining.swap(0, Ordering::AcqRel) > 0 {
                anyhow::bail!("injected checkpoint failure");
            }
            Ok(())
        }

        fn append_event(
            &self,
            _root: &Path,
            task_id: &str,
            event: &DaemonEvent,
        ) -> Result<DaemonEventFrame> {
            Ok(DaemonEventFrame {
                seq: 1,
                task_id: task_id.to_string(),
                event: event.clone(),
            })
        }
    }

    #[test]
    fn failed_checkpoint_remains_dirty_and_the_next_barrier_retries_it() {
        let root = tempfile::tempdir().unwrap();
        ensure_daemon_dir(root.path()).unwrap();
        let lease = acquire_daemon_task_store_lease(root.path()).unwrap();
        let backend = Arc::new(FailOnceBackend {
            failures_remaining: AtomicUsize::new(1),
        });
        let (owner, handle) = PersistenceOwner::start_with_backend(
            root.path().to_path_buf(),
            Some(lease),
            Duration::ZERO,
            TaskRegistry::default(),
            WatchRegistry::default(),
            backend,
        )
        .unwrap();
        let error = handle
            .stage_and_flush(RegistryDelta::default().upsert_task(task_record("retry", "latest")))
            .unwrap_err();
        assert!(error.to_string().contains("injected checkpoint failure"));
        let metrics = handle.flush().unwrap();
        assert_eq!(metrics.checkpoints_written, 1);
        assert_eq!(metrics.failures, 1);
        owner.shutdown(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn shutdown_reports_a_missing_worker_handle_without_panicking() {
        let root = tempfile::tempdir().unwrap();
        let (mut owner, _handle) = owner(root.path(), Duration::ZERO);
        let worker = owner.worker.take().unwrap();

        let error = owner.shutdown(Duration::from_secs(5)).unwrap_err();
        worker.join().unwrap();

        assert!(error
            .to_string()
            .contains("persistence worker handle is unavailable"));
    }

    struct BlockingBackend {
        started: Mutex<Option<SyncSender<()>>>,
        release: Mutex<Receiver<()>>,
        sequence: AtomicU64,
    }

    impl PersistenceBackend for BlockingBackend {
        fn save_checkpoint(
            &self,
            _root: &Path,
            _tasks: &TaskRegistry,
            _watches: &WatchRegistry,
        ) -> Result<()> {
            if let Some(started) = lock_unpoisoned(&self.started).take() {
                let _ = started.send(());
                let _ = lock_unpoisoned(&self.release).recv();
            }
            Ok(())
        }

        fn append_event(
            &self,
            _root: &Path,
            task_id: &str,
            event: &DaemonEvent,
        ) -> Result<DaemonEventFrame> {
            Ok(DaemonEventFrame {
                seq: self.sequence.fetch_add(1, Ordering::AcqRel) + 1,
                task_id: task_id.to_string(),
                event: event.clone(),
            })
        }
    }

    #[test]
    fn shutdown_timeout_is_bounded_while_worker_retains_its_lifecycle_lease() {
        let root = tempfile::tempdir().unwrap();
        ensure_daemon_dir(root.path()).unwrap();
        let lease = acquire_daemon_task_store_lease(root.path()).unwrap();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let backend = Arc::new(BlockingBackend {
            started: Mutex::new(Some(started_tx)),
            release: Mutex::new(release_rx),
            sequence: AtomicU64::new(0),
        });
        let (owner, handle) = PersistenceOwner::start_with_backend(
            root.path().to_path_buf(),
            Some(lease),
            Duration::ZERO,
            TaskRegistry::default(),
            WatchRegistry::default(),
            backend,
        )
        .unwrap();
        handle
            .stage(RegistryDelta::default().upsert_task(task_record("blocked", "pending")))
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        let error = owner.shutdown(Duration::from_millis(20)).unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(200));
        assert!(error.to_string().contains("timed out"));
        release_tx.send(()).unwrap();
        drop(handle);
    }
}
