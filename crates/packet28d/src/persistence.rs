use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use packet28_daemon_core::storage::{
    append_next_task_event, append_task_watch_registry_delta, registry_delta_wal_path,
    save_task_watch_registry_checkpoint_at_revision, RegistryDeltaBatch, RegistryRevision,
    RegistryRevisionRange,
};
use packet28_daemon_core::task_store_lease::TaskStoreLease;
use packet28_daemon_core::DaemonCoreError;
use packet28_daemon_protocol::message::{DaemonEvent, DaemonEventFrame};
use packet28_daemon_protocol::paths::{task_registry_path, watch_registry_path};
use packet28_daemon_protocol::task::{TaskRegistry, WatchRegistry};

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
pub(crate) type RegistryDelta = RegistryDeltaBatch;

struct PendingDelta {
    first_revision: u64,
    last_revision: u64,
    delta: RegistryDelta,
}

impl PendingDelta {
    fn merge_later(&mut self, revision: u64, delta: RegistryDelta) -> Result<()> {
        debug_assert_eq!(revision, self.last_revision.saturating_add(1));
        self.delta
            .merge_later_wins(delta)
            .context("failed to coalesce daemon registry deltas")?;
        self.last_revision = revision;
        Ok(())
    }
}

#[derive(Default)]
struct PendingState {
    next_revision: u64,
    durable_revision: u64,
    durable_task_ids: BTreeSet<String>,
    delta: Option<PendingDelta>,
    unsurfaced_error: Option<String>,
    fatal_error: Option<String>,
}

struct RegistryImage {
    tasks: TaskRegistry,
    watches: WatchRegistry,
    durable_revision: u64,
    checkpoint_revision: u64,
}

#[derive(Clone, Copy)]
struct RegistryRecoveryRevisions {
    checkpoint: RegistryRevision,
    replayed: RegistryRevision,
}

trait PersistenceBackend: Send + Sync {
    fn append_delta(
        &self,
        root: &Path,
        revisions: RegistryRevisionRange,
        delta: &RegistryDelta,
    ) -> Result<u64>;

    fn save_checkpoint(
        &self,
        root: &Path,
        tasks: &TaskRegistry,
        watches: &WatchRegistry,
        revision: RegistryRevision,
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
    fn append_delta(
        &self,
        root: &Path,
        revisions: RegistryRevisionRange,
        delta: &RegistryDelta,
    ) -> Result<u64> {
        let wal_path = registry_delta_wal_path(root);
        let before = std::fs::metadata(&wal_path)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        append_task_watch_registry_delta(root, revisions, delta)?;
        let after = std::fs::metadata(wal_path)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        Ok(after.saturating_sub(before))
    }

    fn save_checkpoint(
        &self,
        root: &Path,
        tasks: &TaskRegistry,
        watches: &WatchRegistry,
        revision: RegistryRevision,
    ) -> Result<()> {
        Ok(save_task_watch_registry_checkpoint_at_revision(
            root, tasks, watches, revision,
        )?)
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
        required_revision: u64,
        require_checkpoint: bool,
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
        checkpoint_revision: RegistryRevision,
        replayed_revision: RegistryRevision,
    ) -> Result<(Self, PersistenceHandle)> {
        Self::start_with_backend(
            root,
            Some(task_store_lease),
            debounce,
            durable_tasks.clone(),
            durable_watches.clone(),
            RegistryRecoveryRevisions {
                checkpoint: checkpoint_revision,
                replayed: replayed_revision,
            },
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
            RegistryRecoveryRevisions {
                checkpoint: RegistryRevision::ZERO,
                replayed: RegistryRevision::ZERO,
            },
            Arc::new(FilesystemBackend),
        )
    }

    fn start_with_backend(
        root: PathBuf,
        task_store_lease: Option<TaskStoreLease>,
        debounce: Duration,
        durable_tasks: TaskRegistry,
        durable_watches: WatchRegistry,
        revisions: RegistryRecoveryRevisions,
        backend: Arc<dyn PersistenceBackend>,
    ) -> Result<(Self, PersistenceHandle)> {
        if revisions.checkpoint > revisions.replayed {
            anyhow::bail!(
                "daemon checkpoint revision {} is ahead of replayed revision {}",
                revisions.checkpoint.get(),
                revisions.replayed.get()
            );
        }
        let (sender, receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let durable_task_ids = if revisions.checkpoint == revisions.replayed {
            durable_tasks.tasks.keys().cloned().collect()
        } else {
            BTreeSet::new()
        };
        let state = Arc::new(PersistenceState {
            root,
            debounce,
            pending: Mutex::new(PendingState {
                next_revision: revisions.replayed.get(),
                durable_revision: revisions.replayed.get(),
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
                        durable_revision: revisions.replayed.get(),
                        checkpoint_revision: revisions.checkpoint.get(),
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
            if let Some(error) = &pending.fatal_error {
                anyhow::bail!("daemon persistence owner is unavailable: {error}");
            }
            let revision = pending
                .next_revision
                .checked_add(1)
                .ok_or_else(|| anyhow!("daemon persistence revision exhausted"))?;
            let coalesced = pending.delta.is_some();
            match pending.delta.as_mut() {
                Some(pending_delta) => pending_delta.merge_later(revision, delta)?,
                None => {
                    pending.delta = Some(PendingDelta {
                        first_revision: revision,
                        last_revision: revision,
                        delta,
                    });
                }
            }
            pending.next_revision = revision;
            let staged = pending
                .delta
                .as_ref()
                .expect("pending delta exists after staging");
            let pending_task_keys = u64::try_from(
                staged
                    .delta
                    .task_upserts
                    .len()
                    .saturating_add(staged.delta.task_removals.len()),
            )
            .unwrap_or(u64::MAX);
            let pending_watch_keys = u64::try_from(
                staged
                    .delta
                    .watch_upserts
                    .len()
                    .saturating_add(staged.delta.watch_removals.len()),
            )
            .unwrap_or(u64::MAX);
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
        let target_revision = self.latest_revision();
        self.checkpoint_through(target_revision)
    }

    fn checkpoint_through(&self, target_revision: u64) -> Result<()> {
        self.ensure_accepting()?;
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
        required_revision: u64,
        require_checkpoint: bool,
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
                require_checkpoint,
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
        self.checkpoint_through(revision)?;
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
    let mut retry_delta = None;
    let mut deadline = (image.durable_revision > image.checkpoint_revision)
        .then(|| Instant::now() + state.debounce);
    loop {
        let command = match deadline {
            Some(checkpoint_deadline) => {
                let remaining = checkpoint_deadline.saturating_duration_since(Instant::now());
                match receiver.recv_timeout(remaining) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => {
                        flush_on_disconnect(&state, &mut image, &mut retry_delta);
                        return;
                    }
                }
            }
            None => match receiver.recv() {
                Ok(command) => Some(command),
                Err(_) => {
                    flush_on_disconnect(&state, &mut image, &mut retry_delta);
                    return;
                }
            },
        };
        let Some(command) = command else {
            service_deadline(&state, &mut image, &mut retry_delta, &mut deadline);
            continue;
        };
        match command {
            PersistenceCommand::Wake => {
                let before = image.durable_revision;
                let result = append_pending(&state, &mut image, &mut retry_delta, None, false);
                update_deadline_after_append(
                    &state,
                    &image,
                    &retry_delta,
                    before,
                    result.is_ok(),
                    &mut deadline,
                );
            }
            PersistenceCommand::Barrier {
                target_revision,
                reply,
            } => {
                let before = image.durable_revision;
                let result = append_pending(
                    &state,
                    &mut image,
                    &mut retry_delta,
                    Some(target_revision),
                    true,
                );
                update_deadline_after_append(
                    &state,
                    &image,
                    &retry_delta,
                    before,
                    result.is_ok(),
                    &mut deadline,
                );
                if result.is_ok() {
                    let mut metrics = lock_unpoisoned(&state.metrics);
                    metrics.barriers_completed = metrics.barriers_completed.saturating_add(1);
                }
                let _ = reply.send(result.map(|_| ()).map_err(|error| error.to_string()));
            }
            PersistenceCommand::Checkpoint {
                target_revision,
                reply,
            } => {
                let before = image.durable_revision;
                let result = append_pending(
                    &state,
                    &mut image,
                    &mut retry_delta,
                    Some(target_revision),
                    true,
                )
                .and_then(|_| save_current_image(&state, &mut image));
                update_deadline_after_explicit_checkpoint(
                    &state,
                    &image,
                    &retry_delta,
                    before,
                    result.is_ok(),
                    &mut deadline,
                );
                let _ = reply.send(result.map_err(|error| error.to_string()));
            }
            PersistenceCommand::AppendEvent {
                required_revision,
                require_checkpoint,
                task_id,
                event,
                reply,
            } => {
                let before = image.durable_revision;
                let result = append_pending(
                    &state,
                    &mut image,
                    &mut retry_delta,
                    Some(required_revision),
                    true,
                )
                .and_then(|_| {
                    if require_checkpoint {
                        save_current_image(&state, &mut image)
                    } else {
                        Ok(())
                    }
                })
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
                        .inspect_err(|_| {
                            let mut metrics = lock_unpoisoned(&state.metrics);
                            metrics.failures = metrics.failures.saturating_add(1);
                        })
                });
                update_deadline_after_append(
                    &state,
                    &image,
                    &retry_delta,
                    before,
                    result.is_ok(),
                    &mut deadline,
                );
                let _ = reply.send(result.map_err(|error| error.to_string()));
            }
            PersistenceCommand::Shutdown {
                target_revision,
                reply,
            } => {
                let result = append_pending(
                    &state,
                    &mut image,
                    &mut retry_delta,
                    Some(target_revision),
                    true,
                )
                .and_then(|_| save_current_image(&state, &mut image))
                .map(|_| *lock_unpoisoned(&state.metrics))
                .map_err(|error| error.to_string());
                // This acknowledgement is the worker's final operation.
                let _ = reply.send(result);
                return;
            }
        }
    }
}

fn service_deadline(
    state: &PersistenceState,
    image: &mut RegistryImage,
    retry_delta: &mut Option<PendingDelta>,
    deadline: &mut Option<Instant>,
) {
    let before = image.durable_revision;
    match append_pending(state, image, retry_delta, None, false) {
        Ok(true) => {
            *deadline = Some(Instant::now() + state.debounce);
        }
        Ok(false) if image.durable_revision > image.checkpoint_revision => {
            if save_current_image(state, image).is_ok() {
                *deadline = None;
            } else {
                *deadline = Some(retry_at(state.debounce));
            }
        }
        Ok(false) => {
            *deadline = None;
        }
        Err(_) => {
            debug_assert_eq!(image.durable_revision, before);
            *deadline = Some(retry_at(state.debounce));
        }
    }
}

fn update_deadline_after_append(
    state: &PersistenceState,
    image: &RegistryImage,
    retry_delta: &Option<PendingDelta>,
    durable_revision_before: u64,
    operation_succeeded: bool,
    deadline: &mut Option<Instant>,
) {
    if image.durable_revision > durable_revision_before {
        *deadline = Some(Instant::now() + state.debounce);
    } else if retry_delta.is_some() || !operation_succeeded {
        *deadline = Some(retry_at(state.debounce));
    } else if image.durable_revision > image.checkpoint_revision && deadline.is_none() {
        *deadline = Some(Instant::now() + state.debounce);
    }
}

fn update_deadline_after_explicit_checkpoint(
    state: &PersistenceState,
    image: &RegistryImage,
    retry_delta: &Option<PendingDelta>,
    durable_revision_before: u64,
    operation_succeeded: bool,
    deadline: &mut Option<Instant>,
) {
    if operation_succeeded && image.durable_revision == image.checkpoint_revision {
        *deadline = None;
        return;
    }
    update_deadline_after_append(
        state,
        image,
        retry_delta,
        durable_revision_before,
        operation_succeeded,
        deadline,
    );
    if image.durable_revision > image.checkpoint_revision {
        *deadline = Some(retry_at(state.debounce));
    }
}

fn retry_at(debounce: Duration) -> Instant {
    Instant::now() + debounce.max(Duration::from_millis(1))
}

fn flush_on_disconnect(
    state: &PersistenceState,
    image: &mut RegistryImage,
    retry_delta: &mut Option<PendingDelta>,
) {
    let target_revision = lock_unpoisoned(&state.pending).next_revision;
    if append_pending(state, image, retry_delta, Some(target_revision), false).is_ok() {
        let _ = save_current_image(state, image);
    }
}

fn append_pending(
    state: &PersistenceState,
    image: &mut RegistryImage,
    retry_delta: &mut Option<PendingDelta>,
    required_revision: Option<u64>,
    surface_prior_error: bool,
) -> Result<bool> {
    let mut appended_any = false;
    loop {
        let pending_delta = retry_delta.take().or_else(|| {
            let mut pending = lock_unpoisoned(&state.pending);
            if pending.fatal_error.is_some() {
                return None;
            }
            if required_revision.is_some_and(|required| pending.durable_revision >= required) {
                return None;
            }
            pending.delta.take()
        });
        let Some(pending_delta) = pending_delta else {
            let mut pending = lock_unpoisoned(&state.pending);
            if let Some(error) = &pending.fatal_error {
                return Err(anyhow!("daemon persistence owner is unavailable: {error}"));
            }
            return match required_revision {
                Some(required) if pending.durable_revision < required => Err(anyhow!(
                    "daemon persistence revision {} is unavailable; durable revision is {}",
                    required,
                    pending.durable_revision
                )),
                _ => take_prior_error(&mut pending, surface_prior_error).map(|()| appended_any),
            };
        };
        await_checkpoint_test_gate(state);
        let revisions = RegistryRevisionRange::new(
            RegistryRevision::new(pending_delta.first_revision),
            RegistryRevision::new(pending_delta.last_revision),
        )
        .context("invalid daemon registry revision range")?;
        match state
            .backend
            .append_delta(&state.root, revisions, &pending_delta.delta)
        {
            Ok(appended_bytes) => {
                if let Err(error) = pending_delta
                    .delta
                    .apply_to(&mut image.tasks, &mut image.watches)
                {
                    let message = format!(
                        "durable daemon registry delta could not be materialized in memory: {error}"
                    );
                    image.durable_revision = pending_delta.last_revision;
                    let mut pending = lock_unpoisoned(&state.pending);
                    pending.durable_revision = pending_delta.last_revision;
                    pending.fatal_error = Some(message.clone());
                    pending.unsurfaced_error = None;
                    let mut metrics = lock_unpoisoned(&state.metrics);
                    metrics.failures = metrics.failures.saturating_add(1);
                    return Err(anyhow!(message));
                }
                image.durable_revision = pending_delta.last_revision;
                {
                    let mut pending = lock_unpoisoned(&state.pending);
                    pending.durable_revision =
                        pending.durable_revision.max(pending_delta.last_revision);
                }
                let mut metrics = lock_unpoisoned(&state.metrics);
                metrics.wal_batches_appended = metrics.wal_batches_appended.saturating_add(1);
                metrics.wal_bytes_appended =
                    metrics.wal_bytes_appended.saturating_add(appended_bytes);
                appended_any = true;
            }
            Err(error) => {
                if matches!(
                    error.downcast_ref::<DaemonCoreError>(),
                    Some(DaemonCoreError::RegistryDeltaWalTooLarge { .. })
                ) && image.durable_revision > image.checkpoint_revision
                {
                    *retry_delta = Some(pending_delta);
                    if let Err(checkpoint_error) = save_current_image(state, image) {
                        let message = checkpoint_error.to_string();
                        let mut pending = lock_unpoisoned(&state.pending);
                        pending.unsurfaced_error =
                            (!surface_prior_error).then_some(message.clone());
                        return Err(anyhow!(message));
                    }
                    continue;
                }
                let message = error.to_string();
                {
                    let mut pending = lock_unpoisoned(&state.pending);
                    pending.unsurfaced_error = Some(message.clone());
                    if surface_prior_error {
                        pending.unsurfaced_error = None;
                    }
                }
                *retry_delta = Some(pending_delta);
                let mut metrics = lock_unpoisoned(&state.metrics);
                metrics.failures = metrics.failures.saturating_add(1);
                return Err(anyhow!(message));
            }
        }
        if required_revision.is_none() {
            return Ok(true);
        }
    }
}

fn save_current_image(state: &PersistenceState, image: &mut RegistryImage) -> Result<()> {
    if image.checkpoint_revision == image.durable_revision {
        return Ok(());
    }
    await_checkpoint_test_gate(state);
    if let Err(error) = state.backend.save_checkpoint(
        &state.root,
        &image.tasks,
        &image.watches,
        RegistryRevision::new(image.durable_revision),
    ) {
        let mut metrics = lock_unpoisoned(&state.metrics);
        metrics.failures = metrics.failures.saturating_add(1);
        return Err(error);
    }
    image.checkpoint_revision = image.durable_revision;
    let durable_task_ids = image.tasks.tasks.keys().cloned().collect();
    lock_unpoisoned(&state.pending).durable_task_ids = durable_task_ids;
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
        ensure_daemon_dir, load_task_events, load_task_registry,
        load_task_watch_registry_with_deltas,
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
            RegistryRevision::ZERO,
            RegistryRevision::ZERO,
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

    #[test]
    fn start_continues_after_the_replayed_registry_revision() {
        let root = tempfile::tempdir().unwrap();
        ensure_daemon_dir(root.path()).unwrap();
        append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::new(RegistryRevision::new(1), RegistryRevision::new(7)).unwrap(),
            &RegistryDelta::default(),
        )
        .unwrap();
        let loaded = load_task_watch_registry_with_deltas(root.path()).unwrap();
        let lease = acquire_daemon_task_store_lease(root.path()).unwrap();
        let (owner, handle) = PersistenceOwner::start(
            root.path().to_path_buf(),
            lease,
            Duration::from_secs(1),
            &loaded.tasks,
            &loaded.watches,
            loaded.checkpoint_revision,
            loaded.replayed_revision,
        )
        .unwrap();

        assert_eq!(handle.latest_revision(), 7);
        owner.shutdown(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn start_does_not_treat_wal_only_tasks_as_checkpoint_admitted() {
        let root = tempfile::tempdir().unwrap();
        ensure_daemon_dir(root.path()).unwrap();
        append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::single(RegistryRevision::new(1)).unwrap(),
            &RegistryDelta::default().upsert_task(task_record("wal-only", "replayed")),
        )
        .unwrap();
        let loaded = load_task_watch_registry_with_deltas(root.path()).unwrap();
        let lease = acquire_daemon_task_store_lease(root.path()).unwrap();
        let (owner, handle) = PersistenceOwner::start(
            root.path().to_path_buf(),
            lease,
            Duration::from_secs(1),
            &loaded.tasks,
            &loaded.watches,
            loaded.checkpoint_revision,
            loaded.replayed_revision,
        )
        .unwrap();

        assert!(!handle.task_is_durably_admitted("wal-only"));
        owner.shutdown(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn start_treats_checkpoint_tasks_as_durably_admitted_at_the_replayed_tail() {
        let root = tempfile::tempdir().unwrap();
        ensure_daemon_dir(root.path()).unwrap();
        append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::new(RegistryRevision::new(1), RegistryRevision::new(5)).unwrap(),
            &RegistryDelta::default().upsert_task(task_record("checkpointed", "durable")),
        )
        .unwrap();
        let loaded = load_task_watch_registry_with_deltas(root.path()).unwrap();
        save_task_watch_registry_checkpoint_at_revision(
            root.path(),
            &loaded.tasks,
            &loaded.watches,
            loaded.replayed_revision,
        )
        .unwrap();
        let loaded = load_task_watch_registry_with_deltas(root.path()).unwrap();
        let lease = acquire_daemon_task_store_lease(root.path()).unwrap();
        let (owner, handle) = PersistenceOwner::start(
            root.path().to_path_buf(),
            lease,
            Duration::from_secs(1),
            &loaded.tasks,
            &loaded.watches,
            loaded.checkpoint_revision,
            loaded.replayed_revision,
        )
        .unwrap();

        assert!(handle.task_is_durably_admitted("checkpointed"));
        owner.shutdown(Duration::from_secs(5)).unwrap();
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
        let loaded = load_task_watch_registry_with_deltas(root.path()).unwrap();
        assert_eq!(
            loaded.tasks.tasks["coalesced"].last_error,
            Some("task-31".to_string())
        );
        assert_eq!(loaded.watches.watches[0].watch_id, "watch-31");
        assert_eq!(metrics.max_pending_batches, 1);
        assert!(metrics.deltas_coalesced > 0);
        assert!(metrics.checkpoints_written < metrics.deltas_submitted);
        owner.shutdown(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn filesystem_barrier_is_replayable_before_the_debounced_checkpoint() {
        let root = tempfile::tempdir().unwrap();
        let (owner, handle) = owner(root.path(), Duration::from_secs(1));

        handle
            .stage_and_flush(
                RegistryDelta::default().upsert_task(task_record("barrier-generation", "durable")),
            )
            .unwrap();

        assert!(!task_registry_path(root.path()).exists());
        assert!(!watch_registry_path(root.path()).exists());
        assert!(registry_delta_wal_path(root.path()).exists());
        let loaded = load_task_watch_registry_with_deltas(root.path()).unwrap();
        assert_eq!(loaded.checkpoint_revision, RegistryRevision::ZERO);
        assert_eq!(loaded.replayed_revision, RegistryRevision::new(1));
        assert_eq!(
            loaded.tasks.tasks["barrier-generation"]
                .last_error
                .as_deref(),
            Some("durable")
        );
        owner.shutdown(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn watch_runtime_metadata_is_replayable_before_the_next_checkpoint() {
        let root = tempfile::tempdir().unwrap();
        let (owner, handle) = owner(root.path(), Duration::from_secs(60));
        let task = task_record_with_watch("watch-owner", "initial", "watch-runtime");
        let mut initial_watch = watch_record("watch-owner", "watch-runtime");
        initial_watch.active = false;
        handle
            .stage(
                RegistryDelta::default()
                    .upsert_task(task)
                    .upsert_watch(initial_watch),
            )
            .unwrap();
        handle.checkpoint_current().unwrap();

        let mut updated_watch = watch_record("watch-owner", "watch-runtime");
        updated_watch.active = true;
        updated_watch.last_event_at_unix = Some(99);
        updated_watch.last_error = Some("transient".to_string());
        let revision = handle
            .stage(RegistryDelta::default().upsert_watch(updated_watch))
            .unwrap();
        let frame = handle
            .append_event("watch-owner", event(1), revision, false)
            .unwrap();
        assert_eq!(frame.seq, 1);

        let checkpointed = packet28_daemon_core::storage::load_watch_registry(root.path()).unwrap();
        assert!(!checkpointed.watches[0].active);
        let replayed = load_task_watch_registry_with_deltas(root.path()).unwrap();
        let replayed_watch = &replayed.watches.watches[0];
        assert!(replayed_watch.active);
        assert_eq!(replayed_watch.last_event_at_unix, Some(99));
        assert_eq!(replayed_watch.last_error.as_deref(), Some("transient"));
        owner.shutdown(Duration::from_secs(5)).unwrap();
    }

    struct EventFailBackend;

    impl PersistenceBackend for EventFailBackend {
        fn append_delta(
            &self,
            root: &Path,
            revisions: RegistryRevisionRange,
            delta: &RegistryDelta,
        ) -> Result<u64> {
            FilesystemBackend.append_delta(root, revisions, delta)
        }

        fn save_checkpoint(
            &self,
            root: &Path,
            tasks: &TaskRegistry,
            watches: &WatchRegistry,
            revision: RegistryRevision,
        ) -> Result<()> {
            FilesystemBackend.save_checkpoint(root, tasks, watches, revision)
        }

        fn append_event(
            &self,
            _root: &Path,
            _task_id: &str,
            _event: &DaemonEvent,
        ) -> Result<DaemonEventFrame> {
            anyhow::bail!("injected event append failure")
        }
    }

    #[test]
    fn failed_event_after_causal_delta_keeps_the_registry_delta_replayable() {
        let root = tempfile::tempdir().unwrap();
        let task = task_record_with_watch("event-crash", "initial", "event-crash-watch");
        let mut initial_watch = watch_record("event-crash", "event-crash-watch");
        initial_watch.active = false;
        let (initial_owner, initial_handle) = owner(root.path(), Duration::from_secs(60));
        initial_handle
            .stage(
                RegistryDelta::default()
                    .upsert_task(task)
                    .upsert_watch(initial_watch),
            )
            .unwrap();
        initial_handle.checkpoint_current().unwrap();
        initial_owner.shutdown(Duration::from_secs(5)).unwrap();

        let loaded = load_task_watch_registry_with_deltas(root.path()).unwrap();
        let lease = acquire_daemon_task_store_lease(root.path()).unwrap();
        let (owner, handle) = PersistenceOwner::start_with_backend(
            root.path().to_path_buf(),
            Some(lease),
            Duration::from_secs(60),
            loaded.tasks,
            loaded.watches,
            RegistryRecoveryRevisions {
                checkpoint: loaded.checkpoint_revision,
                replayed: loaded.replayed_revision,
            },
            Arc::new(EventFailBackend),
        )
        .unwrap();

        let mut updated_watch = watch_record("event-crash", "event-crash-watch");
        updated_watch.active = true;
        updated_watch.last_event_at_unix = Some(73);
        let revision = handle
            .stage(RegistryDelta::default().upsert_watch(updated_watch))
            .unwrap();
        let error = handle
            .append_event("event-crash", event(1), revision, false)
            .unwrap_err();
        assert!(error.to_string().contains("injected event append failure"));

        let replayed = load_task_watch_registry_with_deltas(root.path()).unwrap();
        assert_eq!(replayed.replayed_revision, RegistryRevision::new(2));
        assert!(replayed.watches.watches[0].active);
        assert_eq!(replayed.watches.watches[0].last_event_at_unix, Some(73));
        assert!(load_task_events(root.path(), "event-crash")
            .unwrap()
            .is_empty());
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
        let revision = handle
            .stage(RegistryDelta::default().upsert_task(task_record("concurrent", "admitted")))
            .unwrap();
        handle.ensure_task_admitted("concurrent", revision).unwrap();
        let mut workers = Vec::new();
        for ordinal in 0..16 {
            let handle = handle.clone();
            workers.push(thread::spawn(move || {
                let _event_guard = handle.event_guard();
                handle
                    .append_event(
                        "concurrent",
                        event(ordinal),
                        handle.latest_revision(),
                        false,
                    )
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
        fn append_delta(
            &self,
            _root: &Path,
            _revisions: RegistryRevisionRange,
            _delta: &RegistryDelta,
        ) -> Result<u64> {
            if self.failures_remaining.swap(0, Ordering::AcqRel) > 0 {
                anyhow::bail!("injected WAL append failure");
            }
            Ok(1)
        }

        fn save_checkpoint(
            &self,
            _root: &Path,
            _tasks: &TaskRegistry,
            _watches: &WatchRegistry,
            _revision: RegistryRevision,
        ) -> Result<()> {
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
    fn failed_wal_append_remains_dirty_and_the_next_barrier_retries_it() {
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
            RegistryRecoveryRevisions {
                checkpoint: RegistryRevision::ZERO,
                replayed: RegistryRevision::ZERO,
            },
            backend,
        )
        .unwrap();
        let error = handle
            .stage_and_flush(RegistryDelta::default().upsert_task(task_record("retry", "latest")))
            .unwrap_err();
        assert!(error.to_string().contains("injected WAL append failure"));
        let metrics = handle.flush().unwrap();
        assert_eq!(metrics.wal_batches_appended, 1);
        assert_eq!(metrics.failures, 1);
        let metrics = owner.shutdown(Duration::from_secs(5)).unwrap();
        assert_eq!(metrics.checkpoints_written, 1);
    }

    struct CheckpointFailOnceBackend {
        failures_remaining: AtomicUsize,
    }

    impl PersistenceBackend for CheckpointFailOnceBackend {
        fn append_delta(
            &self,
            root: &Path,
            revisions: RegistryRevisionRange,
            delta: &RegistryDelta,
        ) -> Result<u64> {
            FilesystemBackend.append_delta(root, revisions, delta)
        }

        fn save_checkpoint(
            &self,
            root: &Path,
            tasks: &TaskRegistry,
            watches: &WatchRegistry,
            revision: RegistryRevision,
        ) -> Result<()> {
            if self.failures_remaining.swap(0, Ordering::AcqRel) > 0 {
                anyhow::bail!("injected checkpoint failure");
            }
            FilesystemBackend.save_checkpoint(root, tasks, watches, revision)
        }

        fn append_event(
            &self,
            root: &Path,
            task_id: &str,
            event: &DaemonEvent,
        ) -> Result<DaemonEventFrame> {
            FilesystemBackend.append_event(root, task_id, event)
        }
    }

    #[test]
    fn checkpoint_failure_keeps_the_durable_wal_replayable_and_retries_without_reappend() {
        let root = tempfile::tempdir().unwrap();
        ensure_daemon_dir(root.path()).unwrap();
        let lease = acquire_daemon_task_store_lease(root.path()).unwrap();
        let backend = Arc::new(CheckpointFailOnceBackend {
            failures_remaining: AtomicUsize::new(1),
        });
        let (owner, handle) = PersistenceOwner::start_with_backend(
            root.path().to_path_buf(),
            Some(lease),
            Duration::from_secs(60),
            TaskRegistry::default(),
            WatchRegistry::default(),
            RegistryRecoveryRevisions {
                checkpoint: RegistryRevision::ZERO,
                replayed: RegistryRevision::ZERO,
            },
            backend,
        )
        .unwrap();

        handle
            .stage_and_flush(
                RegistryDelta::default()
                    .upsert_task(task_record("checkpoint-retry", "wal-durable")),
            )
            .unwrap();
        assert_eq!(handle.metrics().wal_batches_appended, 1);
        assert!(!handle.task_is_durably_admitted("checkpoint-retry"));
        assert_eq!(
            load_task_watch_registry_with_deltas(root.path())
                .unwrap()
                .tasks
                .tasks["checkpoint-retry"]
                .last_error
                .as_deref(),
            Some("wal-durable")
        );

        let error = handle.checkpoint_current().unwrap_err();
        assert!(error.to_string().contains("injected checkpoint failure"));
        assert_eq!(handle.metrics().wal_batches_appended, 1);
        assert!(!handle.task_is_durably_admitted("checkpoint-retry"));

        handle.checkpoint_current().unwrap();
        assert_eq!(handle.metrics().wal_batches_appended, 1);
        assert_eq!(handle.metrics().checkpoints_written, 1);
        assert!(handle.task_is_durably_admitted("checkpoint-retry"));
        assert!(load_task_registry(root.path())
            .unwrap()
            .tasks
            .contains_key("checkpoint-retry"));
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
        fn append_delta(
            &self,
            _root: &Path,
            _revisions: RegistryRevisionRange,
            _delta: &RegistryDelta,
        ) -> Result<u64> {
            if let Some(started) = lock_unpoisoned(&self.started).take() {
                let _ = started.send(());
                let _ = lock_unpoisoned(&self.release).recv();
            }
            Ok(1)
        }

        fn save_checkpoint(
            &self,
            _root: &Path,
            _tasks: &TaskRegistry,
            _watches: &WatchRegistry,
            _revision: RegistryRevision,
        ) -> Result<()> {
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
            RegistryRecoveryRevisions {
                checkpoint: RegistryRevision::ZERO,
                replayed: RegistryRevision::ZERO,
            },
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
