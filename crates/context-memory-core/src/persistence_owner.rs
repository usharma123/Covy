use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::persist::{append_wal_record, replay_wal, reset_wal, truncate_wal_to, PersistDelta};
use crate::{PacketCache, PacketCacheEntry, PersistConfig};

const PERSISTENCE_QUEUE_CAPACITY: usize = 256;
const PERSISTENCE_DEBOUNCE: Duration = Duration::from_millis(20);
const CHECKPOINT_DELTA_THRESHOLD: u64 = 256;
const CHECKPOINT_WAL_BYTES_THRESHOLD: u64 = 4 * 1024 * 1024;

/// Failures reported by the context-cache persistence owner.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CachePersistenceError {
    /// The persistence thread could not be created.
    #[error("failed to start cache persistence worker: {detail}")]
    Start { detail: String },

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
    pub persisted_deltas: u64,
    pub wal_records: u64,
    pub wal_bytes: u64,
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
    persisted_deltas: AtomicU64,
    wal_records: AtomicU64,
    wal_bytes: AtomicU64,
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
            persisted_deltas: self.persisted_deltas.load(Ordering::Relaxed),
            wal_records: self.wal_records.load(Ordering::Relaxed),
            wal_bytes: self.wal_bytes.load(Ordering::Relaxed),
            checkpoints: self.checkpoints.load(Ordering::Relaxed),
            checkpoint_bytes: self.checkpoint_bytes.load(Ordering::Relaxed),
            flushes: self.flushes.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
        }
    }
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
    sender: Option<SyncSender<PersistenceCommand>>,
    worker: Option<JoinHandle<()>>,
    metrics: Arc<SharedMetrics>,
    last_error: Arc<Mutex<Option<CachePersistenceError>>>,
    dirty: Arc<Mutex<BTreeMap<String, PersistDelta>>>,
    shutdown_completion: ShutdownCompletion,
    shutdown_requested: bool,
}

impl CachePersistence {
    /// Starts an owner with a private persistence snapshot.
    ///
    /// The caller should pass the cache returned by
    /// [`PacketCache::load_from_disk`], which already includes replayed WAL
    /// deltas.
    pub fn start(config: PersistConfig, cache: PacketCache) -> Result<Self, CachePersistenceError> {
        repair_torn_wal_tail(&config, &cache)?;
        let (sender, receiver) = mpsc::sync_channel(PERSISTENCE_QUEUE_CAPACITY);
        let metrics = Arc::new(SharedMetrics::default());
        let last_error = Arc::new(Mutex::new(None));
        let dirty = Arc::new(Mutex::new(BTreeMap::new()));
        let shutdown_completion = Arc::new((Mutex::new(None), Condvar::new()));
        let worker_metrics = metrics.clone();
        let worker_last_error = last_error.clone();
        let worker_dirty = dirty.clone();
        let worker_shutdown_completion = shutdown_completion.clone();
        let worker = thread::Builder::new()
            .name("packet28-cache-persistence".to_string())
            .spawn(move || {
                PersistenceWorker::new(
                    config,
                    cache,
                    receiver,
                    worker_metrics,
                    worker_last_error,
                    worker_dirty,
                    worker_shutdown_completion,
                )
                .run();
            })
            .map_err(|source| CachePersistenceError::Start {
                detail: source.to_string(),
            })?;

        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            metrics,
            last_error,
            dirty,
            shutdown_completion,
            shutdown_requested: false,
        })
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
        let mut deltas = Vec::with_capacity(removed_cache_keys.len().saturating_add(1));
        deltas.push(PersistDelta::upsert(entry));
        deltas.extend(removed_cache_keys.into_iter().map(PersistDelta::remove));
        self.metrics
            .enqueued_batches
            .fetch_add(1, Ordering::Relaxed);
        self.metrics
            .enqueued_deltas
            .fetch_add(deltas.len() as u64, Ordering::Relaxed);
        {
            let mut dirty = lock_recover(&self.dirty);
            for delta in deltas {
                dirty.insert(delta.cache_key().to_string(), delta);
            }
        }
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
        self.metrics.snapshot()
    }

    /// Returns the most recent background persistence error, if any.
    pub fn last_error(&self) -> Option<CachePersistenceError> {
        lock_recover(&self.last_error).clone()
    }

    /// Flushes, checkpoints, and joins the owner within the supplied timeout.
    pub fn shutdown(
        &mut self,
        timeout: Duration,
    ) -> Result<CachePersistenceMetrics, CachePersistenceError> {
        self.shutdown_inner(timeout)
    }

    fn wake_worker(&self) -> Result<(), CachePersistenceError> {
        let Some(sender) = self.sender.as_ref() else {
            return Err(CachePersistenceError::WorkerUnavailable);
        };
        match sender.try_send(PersistenceCommand::Wake) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.metrics
                    .backpressure_events
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
        self.send_before_deadline(operation, timeout, deadline, command(reply_sender))?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        reply_receiver.recv_timeout(remaining).map_err(|source| {
            if matches!(source, RecvTimeoutError::Timeout) {
                timeout_error(operation, timeout)
            } else {
                CachePersistenceError::WorkerUnavailable
            }
        })?
    }

    fn send_before_deadline(
        &self,
        operation: &'static str,
        timeout: Duration,
        deadline: Instant,
        mut command: PersistenceCommand,
    ) -> Result<(), CachePersistenceError> {
        let Some(sender) = self.sender.as_ref() else {
            return Err(CachePersistenceError::WorkerUnavailable);
        };
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
        &mut self,
        timeout: Duration,
    ) -> Result<CachePersistenceMetrics, CachePersistenceError> {
        if self.worker.is_none() {
            return shutdown_result(&self.shutdown_completion)
                .unwrap_or_else(|| Ok(self.metrics.snapshot()));
        }

        let Some(deadline) = Instant::now().checked_add(timeout) else {
            return Err(timeout_error("shutdown", timeout));
        };
        if !self.shutdown_requested {
            self.send_before_deadline("shutdown", timeout, deadline, PersistenceCommand::Shutdown)?;
            self.shutdown_requested = true;
        }
        let result = wait_for_shutdown(
            &self.shutdown_completion,
            deadline.saturating_duration_since(Instant::now()),
            timeout,
        );
        if result.is_err() && self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            self.sender.take();
            if let Some(worker) = self.worker.take() {
                worker
                    .join()
                    .map_err(|_| CachePersistenceError::WorkerPanicked)?;
            }
        }
        let result = result?;
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| CachePersistenceError::WorkerPanicked)?;
        }
        result
    }
}

impl Drop for CachePersistence {
    fn drop(&mut self) {
        if self.worker.is_none() {
            return;
        }
        if !self.shutdown_requested {
            if let Some(sender) = self.sender.as_ref() {
                if sender.try_send(PersistenceCommand::Shutdown).is_ok() {
                    self.shutdown_requested = true;
                }
            }
        }
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct PersistenceWorker {
    config: PersistConfig,
    cache: PacketCache,
    receiver: Receiver<PersistenceCommand>,
    metrics: Arc<SharedMetrics>,
    last_error: Arc<Mutex<Option<CachePersistenceError>>>,
    dirty: Arc<Mutex<BTreeMap<String, PersistDelta>>>,
    shutdown_completion: ShutdownCompletion,
    persisted_since_checkpoint: u64,
    wal_bytes_since_checkpoint: u64,
}

impl PersistenceWorker {
    fn new(
        config: PersistConfig,
        cache: PacketCache,
        receiver: Receiver<PersistenceCommand>,
        metrics: Arc<SharedMetrics>,
        last_error: Arc<Mutex<Option<CachePersistenceError>>>,
        dirty: Arc<Mutex<BTreeMap<String, PersistDelta>>>,
        shutdown_completion: ShutdownCompletion,
    ) -> Self {
        Self {
            config,
            cache,
            receiver,
            metrics,
            last_error,
            dirty,
            shutdown_completion,
            persisted_since_checkpoint: 0,
            wal_bytes_since_checkpoint: 0,
        }
    }

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
        let deltas = {
            let mut dirty = lock_recover(&self.dirty);
            std::mem::take(&mut *dirty)
                .into_values()
                .collect::<Vec<_>>()
        };
        if deltas.is_empty() {
            return Ok(());
        }
        let sequence = self.cache.persisted_sequence.saturating_add(1);
        let wal_bytes = match append_wal_record(&self.config, sequence, &deltas) {
            Ok(bytes) => bytes,
            Err(source) => {
                let mut dirty = lock_recover(&self.dirty);
                for delta in deltas {
                    dirty.entry(delta.cache_key().to_string()).or_insert(delta);
                }
                return Err(io_error("WAL append", source));
            }
        };
        let delta_count = deltas.len() as u64;
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

    fn checkpoint(&mut self) -> Result<(), CachePersistenceError> {
        let checkpoint_bytes = self
            .cache
            .write_checkpoint(&self.config)
            .map_err(|source| io_error("checkpoint write", source))?;
        reset_wal(&self.config).map_err(|source| io_error("WAL reset", source))?;
        self.metrics.checkpoints.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .checkpoint_bytes
            .fetch_add(checkpoint_bytes, Ordering::Relaxed);
        self.persisted_since_checkpoint = 0;
        self.wal_bytes_since_checkpoint = 0;
        Ok(())
    }

    fn flush_and_checkpoint(&mut self) -> Result<CachePersistenceMetrics, CachePersistenceError> {
        let result = self.flush_dirty().and_then(|()| self.checkpoint());
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
) -> Result<(), CachePersistenceError> {
    let mut verification_cache = cache.clone();
    let replay = replay_wal(&mut verification_cache, config)
        .map_err(|source| io_error("WAL recovery", source))?;
    if replay.recovered_corruption {
        truncate_wal_to(config, replay.valid_bytes)
            .map_err(|source| io_error("WAL recovery", source))?;
    }
    Ok(())
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
        let metrics = Arc::new(SharedMetrics::default());
        let dirty = Arc::new(Mutex::new(BTreeMap::new()));
        let mut owner = CachePersistence {
            sender: Some(sender),
            worker: None,
            metrics: metrics.clone(),
            last_error: Arc::new(Mutex::new(None)),
            dirty: dirty.clone(),
            shutdown_completion: Arc::new((Mutex::new(None), Condvar::new())),
            shutdown_requested: false,
        };
        let mut cache = PacketCache::new();
        let entry = put_entry(&mut cache, 20_000, 32);

        owner.record_update(&entry, Vec::new()).unwrap();

        assert_eq!(metrics.backpressure_events.load(Ordering::Relaxed), 1);
        assert!(lock_recover(&dirty).contains_key(&entry.cache_key));
        owner.sender.take();
        drop(receiver);
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
        let mut owner = CachePersistence {
            sender: Some(sender),
            worker: Some(worker),
            metrics: Arc::new(SharedMetrics::default()),
            last_error: Arc::new(Mutex::new(None)),
            dirty: Arc::new(Mutex::new(BTreeMap::new())),
            shutdown_completion: completion,
            shutdown_requested: false,
        };

        let error = owner.shutdown(Duration::from_millis(1)).unwrap_err();

        assert!(matches!(
            error,
            CachePersistenceError::Timeout {
                operation: "shutdown",
                ..
            }
        ));
        assert!(owner.worker.is_some());
        assert!(owner.sender.is_some());

        let (released, wake) = &*release;
        *lock_recover(released) = true;
        wake.notify_all();
        owner.shutdown(Duration::from_secs(2)).unwrap();
        assert!(owner.worker.is_none());
    }

    #[test]
    fn flush_reports_filesystem_failure_without_panicking() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        let config = PersistConfig::new(root.clone());
        let mut cache = PacketCache::new();
        let owner = CachePersistence::start(config, cache.clone()).unwrap();
        fs::remove_dir(&root).unwrap();
        fs::write(&root, b"file").unwrap();
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
        let mut owner = CachePersistence::start(config.clone(), cache.clone()).unwrap();
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
