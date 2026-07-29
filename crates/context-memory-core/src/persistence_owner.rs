use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::persist::{append_wal_record, replay_wal, reset_wal, truncate_wal_to, PersistDelta};
use crate::{PacketCache, PacketCacheEntry, PersistConfig};

const PERSISTENCE_QUEUE_CAPACITY: usize = 256;
const PERSISTENCE_DEBOUNCE: Duration = Duration::from_millis(20);
const CHECKPOINT_DELTA_THRESHOLD: u64 = 256;
const CHECKPOINT_WAL_BYTES_THRESHOLD: u64 = 4 * 1024 * 1024;
const PERSISTENCE_COORDINATION_FILE: &str = "packet-cache-v3.lock";
const MAX_DIRTY_DELTAS: usize = 4_096;

type CachePersistenceRegistry = HashMap<PathBuf, Weak<CachePersistenceOwner>>;

fn cache_persistence_registry() -> &'static Mutex<CachePersistenceRegistry> {
    static REGISTRY: OnceLock<Mutex<CachePersistenceRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn persistence_root_key(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| {
        if root.is_absolute() {
            root.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|current| current.join(root))
                .unwrap_or_else(|_| root.to_path_buf())
        }
    })
}

fn registry_entry_matches(
    registry: &CachePersistenceRegistry,
    root_key: &Path,
    owner: &CachePersistenceOwner,
) -> bool {
    registry
        .get(root_key)
        .is_some_and(|registered| std::ptr::eq(registered.as_ptr(), owner))
}

struct RootPersistenceLock {
    file: File,
}

struct LockedPersistenceRoot<'a> {
    file: &'a mut File,
}

impl RootPersistenceLock {
    fn open(config: &PersistConfig) -> Result<Self, CachePersistenceError> {
        let cache_dir = config.root_dir.join(crate::PERSIST_CACHE_DIR);
        std::fs::create_dir_all(&cache_dir)
            .map_err(|source| io_error("coordination lock open", source))?;
        let lock_path = cache_dir.join(PERSISTENCE_COORDINATION_FILE);
        let created = !lock_path.exists();
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| io_error("coordination lock open", source))?;
        if created {
            sync_directory(&cache_dir)
                .map_err(|source| io_error("coordination lock open", source))?;
        }
        Ok(Self { file })
    }

    fn lock(&mut self) -> Result<LockedPersistenceRoot<'_>, CachePersistenceError> {
        FileExt::lock_exclusive(&self.file)
            .map_err(|source| io_error("coordination lock acquire", source))?;
        Ok(LockedPersistenceRoot {
            file: &mut self.file,
        })
    }
}

impl LockedPersistenceRoot<'_> {
    fn generation(&mut self) -> Result<u64, CachePersistenceError> {
        let length = self
            .file
            .seek(SeekFrom::End(0))
            .map_err(|source| io_error("coordination generation read", source))?;
        if length == 0 {
            return Ok(0);
        }
        if length != std::mem::size_of::<u64>() as u64 {
            return Err(CachePersistenceError::Io {
                operation: "coordination generation read",
                detail: format!("expected 8-byte generation marker, found {length} bytes"),
            });
        }
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|source| io_error("coordination generation read", source))?;
        let mut encoded = [0u8; std::mem::size_of::<u64>()];
        self.file
            .read_exact(&mut encoded)
            .map_err(|source| io_error("coordination generation read", source))?;
        Ok(u64::from_le_bytes(encoded))
    }

    fn advance_generation(&mut self, current: u64) -> Result<u64, CachePersistenceError> {
        let next = current
            .checked_add(1)
            .ok_or_else(|| CachePersistenceError::Io {
                operation: "coordination generation write",
                detail: "generation counter exhausted".to_string(),
            })?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|source| io_error("coordination generation write", source))?;
        self.file
            .write_all(&next.to_le_bytes())
            .map_err(|source| io_error("coordination generation write", source))?;
        self.file
            .set_len(std::mem::size_of::<u64>() as u64)
            .map_err(|source| io_error("coordination generation write", source))?;
        self.file
            .sync_data()
            .map_err(|source| io_error("coordination generation write", source))?;
        Ok(next)
    }
}

impl Drop for LockedPersistenceRoot<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(self.file);
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

/// Failures reported by the context-cache persistence owner.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CachePersistenceError {
    /// The persistence thread could not be created.
    #[error("failed to start cache persistence worker: {detail}")]
    Start { detail: String },

    /// A live owner already uses the root with a different retention policy.
    #[error(
        "cache persistence root already uses ttl {existing_ttl_secs}s; \
         requested ttl was {requested_ttl_secs}s"
    )]
    ConfigurationConflict {
        existing_ttl_secs: u64,
        requested_ttl_secs: u64,
    },

    /// The bounded dirty map could not accept a complete mutation batch.
    #[error(
        "cache persistence backpressure: {pending} pending keys at capacity {capacity}; \
         batch requires {requested_new_keys} new keys"
    )]
    Backpressure {
        capacity: usize,
        pending: usize,
        requested_new_keys: usize,
    },

    /// A mutation reservation was created by a different persistence root.
    #[error("cache persistence mutation reservation belongs to a different root owner")]
    ReservationOwnerMismatch,

    /// The persistence worker stopped before accepting an operation.
    #[error("cache persistence worker is unavailable")]
    WorkerUnavailable,

    /// A bounded flush or shutdown did not finish in time.
    #[error("cache persistence {operation} timed out after {timeout_ms} ms")]
    Timeout {
        operation: &'static str,
        timeout_ms: u64,
    },

    /// Encoding or filesystem I/O failed in the persistence worker.
    #[error("cache persistence {operation} failed: {detail}")]
    Io {
        operation: &'static str,
        detail: String,
    },

    /// The persistence worker panicked while it was being joined.
    #[error("cache persistence worker panicked")]
    WorkerPanicked,
}

/// Monotonic counters for cache-persistence write amplification and lifecycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachePersistenceMetrics {
    pub enqueued_batches: u64,
    pub enqueued_deltas: u64,
    pub backpressure_events: u64,
    pub wake_coalesces: u64,
    pub rejected_batches: u64,
    pub rejected_deltas: u64,
    pub max_pending_deltas: u64,
    pub persisted_deltas: u64,
    pub wal_records: u64,
    pub wal_bytes: u64,
    pub coordination_bytes: u64,
    pub checkpoints: u64,
    pub checkpoint_bytes: u64,
    pub flushes: u64,
    pub failures: u64,
}

#[derive(Default)]
struct SharedMetrics {
    enqueued_batches: AtomicU64,
    enqueued_deltas: AtomicU64,
    backpressure_events: AtomicU64,
    wake_coalesces: AtomicU64,
    rejected_batches: AtomicU64,
    rejected_deltas: AtomicU64,
    max_pending_deltas: AtomicU64,
    persisted_deltas: AtomicU64,
    wal_records: AtomicU64,
    wal_bytes: AtomicU64,
    coordination_bytes: AtomicU64,
    checkpoints: AtomicU64,
    checkpoint_bytes: AtomicU64,
    flushes: AtomicU64,
    failures: AtomicU64,
}

impl SharedMetrics {
    fn snapshot(&self) -> CachePersistenceMetrics {
        CachePersistenceMetrics {
            enqueued_batches: self.enqueued_batches.load(Ordering::Relaxed),
            enqueued_deltas: self.enqueued_deltas.load(Ordering::Relaxed),
            backpressure_events: self.backpressure_events.load(Ordering::Relaxed),
            wake_coalesces: self.wake_coalesces.load(Ordering::Relaxed),
            rejected_batches: self.rejected_batches.load(Ordering::Relaxed),
            rejected_deltas: self.rejected_deltas.load(Ordering::Relaxed),
            max_pending_deltas: self.max_pending_deltas.load(Ordering::Relaxed),
            persisted_deltas: self.persisted_deltas.load(Ordering::Relaxed),
            wal_records: self.wal_records.load(Ordering::Relaxed),
            wal_bytes: self.wal_bytes.load(Ordering::Relaxed),
            coordination_bytes: self.coordination_bytes.load(Ordering::Relaxed),
            checkpoints: self.checkpoints.load(Ordering::Relaxed),
            checkpoint_bytes: self.checkpoint_bytes.load(Ordering::Relaxed),
            flushes: self.flushes.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone)]
struct PendingDelta {
    revision: u64,
    delta: PersistDelta,
}

type CommandReply = mpsc::Sender<Result<CachePersistenceMetrics, CachePersistenceError>>;
type ShutdownCompletion = Arc<(
    Mutex<Option<Result<CachePersistenceMetrics, CachePersistenceError>>>,
    Condvar,
)>;

enum PersistenceCommand {
    Wake,
    Flush(CommandReply),
    Shutdown,
}

/// Single-owner, bounded persistence pipeline for a [`PacketCache`].
///
/// Cache mutations remain immediately visible in the caller's in-memory cache.
/// This owner coalesces per-key dirty deltas, appends a checksummed WAL record
/// after a short debounce, and periodically folds the WAL into a checkpoint.
pub struct CachePersistence {
    owner: Arc<CachePersistenceOwner>,
}

/// Capacity and ordering reserved before a live cache mutation is exposed.
///
/// Dropping an unused reservation releases its bounded admission slots.
pub struct CacheMutationReservation {
    owner: Arc<CachePersistenceOwner>,
    revision: u64,
    reserved_slots: usize,
}

struct CachePersistenceOwner {
    root_key: PathBuf,
    ttl_secs: u64,
    handle_count: AtomicUsize,
    memory: Arc<Mutex<PacketCache>>,
    lifecycle: Mutex<PersistenceLifecycle>,
    lifecycle_changed: Condvar,
    metrics: Arc<SharedMetrics>,
    last_error: Arc<Mutex<Option<CachePersistenceError>>>,
    dirty: Arc<Mutex<BTreeMap<String, PendingDelta>>>,
    pending_slots: Arc<AtomicUsize>,
    next_revision: AtomicU64,
    shutdown_completion: ShutdownCompletion,
}

struct PersistenceLifecycle {
    sender: Option<SyncSender<PersistenceCommand>>,
    worker: Option<JoinHandle<()>>,
    shutdown_requested: bool,
    shutdown_command_sent: bool,
    active_reservations: usize,
}

impl CachePersistence {
    /// Opens the single process owner and live cache for a persistence root.
    ///
    /// Repeated opens for the same canonical root share one worker, dirty map,
    /// and immediately visible in-memory cache.
    pub fn open(config: PersistConfig) -> Result<Self, CachePersistenceError> {
        let root_key = persistence_root_key(&config.root_dir);
        let mut registry = lock_recover(cache_persistence_registry());
        if let Some(owner) = registry.get(&root_key).and_then(Weak::upgrade) {
            if owner.ttl_secs != config.ttl_secs {
                return Err(CachePersistenceError::ConfigurationConflict {
                    existing_ttl_secs: owner.ttl_secs,
                    requested_ttl_secs: config.ttl_secs,
                });
            }
            owner.handle_count.fetch_add(1, Ordering::AcqRel);
            return Ok(Self { owner });
        }
        registry.remove(&root_key);

        let mut root_lock = RootPersistenceLock::open(&config)?;
        let (cache, observed_generation) = {
            let mut locked_root = root_lock.lock()?;
            let cache = PacketCache::load_from_disk(&config);
            let current = locked_root.generation()?;
            let observed_generation = if repair_torn_wal_tail(&config, &cache)? {
                locked_root.advance_generation(current)?
            } else {
                current
            };
            (cache, observed_generation)
        };
        let memory = Arc::new(Mutex::new(cache.clone()));
        let ttl_secs = config.ttl_secs;
        let (sender, receiver) = mpsc::sync_channel(PERSISTENCE_QUEUE_CAPACITY);
        let metrics = Arc::new(SharedMetrics::default());
        let last_error = Arc::new(Mutex::new(None));
        let dirty = Arc::new(Mutex::new(BTreeMap::new()));
        let pending_slots = Arc::new(AtomicUsize::new(0));
        let shutdown_completion = Arc::new((Mutex::new(None), Condvar::new()));
        let worker_metrics = metrics.clone();
        let worker_last_error = last_error.clone();
        let worker_dirty = dirty.clone();
        let worker_pending_slots = pending_slots.clone();
        let worker_shutdown_completion = shutdown_completion.clone();
        let worker = thread::Builder::new()
            .name("packet28-cache-persistence".to_string())
            .spawn(move || {
                PersistenceWorker {
                    config,
                    cache,
                    receiver,
                    metrics: worker_metrics,
                    last_error: worker_last_error,
                    dirty: worker_dirty,
                    pending_slots: worker_pending_slots,
                    shutdown_completion: worker_shutdown_completion,
                    root_lock,
                    observed_generation,
                    persisted_revisions: HashMap::new(),
                    persisted_since_checkpoint: 0,
                    wal_bytes_since_checkpoint: 0,
                }
                .run();
            })
            .map_err(|source| CachePersistenceError::Start {
                detail: source.to_string(),
            })?;

        let owner = Arc::new(CachePersistenceOwner {
            root_key: root_key.clone(),
            ttl_secs,
            handle_count: AtomicUsize::new(1),
            memory,
            lifecycle: Mutex::new(PersistenceLifecycle {
                sender: Some(sender),
                worker: Some(worker),
                shutdown_requested: false,
                shutdown_command_sent: false,
                active_reservations: 0,
            }),
            lifecycle_changed: Condvar::new(),
            metrics,
            last_error,
            dirty,
            pending_slots,
            next_revision: AtomicU64::new(0),
            shutdown_completion,
        });
        registry.insert(root_key, Arc::downgrade(&owner));
        Ok(Self { owner })
    }

    /// Compatibility constructor; the root is reloaded while holding its
    /// coordination lease so a caller-supplied stale snapshot cannot win a
    /// cross-process race.
    pub fn start(
        config: PersistConfig,
        _snapshot: PacketCache,
    ) -> Result<Self, CachePersistenceError> {
        Self::open(config)
    }

    /// Returns the process-root cache shared by every live owner handle.
    pub fn shared_cache(&self) -> Arc<Mutex<PacketCache>> {
        self.owner.memory.clone()
    }

    /// Queues one cache upsert and any tombstones created by the same mutation.
    ///
    /// The bounded queue applies backpressure only after the caller has
    /// released its cache lock.
    pub fn record_update(
        &self,
        entry: &PacketCacheEntry,
        removed_cache_keys: Vec<String>,
    ) -> Result<(), CachePersistenceError> {
        let unique_keys = removed_cache_keys
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(entry.cache_key.as_str()))
            .collect::<BTreeSet<_>>()
            .len();
        let reservation = self.reserve_mutation(unique_keys)?;
        self.record_update_reserved(entry, removed_cache_keys, reservation)
    }

    /// Reserves bounded capacity and a root-global mutation revision.
    ///
    /// Callers must reserve while still holding the live cache mutation lock,
    /// mutate only after this succeeds, then enqueue the resulting deltas with
    /// the reservation after releasing that lock.
    pub fn reserve_mutation(
        &self,
        unique_key_count: usize,
    ) -> Result<CacheMutationReservation, CachePersistenceError> {
        {
            let mut lifecycle = lock_recover(&self.owner.lifecycle);
            if lifecycle.shutdown_requested
                || lifecycle.sender.is_none()
                || lifecycle
                    .worker
                    .as_ref()
                    .is_none_or(JoinHandle::is_finished)
            {
                return Err(CachePersistenceError::WorkerUnavailable);
            }
            lifecycle.active_reservations = lifecycle.active_reservations.saturating_add(1);
        }
        let revision = self
            .owner
            .next_revision
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        if unique_key_count == 0 {
            return Ok(CacheMutationReservation {
                owner: self.owner.clone(),
                revision,
                reserved_slots: 0,
            });
        }
        let admission =
            self.owner
                .pending_slots
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                    pending
                        .checked_add(unique_key_count)
                        .filter(|next| *next <= MAX_DIRTY_DELTAS)
                });
        let pending = match admission {
            Ok(previous) => previous.saturating_add(unique_key_count),
            Err(pending) => {
                self.owner.finish_reservation();
                self.owner
                    .metrics
                    .backpressure_events
                    .fetch_add(1, Ordering::Relaxed);
                self.owner
                    .metrics
                    .rejected_batches
                    .fetch_add(1, Ordering::Relaxed);
                self.owner
                    .metrics
                    .rejected_deltas
                    .fetch_add(unique_key_count as u64, Ordering::Relaxed);
                return Err(CachePersistenceError::Backpressure {
                    capacity: MAX_DIRTY_DELTAS,
                    pending,
                    requested_new_keys: unique_key_count,
                });
            }
        };
        self.owner
            .metrics
            .max_pending_deltas
            .fetch_max(pending as u64, Ordering::Relaxed);
        Ok(CacheMutationReservation {
            owner: self.owner.clone(),
            revision,
            reserved_slots: unique_key_count,
        })
    }

    /// Queues one cache mutation using capacity reserved before mutation.
    pub fn record_update_reserved(
        &self,
        entry: &PacketCacheEntry,
        removed_cache_keys: Vec<String>,
        reservation: CacheMutationReservation,
    ) -> Result<(), CachePersistenceError> {
        let mut batch = BTreeMap::new();
        batch.insert(entry.cache_key.clone(), PersistDelta::upsert(entry));
        for cache_key in removed_cache_keys {
            batch.insert(cache_key.clone(), PersistDelta::remove(cache_key));
        }
        self.record_reserved_batch(batch, reservation)
    }

    /// Queues tombstones using capacity reserved before pruning.
    pub fn record_removals_reserved(
        &self,
        removed_cache_keys: Vec<String>,
        reservation: CacheMutationReservation,
    ) -> Result<(), CachePersistenceError> {
        let batch = removed_cache_keys
            .into_iter()
            .map(|cache_key| (cache_key.clone(), PersistDelta::remove(cache_key)))
            .collect();
        self.record_reserved_batch(batch, reservation)
    }

    fn record_reserved_batch(
        &self,
        batch: BTreeMap<String, PersistDelta>,
        mut reservation: CacheMutationReservation,
    ) -> Result<(), CachePersistenceError> {
        if !Arc::ptr_eq(&self.owner, &reservation.owner) {
            return Err(CachePersistenceError::ReservationOwnerMismatch);
        }
        if batch.is_empty() {
            return Ok(());
        }
        if batch.len() > reservation.reserved_slots {
            return Err(CachePersistenceError::Backpressure {
                capacity: reservation.reserved_slots,
                pending: self.owner.pending_slots.load(Ordering::Acquire),
                requested_new_keys: batch.len(),
            });
        }
        let revision = reservation.revision;
        let pending = {
            let mut dirty = lock_recover(&self.owner.dirty);
            let mut retained_slots = 0usize;
            for (cache_key, delta) in batch
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
            {
                match dirty.entry(cache_key) {
                    std::collections::btree_map::Entry::Vacant(slot) => {
                        slot.insert(PendingDelta { revision, delta });
                        retained_slots = retained_slots.saturating_add(1);
                    }
                    std::collections::btree_map::Entry::Occupied(mut slot)
                        if slot.get().revision <= revision =>
                    {
                        slot.insert(PendingDelta { revision, delta });
                    }
                    std::collections::btree_map::Entry::Occupied(_) => {}
                }
            }
            self.owner
                .release_pending_slots(reservation.reserved_slots.saturating_sub(retained_slots));
            reservation.reserved_slots = 0;
            dirty.len()
        };
        self.owner
            .metrics
            .enqueued_batches
            .fetch_add(1, Ordering::Relaxed);
        self.owner
            .metrics
            .enqueued_deltas
            .fetch_add(batch.len() as u64, Ordering::Relaxed);
        debug_assert!(pending <= MAX_DIRTY_DELTAS);
        self.wake_worker()
    }

    /// Waits up to `timeout` for all previously queued deltas to reach the WAL.
    pub fn flush(
        &self,
        timeout: Duration,
    ) -> Result<CachePersistenceMetrics, CachePersistenceError> {
        self.control("flush", timeout, PersistenceCommand::Flush)
    }

    /// Returns a lock-free snapshot of persistence counters.
    pub fn metrics(&self) -> CachePersistenceMetrics {
        self.owner.metrics.snapshot()
    }

    /// Returns the most recent background persistence error, if any.
    pub fn last_error(&self) -> Option<CachePersistenceError> {
        lock_recover(&self.owner.last_error).clone()
    }

    /// Flushes and, for the final root handle, checkpoints and joins the owner.
    ///
    /// A non-final handle cannot stop or checkpoint the shared root owner; it
    /// only waits for all dirty deltas already visible to that owner.
    pub fn shutdown(
        &self,
        timeout: Duration,
    ) -> Result<CachePersistenceMetrics, CachePersistenceError> {
        let mut registry = lock_recover(cache_persistence_registry());
        if self.owner.handle_count.load(Ordering::Acquire) > 1 {
            drop(registry);
            return self.flush(timeout);
        }

        let result = self.owner.shutdown_inner(timeout);
        if self.owner.worker_is_none()
            && registry_entry_matches(&registry, &self.owner.root_key, &self.owner)
        {
            registry.remove(&self.owner.root_key);
        }
        result
    }

    fn wake_worker(&self) -> Result<(), CachePersistenceError> {
        let sender = self.owner.sender()?;
        match sender.try_send(PersistenceCommand::Wake) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.owner
                    .metrics
                    .wake_coalesces
                    .fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(TrySendError::Disconnected(_)) => Err(CachePersistenceError::WorkerUnavailable),
        }
    }

    fn control<F>(
        &self,
        operation: &'static str,
        timeout: Duration,
        command: F,
    ) -> Result<CachePersistenceMetrics, CachePersistenceError>
    where
        F: FnOnce(CommandReply) -> PersistenceCommand,
    {
        let Some(deadline) = Instant::now().checked_add(timeout) else {
            return Err(timeout_error(operation, timeout));
        };
        let (reply_sender, reply_receiver) = mpsc::channel();
        self.owner
            .send_before_deadline(operation, timeout, deadline, command(reply_sender))?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        reply_receiver.recv_timeout(remaining).map_err(|source| {
            if matches!(source, RecvTimeoutError::Timeout) {
                timeout_error(operation, timeout)
            } else {
                CachePersistenceError::WorkerUnavailable
            }
        })?
    }
}

impl CachePersistenceOwner {
    fn finish_reservation(&self) {
        let mut lifecycle = lock_recover(&self.lifecycle);
        lifecycle.active_reservations = lifecycle.active_reservations.saturating_sub(1);
        self.lifecycle_changed.notify_all();
    }

    fn release_pending_slots(&self, count: usize) {
        if count > 0 {
            let previous = self.pending_slots.fetch_sub(count, Ordering::AcqRel);
            debug_assert!(previous >= count);
        }
    }

    fn sender(&self) -> Result<SyncSender<PersistenceCommand>, CachePersistenceError> {
        lock_recover(&self.lifecycle)
            .sender
            .clone()
            .ok_or(CachePersistenceError::WorkerUnavailable)
    }

    fn send_before_deadline(
        &self,
        operation: &'static str,
        timeout: Duration,
        deadline: Instant,
        mut command: PersistenceCommand,
    ) -> Result<(), CachePersistenceError> {
        let sender = self.sender()?;
        loop {
            match sender.try_send(command) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(returned)) => {
                    command = returned;
                    if Instant::now() >= deadline {
                        return Err(timeout_error(operation, timeout));
                    }
                    thread::yield_now();
                }
                Err(TrySendError::Disconnected(_)) => {
                    return Err(CachePersistenceError::WorkerUnavailable);
                }
            }
        }
    }

    fn shutdown_inner(
        &self,
        timeout: Duration,
    ) -> Result<CachePersistenceMetrics, CachePersistenceError> {
        if self.worker_is_none() {
            return shutdown_result(&self.shutdown_completion)
                .unwrap_or_else(|| Ok(self.metrics.snapshot()));
        }

        let Some(deadline) = Instant::now().checked_add(timeout) else {
            return Err(timeout_error("shutdown", timeout));
        };
        let should_send = {
            let mut lifecycle = lock_recover(&self.lifecycle);
            lifecycle.shutdown_requested = true;
            while lifecycle.active_reservations > 0 {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(timeout_error("shutdown", timeout));
                }
                let (next, wait) = self
                    .lifecycle_changed
                    .wait_timeout(lifecycle, remaining)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                lifecycle = next;
                if wait.timed_out() && lifecycle.active_reservations > 0 {
                    return Err(timeout_error("shutdown", timeout));
                }
            }
            if lifecycle.shutdown_command_sent {
                false
            } else {
                lifecycle.shutdown_command_sent = true;
                true
            }
        };
        if should_send {
            if let Err(error) = self.send_before_deadline(
                "shutdown",
                timeout,
                deadline,
                PersistenceCommand::Shutdown,
            ) {
                lock_recover(&self.lifecycle).shutdown_command_sent = false;
                return Err(error);
            }
        }
        let result = wait_for_shutdown(
            &self.shutdown_completion,
            deadline.saturating_duration_since(Instant::now()),
            timeout,
        );
        if result.is_err() && self.worker_is_finished() {
            self.join_worker()?;
        }
        let result = result?;
        self.join_worker()?;
        result
    }

    fn worker_is_none(&self) -> bool {
        lock_recover(&self.lifecycle).worker.is_none()
    }

    fn worker_is_finished(&self) -> bool {
        lock_recover(&self.lifecycle)
            .worker
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
    }

    fn join_worker(&self) -> Result<(), CachePersistenceError> {
        let worker = {
            let mut lifecycle = lock_recover(&self.lifecycle);
            lifecycle.sender.take();
            lifecycle.worker.take()
        };
        if let Some(worker) = worker {
            worker
                .join()
                .map_err(|_| CachePersistenceError::WorkerPanicked)?;
        }
        Ok(())
    }

    fn shutdown_on_drop(&self) {
        let mut lifecycle = lock_recover(&self.lifecycle);
        if lifecycle.worker.is_none() {
            return;
        }
        lifecycle.shutdown_requested = true;
        if !lifecycle.shutdown_command_sent {
            if let Some(sender) = lifecycle.sender.as_ref() {
                if sender.try_send(PersistenceCommand::Shutdown).is_ok() {
                    lifecycle.shutdown_command_sent = true;
                }
            }
        }
        lifecycle.sender.take();
        // Dropping a JoinHandle detaches the worker. Destruction must never
        // defeat an explicit bounded shutdown deadline when filesystem
        // coordination remains blocked.
        drop(lifecycle.worker.take());
    }
}

impl Drop for CacheMutationReservation {
    fn drop(&mut self) {
        self.owner.release_pending_slots(self.reserved_slots);
        self.owner.finish_reservation();
    }
}

impl Drop for CachePersistence {
    fn drop(&mut self) {
        let previous = self.owner.handle_count.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
    }
}

impl Drop for CachePersistenceOwner {
    fn drop(&mut self) {
        let mut registry = lock_recover(cache_persistence_registry());
        if registry_entry_matches(&registry, &self.root_key, self) {
            registry.remove(&self.root_key);
        }
        self.shutdown_on_drop();
    }
}

struct PersistenceWorker {
    config: PersistConfig,
    cache: PacketCache,
    receiver: Receiver<PersistenceCommand>,
    metrics: Arc<SharedMetrics>,
    last_error: Arc<Mutex<Option<CachePersistenceError>>>,
    dirty: Arc<Mutex<BTreeMap<String, PendingDelta>>>,
    pending_slots: Arc<AtomicUsize>,
    shutdown_completion: ShutdownCompletion,
    root_lock: RootPersistenceLock,
    observed_generation: u64,
    persisted_revisions: HashMap<String, u64>,
    persisted_since_checkpoint: u64,
    wal_bytes_since_checkpoint: u64,
}

impl PersistenceWorker {
    fn run(mut self) {
        loop {
            let received = if self.dirty_is_empty() {
                self.receiver
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected)
            } else {
                self.receiver.recv_timeout(PERSISTENCE_DEBOUNCE)
            };

            match received {
                Ok(PersistenceCommand::Wake) => {}
                Ok(PersistenceCommand::Flush(reply)) => {
                    let result = self.flush_dirty();
                    if let Err(error) = result.as_ref() {
                        self.record_error(error.clone());
                    }
                    let result = result.map(|()| {
                        self.metrics.flushes.fetch_add(1, Ordering::Relaxed);
                        self.metrics.snapshot()
                    });
                    let _ = reply.send(result);
                }
                Ok(PersistenceCommand::Shutdown) => {
                    let result = self.flush_and_checkpoint();
                    complete_shutdown(&self.shutdown_completion, result);
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Err(error) = self.flush_dirty() {
                        self.record_error(error);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let result = self.flush_and_checkpoint();
                    complete_shutdown(&self.shutdown_completion, result);
                    break;
                }
            }
        }
    }

    fn dirty_is_empty(&self) -> bool {
        lock_recover(&self.dirty).is_empty()
    }

    fn flush_dirty(&mut self) -> Result<(), CachePersistenceError> {
        let taken_deltas = {
            let mut dirty = lock_recover(&self.dirty);
            std::mem::take(&mut *dirty)
                .into_values()
                .collect::<Vec<_>>()
        };
        let (pending_deltas, superseded): (Vec<_>, Vec<_>) =
            taken_deltas.into_iter().partition(|pending| {
                self.persisted_revisions
                    .get(pending.delta.cache_key())
                    .is_none_or(|persisted| *persisted < pending.revision)
            });
        self.release_pending_slots(superseded.len());
        if pending_deltas.is_empty() {
            return Ok(());
        }
        let deltas = pending_deltas
            .iter()
            .map(|pending| pending.delta.clone())
            .collect::<Vec<_>>();

        let persist_result = (|| {
            let mut locked_root = self.root_lock.lock()?;
            let durable_generation = locked_root.generation()?;
            if durable_generation != self.observed_generation {
                let latest = PacketCache::load_from_disk(&self.config);
                repair_torn_wal_tail(&self.config, &latest)?;
                self.cache = latest;
            }
            let next_generation = locked_root.advance_generation(durable_generation)?;
            self.metrics
                .coordination_bytes
                .fetch_add(std::mem::size_of::<u64>() as u64, Ordering::Relaxed);
            let sequence = self.cache.persisted_sequence.saturating_add(1);
            let wal_bytes = append_wal_record(&self.config, sequence, &deltas)
                .map_err(|source| io_error("WAL append", source))?;
            Ok((next_generation, sequence, wal_bytes))
        })();
        let (next_generation, sequence, wal_bytes) = match persist_result {
            Ok(result) => result,
            Err(error) => {
                self.restore_pending_deltas(pending_deltas);
                return Err(error);
            }
        };
        self.observed_generation = next_generation;
        let delta_count = deltas.len() as u64;
        for pending in &pending_deltas {
            self.persisted_revisions
                .entry(pending.delta.cache_key().to_string())
                .and_modify(|persisted| *persisted = (*persisted).max(pending.revision))
                .or_insert(pending.revision);
        }
        self.release_pending_slots(pending_deltas.len());
        self.cache.apply_persist_deltas(sequence, deltas);
        self.metrics
            .persisted_deltas
            .fetch_add(delta_count, Ordering::Relaxed);
        self.metrics.wal_records.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .wal_bytes
            .fetch_add(wal_bytes, Ordering::Relaxed);
        self.persisted_since_checkpoint =
            self.persisted_since_checkpoint.saturating_add(delta_count);
        self.wal_bytes_since_checkpoint = self.wal_bytes_since_checkpoint.saturating_add(wal_bytes);

        if self.persisted_since_checkpoint >= CHECKPOINT_DELTA_THRESHOLD
            || self.wal_bytes_since_checkpoint >= CHECKPOINT_WAL_BYTES_THRESHOLD
        {
            self.checkpoint()?;
        }
        Ok(())
    }

    fn restore_pending_deltas(&self, pending_deltas: Vec<PendingDelta>) {
        let mut dirty = lock_recover(&self.dirty);
        let mut superseded_slots = 0usize;
        for pending in pending_deltas {
            let cache_key = pending.delta.cache_key().to_string();
            match dirty.entry(cache_key) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(pending);
                }
                std::collections::btree_map::Entry::Occupied(mut slot)
                    if slot.get().revision <= pending.revision =>
                {
                    slot.insert(pending);
                    superseded_slots = superseded_slots.saturating_add(1);
                }
                std::collections::btree_map::Entry::Occupied(_) => {
                    superseded_slots = superseded_slots.saturating_add(1);
                }
            }
        }
        drop(dirty);
        self.release_pending_slots(superseded_slots);
    }

    fn release_pending_slots(&self, count: usize) {
        if count > 0 {
            let previous = self.pending_slots.fetch_sub(count, Ordering::AcqRel);
            debug_assert!(previous >= count);
        }
    }

    fn checkpoint(&mut self) -> Result<(), CachePersistenceError> {
        let mut locked_root = self.root_lock.lock()?;
        let durable_generation = locked_root.generation()?;
        if durable_generation != self.observed_generation {
            let latest = PacketCache::load_from_disk(&self.config);
            repair_torn_wal_tail(&self.config, &latest)?;
            self.cache = latest;
        }
        let next_generation = locked_root.advance_generation(durable_generation)?;
        self.metrics
            .coordination_bytes
            .fetch_add(std::mem::size_of::<u64>() as u64, Ordering::Relaxed);
        let checkpoint_bytes = self
            .cache
            .write_checkpoint(&self.config)
            .map_err(|source| io_error("checkpoint write", source))?;
        reset_wal(&self.config).map_err(|source| io_error("WAL reset", source))?;
        self.observed_generation = next_generation;
        self.metrics.checkpoints.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .checkpoint_bytes
            .fetch_add(checkpoint_bytes, Ordering::Relaxed);
        self.persisted_since_checkpoint = 0;
        self.wal_bytes_since_checkpoint = 0;
        Ok(())
    }

    fn flush_and_checkpoint(&mut self) -> Result<CachePersistenceMetrics, CachePersistenceError> {
        let result = self.flush_dirty().and_then(|()| {
            if self.persisted_since_checkpoint == 0 && self.wal_bytes_since_checkpoint == 0 {
                Ok(())
            } else {
                self.checkpoint()
            }
        });
        match result {
            Ok(()) => {
                self.metrics.flushes.fetch_add(1, Ordering::Relaxed);
                Ok(self.metrics.snapshot())
            }
            Err(error) => {
                self.record_error(error.clone());
                Err(error)
            }
        }
    }

    fn record_error(&self, error: CachePersistenceError) {
        self.metrics.failures.fetch_add(1, Ordering::Relaxed);
        *lock_recover(&self.last_error) = Some(error);
    }
}

fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn io_error(operation: &'static str, source: std::io::Error) -> CachePersistenceError {
    CachePersistenceError::Io {
        operation,
        detail: source.to_string(),
    }
}

fn repair_torn_wal_tail(
    config: &PersistConfig,
    cache: &PacketCache,
) -> Result<bool, CachePersistenceError> {
    let mut verification_cache = cache.clone();
    let replay = replay_wal(&mut verification_cache, config)
        .map_err(|source| io_error("WAL recovery", source))?;
    if replay.baseline_mismatch {
        return Err(CachePersistenceError::Io {
            operation: "WAL recovery",
            detail: format!(
                "WAL sequence does not continue checkpoint sequence {}; refusing destructive repair",
                cache.persisted_sequence
            ),
        });
    }
    if replay.recovered_corruption {
        if !cache.has_v3_checkpoint_baseline {
            return Err(CachePersistenceError::Io {
                operation: "WAL recovery",
                detail:
                    "WAL tail is corrupt without a trusted V3 checkpoint; refusing destructive repair"
                        .to_string(),
            });
        }
        truncate_wal_to(config, replay.valid_bytes)
            .map_err(|source| io_error("WAL recovery", source))?;
        return Ok(true);
    }
    Ok(false)
}

fn complete_shutdown(
    completion: &ShutdownCompletion,
    result: Result<CachePersistenceMetrics, CachePersistenceError>,
) {
    let (state, wake) = &**completion;
    *lock_recover(state) = Some(result);
    wake.notify_all();
}

fn shutdown_result(
    completion: &ShutdownCompletion,
) -> Option<Result<CachePersistenceMetrics, CachePersistenceError>> {
    let (state, _) = &**completion;
    lock_recover(state).clone()
}

fn wait_for_shutdown(
    completion: &ShutdownCompletion,
    wait: Duration,
    requested_timeout: Duration,
) -> Result<Result<CachePersistenceMetrics, CachePersistenceError>, CachePersistenceError> {
    let (state, wake) = &**completion;
    let state = lock_recover(state);
    let (state, _) = wake
        .wait_timeout_while(state, wait, |result| result.is_none())
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state
        .clone()
        .ok_or_else(|| timeout_error("shutdown", requested_timeout))
}

fn timeout_error(operation: &'static str, timeout: Duration) -> CachePersistenceError {
    CachePersistenceError::Timeout {
        operation,
        timeout_ms: timeout.as_millis().try_into().unwrap_or(u64::MAX),
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::process::Command;
    use std::sync::{Arc, Mutex};

    use serde_json::{json, Value};
    use tempfile::tempdir;

    use super::*;
    use crate::persist::{
        append_wal_record, persist_cache_path_v3, persist_cache_wal_path_v3, PersistDelta,
    };
    use crate::{CachePacket, NoopDeltaReuseHooks};

    fn put_entry(cache: &mut PacketCache, id: usize, payload_bytes: usize) -> PacketCacheEntry {
        let target = format!("demo.reducer.{id}");
        let mut hooks = NoopDeltaReuseHooks;
        let lookup = cache.lookup_with_hooks(&target, &json!({"id": id}), &mut hooks);
        cache.put_with_hooks(
            &target,
            &lookup,
            vec![CachePacket {
                packet_id: Some(format!("packet-{id}")),
                body: json!({"payload": "x".repeat(payload_bytes)}),
                ..CachePacket::default()
            }],
            Value::Null,
            &mut hooks,
        )
    }

    fn wait_for_path(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {}",
                path.display()
            );
            thread::yield_now();
        }
    }

    fn test_persistence_handle(
        sender: SyncSender<PersistenceCommand>,
        worker: Option<JoinHandle<()>>,
        metrics: Arc<SharedMetrics>,
        dirty: Arc<Mutex<BTreeMap<String, PendingDelta>>>,
        shutdown_completion: ShutdownCompletion,
    ) -> CachePersistence {
        CachePersistence {
            owner: Arc::new(CachePersistenceOwner {
                root_key: PathBuf::from("__packet28_test_persistence_owner__"),
                ttl_secs: crate::DEFAULT_PERSIST_TTL_SECS,
                handle_count: AtomicUsize::new(1),
                memory: Arc::new(Mutex::new(PacketCache::new())),
                lifecycle: Mutex::new(PersistenceLifecycle {
                    sender: Some(sender),
                    worker,
                    shutdown_requested: false,
                    shutdown_command_sent: false,
                    active_reservations: 0,
                }),
                lifecycle_changed: Condvar::new(),
                metrics,
                last_error: Arc::new(Mutex::new(None)),
                dirty,
                pending_slots: Arc::new(AtomicUsize::new(0)),
                next_revision: AtomicU64::new(0),
                shutdown_completion,
            }),
        }
    }

    #[test]
    fn flush_makes_wal_delta_visible_before_checkpoint() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let mut cache = PacketCache::load_from_disk(&config);
        let owner = CachePersistence::start(config.clone(), cache.clone()).unwrap();
        let entry = put_entry(&mut cache, 1, 128);

        owner.record_update(&entry, Vec::new()).unwrap();
        let metrics = owner.flush(Duration::from_secs(2)).unwrap();
        let loaded = PacketCache::load_from_disk(&config);

        assert!(loaded.get(&entry.cache_key).is_some());
        assert_eq!(metrics.persisted_deltas, 1);
        assert_eq!(metrics.wal_records, 1);
        assert_eq!(metrics.checkpoints, 0);
    }

    #[test]
    fn replay_keeps_valid_prefix_and_truncates_torn_tail() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        PacketCache::new().save_to_disk(&config).unwrap();
        let mut cache = PacketCache::new();
        let entry = put_entry(&mut cache, 2, 128);
        let valid_bytes = append_wal_record(&config, 1, &[PersistDelta::upsert(&entry)]).unwrap();
        let wal_path = persist_cache_wal_path_v3(dir.path());
        OpenOptions::new()
            .append(true)
            .open(&wal_path)
            .unwrap()
            .write_all(b"P28CWAL1\x10\x00")
            .unwrap();

        let loaded = PacketCache::load_from_disk(&config);

        assert!(loaded.get(&entry.cache_key).is_some());
        assert_eq!(loaded.stats().evictions.corrupt_load_recovery, 1);
        assert!(fs::metadata(&wal_path).unwrap().len() > valid_bytes);

        let owner = CachePersistence::start(config, loaded).unwrap();
        assert_eq!(fs::metadata(wal_path).unwrap().len(), valid_bytes);
        drop(owner);
    }

    #[test]
    fn checkpoint_watermark_ignores_precheckpoint_wal_after_crash() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let mut cache = PacketCache::new();
        let entry = put_entry(&mut cache, 3, 128);
        append_wal_record(&config, 1, &[PersistDelta::upsert(&entry)]).unwrap();
        let wal_path = persist_cache_wal_path_v3(dir.path());
        let old_wal = fs::read(&wal_path).unwrap();

        let mut loaded = PacketCache::load_from_disk(&config);
        loaded.prune(crate::ContextStorePruneRequest {
            all: true,
            ttl_secs: None,
        });
        loaded.save_to_disk(&config).unwrap();
        fs::write(&wal_path, old_wal).unwrap();

        let recovered = PacketCache::load_from_disk(&config);
        assert!(recovered.is_empty());
    }

    #[test]
    fn crash_after_backup_before_primary_keeps_primary_wal_recovery_sound() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let mut checkpoint = PacketCache::new();
        let first = put_entry(&mut checkpoint, 3_100, 32);
        checkpoint.save_to_disk(&config).unwrap();

        let mut newer = checkpoint.clone();
        let second = put_entry(&mut newer, 3_101, 32);
        append_wal_record(&config, 1, &[PersistDelta::upsert(&second)]).unwrap();
        newer.persisted_sequence = 1;
        let error = newer
            .write_checkpoint_failing_after_backup(&config)
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);

        let recovered = PacketCache::load_from_disk(&config);
        assert!(recovered.get(&first.cache_key).is_some());
        assert!(recovered.get(&second.cache_key).is_some());
        assert_eq!(recovered.persisted_sequence, 1);
    }

    #[test]
    fn concurrent_mutations_persist_every_distinct_entry() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let cache = Arc::new(Mutex::new(PacketCache::load_from_disk(&config)));
        let owner = Arc::new(
            CachePersistence::start(config.clone(), cache.lock().unwrap().clone()).unwrap(),
        );
        let mut workers = Vec::new();
        for worker_id in 0..8 {
            let cache = cache.clone();
            let owner = owner.clone();
            workers.push(thread::spawn(move || {
                for offset in 0..16 {
                    let id = worker_id * 16 + offset;
                    let entry = {
                        let mut cache = cache.lock().unwrap();
                        put_entry(&mut cache, id, 64)
                    };
                    owner.record_update(&entry, Vec::new()).unwrap();
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }

        owner.flush(Duration::from_secs(5)).unwrap();
        let loaded = PacketCache::load_from_disk(&config);
        assert_eq!(loaded.len(), 128);
    }

    #[test]
    fn reversed_same_key_upserts_keep_the_newest_reserved_revision() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let owner = CachePersistence::open(config.clone()).unwrap();
        let memory = owner.shared_cache();
        let (older, older_reservation, newer, newer_reservation) = {
            let mut cache = lock_recover(&memory);
            let older_reservation = owner.reserve_mutation(1).unwrap();
            let older = put_entry(&mut cache, 30_000, 1);
            let newer_reservation = owner.reserve_mutation(1).unwrap();
            let newer = put_entry(&mut cache, 30_000, 2);
            (older, older_reservation, newer, newer_reservation)
        };

        owner
            .record_update_reserved(&newer, Vec::new(), newer_reservation)
            .unwrap();
        owner
            .record_update_reserved(&older, Vec::new(), older_reservation)
            .unwrap();
        owner.shutdown(Duration::from_secs(2)).unwrap();

        let loaded = PacketCache::load_from_disk(&config);
        assert_eq!(
            loaded
                .get(&newer.cache_key)
                .unwrap()
                .packets
                .first()
                .and_then(|packet| packet.body.get("payload"))
                .and_then(Value::as_str),
            Some("xx")
        );
    }

    #[test]
    fn reversed_tombstone_cannot_delete_a_newer_upsert() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let owner = CachePersistence::open(config.clone()).unwrap();
        let memory = owner.shared_cache();
        let (entry, tombstone, upsert) = {
            let mut cache = lock_recover(&memory);
            let tombstone = owner.reserve_mutation(1).unwrap();
            let upsert = owner.reserve_mutation(1).unwrap();
            let entry = put_entry(&mut cache, 31_000, 16);
            (entry, tombstone, upsert)
        };

        owner
            .record_update_reserved(&entry, Vec::new(), upsert)
            .unwrap();
        owner
            .record_removals_reserved(vec![entry.cache_key.clone()], tombstone)
            .unwrap();
        owner.shutdown(Duration::from_secs(2)).unwrap();

        let loaded = PacketCache::load_from_disk(&config);
        assert!(loaded.get(&entry.cache_key).is_some());
    }

    #[test]
    fn same_root_handles_share_one_live_cache_and_owner() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let first = CachePersistence::open(config.clone()).unwrap();
        let second = CachePersistence::open(config.clone()).unwrap();
        let first_memory = first.shared_cache();
        let second_memory = second.shared_cache();
        assert!(Arc::ptr_eq(&first_memory, &second_memory));

        let (entry, reservation) = {
            let mut cache = lock_recover(&first_memory);
            let reservation = first.reserve_mutation(1).unwrap();
            let entry = put_entry(&mut cache, 32_000, 16);
            (entry, reservation)
        };
        first
            .record_update_reserved(&entry, Vec::new(), reservation)
            .unwrap();

        assert!(lock_recover(&second_memory).get(&entry.cache_key).is_some());
        second.flush(Duration::from_secs(2)).unwrap();
        drop(second);
        first.shutdown(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn bounded_admission_rejects_without_dropping_accepted_deltas() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let owner = CachePersistence::open(config.clone()).unwrap();
        let lock_path = config
            .root_dir
            .join(crate::PERSIST_CACHE_DIR)
            .join(PERSISTENCE_COORDINATION_FILE);
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .unwrap();
        FileExt::lock_exclusive(&lock_file).unwrap();
        let mut cache = PacketCache::new();

        for id in 0..MAX_DIRTY_DELTAS {
            let entry = put_entry(&mut cache, 40_000 + id, 8);
            owner.record_update(&entry, Vec::new()).unwrap();
        }
        for id in 0..16 {
            let rejected = put_entry(&mut cache, 50_000 + id, 8);
            assert!(matches!(
                owner.record_update(&rejected, Vec::new()),
                Err(CachePersistenceError::Backpressure {
                    capacity: MAX_DIRTY_DELTAS,
                    ..
                })
            ));
        }

        let metrics = owner.metrics();
        assert_eq!(
            owner.owner.pending_slots.load(Ordering::Acquire),
            MAX_DIRTY_DELTAS
        );
        assert_eq!(metrics.max_pending_deltas, MAX_DIRTY_DELTAS as u64);
        assert_eq!(metrics.backpressure_events, 16);
        assert_eq!(metrics.rejected_batches, 16);
        FileExt::unlock(&lock_file).unwrap();
        owner.shutdown(Duration::from_secs(10)).unwrap();

        assert_eq!(PacketCache::load_from_disk(&config).len(), MAX_DIRTY_DELTAS);
    }

    #[test]
    fn two_process_checkpoint_and_append_preserve_all_deltas() {
        const CHILD_ROOT: &str = "PACKET28_PER02_CHILD_ROOT";
        const CHILD_ROLE: &str = "PACKET28_PER02_CHILD_ROLE";
        if let (Ok(root), Ok(role)) = (std::env::var(CHILD_ROOT), std::env::var(CHILD_ROLE)) {
            let root = PathBuf::from(root);
            let config = PersistConfig::new(root.clone());
            let owner = CachePersistence::open(config).unwrap();
            fs::write(root.join(format!("opened-{role}")), b"ready").unwrap();
            wait_for_path(&root.join("mutate"));
            let memory = owner.shared_cache();
            let (entry, reservation) = {
                let mut cache = lock_recover(&memory);
                let reservation = owner.reserve_mutation(1).unwrap();
                let id = if role == "a" { 70_001 } else { 70_002 };
                let entry = put_entry(&mut cache, id, 32);
                (entry, reservation)
            };
            owner
                .record_update_reserved(&entry, Vec::new(), reservation)
                .unwrap();
            fs::write(root.join(format!("queued-{role}")), b"ready").unwrap();
            wait_for_path(&root.join("finish"));
            owner.shutdown(Duration::from_secs(10)).unwrap();
            return;
        }

        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let config = PersistConfig::new(root.clone());
        PacketCache::new().save_to_disk(&config).unwrap();
        let current_exe = std::env::current_exe().unwrap();
        let test_name =
            "persistence_owner::tests::two_process_checkpoint_and_append_preserve_all_deltas";
        let mut children = ["a", "b"].map(|role| {
            Command::new(&current_exe)
                .arg("--exact")
                .arg(test_name)
                .arg("--nocapture")
                .env(CHILD_ROOT, &root)
                .env(CHILD_ROLE, role)
                .spawn()
                .unwrap()
        });
        wait_for_path(&root.join("opened-a"));
        wait_for_path(&root.join("opened-b"));

        let lock_path = root
            .join(crate::PERSIST_CACHE_DIR)
            .join(PERSISTENCE_COORDINATION_FILE);
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .unwrap();
        FileExt::lock_exclusive(&lock_file).unwrap();
        fs::write(root.join("mutate"), b"go").unwrap();
        wait_for_path(&root.join("queued-a"));
        wait_for_path(&root.join("queued-b"));
        fs::write(root.join("finish"), b"go").unwrap();
        FileExt::unlock(&lock_file).unwrap();

        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        assert_eq!(PacketCache::load_from_disk(&config).len(), 2);
    }

    #[test]
    fn two_process_prune_and_write_never_resurrect_pruned_data() {
        const CHILD_ROOT: &str = "PACKET28_PER02_PRUNE_CHILD_ROOT";
        const CHILD_ROLE: &str = "PACKET28_PER02_PRUNE_CHILD_ROLE";
        if let (Ok(root), Ok(role)) = (std::env::var(CHILD_ROOT), std::env::var(CHILD_ROLE)) {
            let root = PathBuf::from(root);
            let config = PersistConfig::new(root.clone());
            let owner = CachePersistence::open(config).unwrap();
            fs::write(root.join(format!("prune-opened-{role}")), b"ready").unwrap();
            wait_for_path(&root.join("prune-mutate"));
            let memory = owner.shared_cache();
            if role == "writer" {
                let (entry, reservation) = {
                    let mut cache = lock_recover(&memory);
                    let reservation = owner.reserve_mutation(1).unwrap();
                    let entry = put_entry(&mut cache, 71_001, 32);
                    (entry, reservation)
                };
                owner
                    .record_update_reserved(&entry, Vec::new(), reservation)
                    .unwrap();
            } else {
                let (removed, reservation) = {
                    let mut cache = lock_recover(&memory);
                    let request = crate::ContextStorePruneRequest {
                        all: true,
                        ttl_secs: None,
                    };
                    let removed = cache.prune_candidate_keys(&request);
                    let reservation = owner.reserve_mutation(removed.len()).unwrap();
                    let report = cache.prune(request);
                    assert_eq!(report.removed, removed.len());
                    (removed, reservation)
                };
                owner
                    .record_removals_reserved(removed, reservation)
                    .unwrap();
            }
            fs::write(root.join(format!("prune-queued-{role}")), b"ready").unwrap();
            wait_for_path(&root.join("prune-finish"));
            owner.shutdown(Duration::from_secs(10)).unwrap();
            return;
        }

        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let config = PersistConfig::new(root.clone());
        let mut initial = PacketCache::new();
        let old = put_entry(&mut initial, 71_000, 32);
        initial.save_to_disk(&config).unwrap();
        let current_exe = std::env::current_exe().unwrap();
        let test_name =
            "persistence_owner::tests::two_process_prune_and_write_never_resurrect_pruned_data";
        let mut children = ["writer", "pruner"].map(|role| {
            Command::new(&current_exe)
                .arg("--exact")
                .arg(test_name)
                .arg("--nocapture")
                .env(CHILD_ROOT, &root)
                .env(CHILD_ROLE, role)
                .spawn()
                .unwrap()
        });
        wait_for_path(&root.join("prune-opened-writer"));
        wait_for_path(&root.join("prune-opened-pruner"));

        let lock_path = root
            .join(crate::PERSIST_CACHE_DIR)
            .join(PERSISTENCE_COORDINATION_FILE);
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .unwrap();
        FileExt::lock_exclusive(&lock_file).unwrap();
        fs::write(root.join("prune-mutate"), b"go").unwrap();
        wait_for_path(&root.join("prune-queued-writer"));
        wait_for_path(&root.join("prune-queued-pruner"));
        fs::write(root.join("prune-finish"), b"go").unwrap();
        FileExt::unlock(&lock_file).unwrap();

        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        let loaded = PacketCache::load_from_disk(&config);
        assert_eq!(loaded.len(), 1);
        assert!(loaded.get(&old.cache_key).is_none());
        assert_eq!(
            loaded.entries().first().map(|entry| entry.target.as_str()),
            Some("demo.reducer.71001")
        );
    }

    #[test]
    fn shutdown_waits_for_reserved_mutation_without_accepting_new_work() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let owner = CachePersistence::open(config.clone()).unwrap();
        let memory = owner.shared_cache();
        let (entry, reservation) = {
            let mut cache = lock_recover(&memory);
            let reservation = owner.reserve_mutation(1).unwrap();
            let entry = put_entry(&mut cache, 60_000, 8);
            (entry, reservation)
        };
        let shutdown_owner = owner.owner.clone();
        let shutdown = thread::spawn(move || {
            shutdown_owner
                .shutdown_inner(Duration::from_millis(20))
                .unwrap_err()
        });
        let error = shutdown.join().unwrap();
        assert!(matches!(
            error,
            CachePersistenceError::Timeout {
                operation: "shutdown",
                ..
            }
        ));
        assert!(matches!(
            owner.reserve_mutation(1),
            Err(CachePersistenceError::WorkerUnavailable)
        ));

        owner
            .record_update_reserved(&entry, Vec::new(), reservation)
            .unwrap();
        owner.shutdown(Duration::from_secs(2)).unwrap();
        assert!(PacketCache::load_from_disk(&config)
            .get(&entry.cache_key)
            .is_some());
    }

    #[test]
    fn timed_out_shutdown_drop_never_joins_blocked_worker() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let owner = CachePersistence::open(config.clone()).unwrap();
        let lock_path = config
            .root_dir
            .join(crate::PERSIST_CACHE_DIR)
            .join(PERSISTENCE_COORDINATION_FILE);
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .unwrap();
        FileExt::lock_exclusive(&lock_file).unwrap();
        let memory = owner.shared_cache();
        let (entry, reservation) = {
            let mut cache = lock_recover(&memory);
            let reservation = owner.reserve_mutation(1).unwrap();
            let entry = put_entry(&mut cache, 61_000, 8);
            (entry, reservation)
        };
        owner
            .record_update_reserved(&entry, Vec::new(), reservation)
            .unwrap();
        assert!(matches!(
            owner.shutdown(Duration::from_millis(10)),
            Err(CachePersistenceError::Timeout {
                operation: "shutdown",
                ..
            })
        ));

        let drop_started = Instant::now();
        drop(owner);
        assert!(drop_started.elapsed() < Duration::from_millis(100));
        FileExt::unlock(&lock_file).unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while PacketCache::load_from_disk(&config)
            .get(&entry.cache_key)
            .is_none()
        {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
    }

    #[test]
    fn one_dirty_entry_writes_less_than_a_full_checkpoint() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let mut cache = PacketCache::new();
        for id in 0..128 {
            put_entry(&mut cache, id, 512);
        }
        cache.save_to_disk(&config).unwrap();
        let checkpoint_bytes = fs::metadata(persist_cache_path_v3(dir.path()))
            .unwrap()
            .len();
        let owner = CachePersistence::start(config.clone(), cache.clone()).unwrap();
        let entry = put_entry(&mut cache, 10_000, 512);

        owner.record_update(&entry, Vec::new()).unwrap();
        let metrics = owner.flush(Duration::from_secs(2)).unwrap();

        assert!(metrics.wal_bytes < checkpoint_bytes);
    }

    #[test]
    fn full_wake_queue_keeps_dirty_update_without_blocking() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(PersistenceCommand::Wake).unwrap();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_release = release.clone();
        let worker = thread::spawn(move || {
            let (released, wake) = &*worker_release;
            let released = lock_recover(released);
            drop(
                wake.wait_while(released, |released| !*released)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        });
        let metrics = Arc::new(SharedMetrics::default());
        let dirty = Arc::new(Mutex::new(BTreeMap::new()));
        let owner = test_persistence_handle(
            sender,
            Some(worker),
            metrics.clone(),
            dirty.clone(),
            Arc::new((Mutex::new(None), Condvar::new())),
        );
        let mut cache = PacketCache::new();
        let entry = put_entry(&mut cache, 20_000, 32);

        owner.record_update(&entry, Vec::new()).unwrap();

        assert_eq!(metrics.backpressure_events.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.wake_coalesces.load(Ordering::Relaxed), 1);
        assert!(lock_recover(&dirty).contains_key(&entry.cache_key));
        let worker = {
            let mut lifecycle = lock_recover(&owner.owner.lifecycle);
            lifecycle.sender.take();
            lifecycle.worker.take().unwrap()
        };
        drop(receiver);
        let (released, wake) = &*release;
        *lock_recover(released) = true;
        wake.notify_all();
        worker.join().unwrap();
    }

    #[test]
    fn shutdown_timeout_retains_worker_for_bounded_retry() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let completion = Arc::new((Mutex::new(None), Condvar::new()));
        let worker_completion = completion.clone();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_release = release.clone();
        let worker = thread::spawn(move || {
            assert!(matches!(
                receiver.recv().unwrap(),
                PersistenceCommand::Shutdown
            ));
            let (released, wake) = &*worker_release;
            let released = lock_recover(released);
            let _released = wake
                .wait_while(released, |released| !*released)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            complete_shutdown(&worker_completion, Ok(CachePersistenceMetrics::default()));
        });
        let owner = test_persistence_handle(
            sender,
            Some(worker),
            Arc::new(SharedMetrics::default()),
            Arc::new(Mutex::new(BTreeMap::new())),
            completion,
        );

        let error = owner.shutdown(Duration::from_millis(1)).unwrap_err();

        assert!(matches!(
            error,
            CachePersistenceError::Timeout {
                operation: "shutdown",
                ..
            }
        ));
        {
            let lifecycle = lock_recover(&owner.owner.lifecycle);
            assert!(lifecycle.worker.is_some());
            assert!(lifecycle.sender.is_some());
        }

        let (released, wake) = &*release;
        *lock_recover(released) = true;
        wake.notify_all();
        owner.shutdown(Duration::from_secs(2)).unwrap();
        assert!(lock_recover(&owner.owner.lifecycle).worker.is_none());
    }

    #[test]
    fn flush_reports_filesystem_failure_without_panicking() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        let config = PersistConfig::new(root.clone());
        let mut cache = PacketCache::new();
        let owner = CachePersistence::start(config, cache.clone()).unwrap();
        fs::create_dir(persist_cache_wal_path_v3(&root)).unwrap();
        let entry = put_entry(&mut cache, 4, 32);

        owner.record_update(&entry, Vec::new()).unwrap();
        let error = owner.flush(Duration::from_secs(2)).unwrap_err();

        assert!(matches!(
            error,
            CachePersistenceError::Io {
                operation: "WAL append",
                ..
            }
        ));
    }

    #[test]
    fn shutdown_checkpoints_and_resets_wal() {
        let dir = tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let mut cache = PacketCache::new();
        let owner = CachePersistence::start(config.clone(), cache.clone()).unwrap();
        let entry = put_entry(&mut cache, 5, 64);
        owner.record_update(&entry, Vec::new()).unwrap();

        let metrics = owner.shutdown(Duration::from_secs(2)).unwrap();

        assert_eq!(metrics.checkpoints, 1);
        assert!(persist_cache_path_v3(dir.path()).exists());
        assert_eq!(
            fs::metadata(persist_cache_wal_path_v3(dir.path()))
                .unwrap()
                .len(),
            0
        );
    }
}
