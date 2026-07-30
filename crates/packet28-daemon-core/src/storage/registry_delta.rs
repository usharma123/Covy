use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use packet28_daemon_protocol::task::{TaskRecord, TaskRegistry, WatchRegistration, WatchRegistry};
use packet28_state_fs::{FileAccess, StateDir, StateFile};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::*;

const REGISTRY_DELTA_WAL_FILE_NAME: &str = "task-watch-registry-delta-v1.wal";
const WAL_MAGIC: [u8; 8] = *b"P28RDW01";
const FRAME_MAGIC: [u8; 8] = *b"P28RDF01";
const FRAME_FOOTER_MAGIC: [u8; 8] = *b"P28RDE01";
const WAL_FORMAT_VERSION: u32 = 1;
const FRAME_FORMAT_VERSION: u32 = 1;
const WAL_HEADER_BYTES: usize = 56;
const FRAME_HEADER_BYTES: usize = 72;
const FRAME_FOOTER_BYTES: usize = 56;

#[cfg(test)]
std::thread_local! {
    static FAST_TAIL_BYTES_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Maximum encoded JSON payload accepted for one atomic registry delta.
///
/// One transition may legitimately replace both registries at their existing
/// public size ceilings. The extra MiB covers the delta envelope and keys, so
/// adopting the WAL does not narrow the pre-existing checkpoint contract.
pub const MAX_REGISTRY_DELTA_FRAME_BYTES: usize =
    MAX_TASK_REGISTRY_BYTES + MAX_WATCH_REGISTRY_BYTES + 1024 * 1024;
/// Maximum durable registry-delta WAL size before a checkpoint is required.
pub const MAX_REGISTRY_DELTA_WAL_BYTES: usize = 256 * 1024 * 1024;

/// Monotonic durable revision of task/watch registry authority.
///
/// Revision zero denotes legacy checkpoint state before the first WAL frame.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct RegistryRevision(u64);

impl RegistryRevision {
    /// Legacy checkpoint watermark before the first delta.
    pub const ZERO: Self = Self(0);

    /// Creates a revision from its durable integer representation.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the durable integer representation.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the following revision, or `None` at exhaustion.
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// Contiguous revisions represented by one coalesced WAL frame.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryRevisionRange {
    /// First revision represented by the frame.
    pub first: RegistryRevision,
    /// Last revision represented by the frame.
    pub last: RegistryRevision,
}

impl RegistryRevisionRange {
    /// Creates a non-zero, ascending revision range.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryDeltaValidationError::InvalidRevisionRange`] for a
    /// zero first revision or descending bounds.
    pub fn new(
        first: RegistryRevision,
        last: RegistryRevision,
    ) -> std::result::Result<Self, RegistryDeltaValidationError> {
        let range = Self { first, last };
        range.validate()?;
        Ok(range)
    }

    /// Creates one single-revision range.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryDeltaValidationError::InvalidRevisionRange`] for
    /// revision zero.
    pub fn single(
        revision: RegistryRevision,
    ) -> std::result::Result<Self, RegistryDeltaValidationError> {
        Self::new(revision, revision)
    }

    fn validate(self) -> std::result::Result<(), RegistryDeltaValidationError> {
        if self.first == RegistryRevision::ZERO || self.first > self.last {
            return Err(RegistryDeltaValidationError::InvalidRevisionRange {
                first: self.first.get(),
                last: self.last.get(),
            });
        }
        Ok(())
    }
}

/// Invalid caller-provided registry delta.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum RegistryDeltaValidationError {
    /// A revision range is zero-based or descending.
    #[error("registry revision range must be non-zero and ascending, received {first}..={last}")]
    InvalidRevisionRange {
        /// First supplied revision.
        first: u64,
        /// Last supplied revision.
        last: u64,
    },
    /// A task upsert map key does not equal its embedded identifier.
    #[error("task upsert key '{key}' does not match embedded task identifier '{record_id}'")]
    TaskIdentifierMismatch {
        /// Map key supplied by the caller.
        key: String,
        /// Identifier carried by the task record.
        record_id: String,
    },
    /// A watch upsert map key does not equal its embedded identifier.
    #[error("watch upsert key '{key}' does not match embedded watch identifier '{record_id}'")]
    WatchIdentifierMismatch {
        /// Map key supplied by the caller.
        key: String,
        /// Identifier carried by the watch record.
        record_id: String,
    },
    /// A delta contains an empty task identifier.
    #[error("registry delta contains an empty task identifier")]
    EmptyTaskIdentifier,
    /// A delta contains an empty watch identifier.
    #[error("registry delta contains an empty watch identifier")]
    EmptyWatchIdentifier,
    /// One task is both upserted and removed by the same atomic batch.
    #[error("registry delta both upserts and removes task '{task_id}'")]
    ConflictingTaskMutation {
        /// Ambiguous task identifier.
        task_id: String,
    },
    /// The explicit watch insertion order is incomplete or contains extras.
    #[error("watch upsert order does not name every upsert exactly once")]
    InvalidWatchUpsertOrder,
    /// The watch insertion order names an identifier with no upsert.
    #[error("watch upsert order names missing upsert '{watch_id}'")]
    UnknownOrderedWatchUpsert {
        /// Identifier named only by the order vector.
        watch_id: String,
    },
    /// The watch insertion order repeats an identifier.
    #[error("watch upsert order repeats identifier '{watch_id}'")]
    DuplicateOrderedWatchUpsert {
        /// Repeated identifier.
        watch_id: String,
    },
    /// The input watch registry already contains duplicate identifiers.
    #[error("watch registry contains duplicate identifier '{watch_id}'")]
    DuplicateWatchIdentifier {
        /// Duplicated watch identifier.
        watch_id: String,
    },
}

/// One atomic task/watch registry mutation.
///
/// The task and watch halves are encoded in one checksummed WAL frame. Public
/// fields keep daemon-side mutation assembly allocation-efficient while
/// [`Self::apply_to`] rejects ambiguous or identifier-inconsistent batches.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryDeltaBatch {
    /// Complete task records to insert or replace by task identifier.
    pub task_upserts: BTreeMap<String, TaskRecord>,
    /// Task identifiers to remove.
    pub task_removals: BTreeSet<String>,
    /// Complete watch records to insert or replace by watch identifier.
    pub watch_upserts: BTreeMap<String, WatchRegistration>,
    /// Observable insertion order for `watch_upserts`.
    ///
    /// This must contain every `watch_upserts` key exactly once. A watch also
    /// present in `watch_removals` is removed first and then appended at this
    /// position, explicitly representing remove-then-upsert reinsertion.
    pub watch_upsert_order: Vec<String>,
    /// Watch identifiers to remove.
    pub watch_removals: BTreeSet<String>,
}

impl RegistryDeltaBatch {
    /// Adds a later task upsert and clears an earlier removal of the same key.
    #[must_use]
    pub fn upsert_task(mut self, task: TaskRecord) -> Self {
        let task_id = task.task_id.clone();
        self.task_removals.remove(&task_id);
        self.task_upserts.insert(task_id, task);
        self
    }

    /// Adds a later task removal and clears an earlier upsert of the same key.
    #[must_use]
    pub fn remove_task(mut self, task_id: impl Into<String>) -> Self {
        let task_id = task_id.into();
        self.task_upserts.remove(&task_id);
        self.task_removals.insert(task_id);
        self
    }

    /// Adds a later watch upsert while preserving its sequential insertion
    /// position.
    ///
    /// If an earlier removal exists, the overlap is retained to represent
    /// remove-then-upsert reinsertion at the end of the observable watch list.
    #[must_use]
    pub fn upsert_watch(mut self, watch: WatchRegistration) -> Self {
        let watch_id = watch.watch_id.clone();
        if !self.watch_upserts.contains_key(&watch_id) {
            self.watch_upsert_order.push(watch_id.clone());
        }
        self.watch_upserts.insert(watch_id, watch);
        self
    }

    /// Adds a later watch removal and clears an earlier upsert/order entry.
    #[must_use]
    pub fn remove_watch(mut self, watch_id: impl Into<String>) -> Self {
        let watch_id = watch_id.into();
        self.watch_upserts.remove(&watch_id);
        self.watch_upsert_order
            .retain(|candidate| candidate != &watch_id);
        self.watch_removals.insert(watch_id);
        self
    }

    /// Returns whether the batch has no observable registry mutation.
    pub fn is_empty(&self) -> bool {
        self.task_upserts.is_empty()
            && self.task_removals.is_empty()
            && self.watch_upserts.is_empty()
            && self.watch_upsert_order.is_empty()
            && self.watch_removals.is_empty()
    }

    /// Coalesces a later batch into this batch using per-identifier
    /// last-mutation-wins semantics.
    ///
    /// # Errors
    ///
    /// Returns an identifier or overlap error if either input was assembled
    /// inconsistently before coalescing.
    pub fn merge_later_wins(
        &mut self,
        later: Self,
    ) -> std::result::Result<(), RegistryDeltaValidationError> {
        self.validate()?;
        later.validate()?;

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
        for watch_id in &later.watch_upsert_order {
            let watch = later
                .watch_upserts
                .get(watch_id)
                .expect("validated watch upsert order")
                .clone();
            let later_reinserts = later.watch_removals.contains(watch_id);
            let already_upserted = self.watch_upserts.contains_key(watch_id);
            let already_removed = self.watch_removals.contains(watch_id);
            if later_reinserts {
                self.watch_removals.insert(watch_id.clone());
                self.watch_upsert_order
                    .retain(|candidate| candidate != watch_id);
                self.watch_upsert_order.push(watch_id.clone());
            } else if already_removed {
                // An earlier removal followed by this later upsert is a
                // reinsertion, so retain the removal marker and append last.
                self.watch_upsert_order.push(watch_id.clone());
            } else if !already_upserted {
                self.watch_upsert_order.push(watch_id.clone());
            }
            self.watch_upserts.insert(watch_id.clone(), watch);
        }
        Ok(())
    }

    /// Atomically applies this batch to task and watch registries.
    ///
    /// Both inputs remain unchanged if validation fails. Existing watch order
    /// is retained for replacements; newly admitted and explicitly reinserted
    /// watches append in [`Self::watch_upsert_order`].
    ///
    /// # Errors
    ///
    /// Returns a typed error for empty or mismatched identifiers, ambiguous
    /// upsert/removal pairs, or duplicate identifiers in the input watch
    /// registry.
    pub fn apply_to(
        &self,
        tasks: &mut TaskRegistry,
        watches: &mut WatchRegistry,
    ) -> std::result::Result<(), RegistryDeltaValidationError> {
        self.validate()?;
        let mut admitted_watch_ids = BTreeSet::new();
        for watch in &watches.watches {
            if !admitted_watch_ids.insert(watch.watch_id.as_str()) {
                return Err(RegistryDeltaValidationError::DuplicateWatchIdentifier {
                    watch_id: watch.watch_id.clone(),
                });
            }
        }

        // All fallible validation finishes above. Mutations below touch only
        // dirty task/watch records plus one watch-position index, so applying
        // a small delta never clones the O(total tasks) registry.
        for task_id in &self.task_removals {
            tasks.tasks.remove(task_id);
        }
        for (task_id, task) in &self.task_upserts {
            tasks.tasks.insert(task_id.clone(), task.clone());
        }
        watches
            .watches
            .retain(|watch| !self.watch_removals.contains(&watch.watch_id));
        let mut watch_positions = watches
            .watches
            .iter()
            .enumerate()
            .map(|(position, watch)| (watch.watch_id.clone(), position))
            .collect::<BTreeMap<_, _>>();
        for watch_id in &self.watch_upsert_order {
            let watch = &self.watch_upserts[watch_id];
            if let Some(position) = watch_positions.get(watch_id).copied() {
                watches.watches[position] = watch.clone();
            } else {
                let position = watches.watches.len();
                watches.watches.push(watch.clone());
                watch_positions.insert(watch_id.clone(), position);
            }
        }
        Ok(())
    }

    fn validate(&self) -> std::result::Result<(), RegistryDeltaValidationError> {
        for (task_id, task) in &self.task_upserts {
            validate_task_identifier(task_id)?;
            if task.task_id != *task_id {
                return Err(RegistryDeltaValidationError::TaskIdentifierMismatch {
                    key: task_id.clone(),
                    record_id: task.task_id.clone(),
                });
            }
            if self.task_removals.contains(task_id) {
                return Err(RegistryDeltaValidationError::ConflictingTaskMutation {
                    task_id: task_id.clone(),
                });
            }
        }
        for task_id in &self.task_removals {
            validate_task_identifier(task_id)?;
        }
        for (watch_id, watch) in &self.watch_upserts {
            validate_watch_identifier(watch_id)?;
            if watch.watch_id != *watch_id {
                return Err(RegistryDeltaValidationError::WatchIdentifierMismatch {
                    key: watch_id.clone(),
                    record_id: watch.watch_id.clone(),
                });
            }
        }
        let mut ordered = BTreeSet::new();
        for watch_id in &self.watch_upsert_order {
            if !self.watch_upserts.contains_key(watch_id) {
                return Err(RegistryDeltaValidationError::UnknownOrderedWatchUpsert {
                    watch_id: watch_id.clone(),
                });
            }
            if !ordered.insert(watch_id.as_str()) {
                return Err(RegistryDeltaValidationError::DuplicateOrderedWatchUpsert {
                    watch_id: watch_id.clone(),
                });
            }
        }
        if ordered.len() != self.watch_upserts.len() {
            return Err(RegistryDeltaValidationError::InvalidWatchUpsertOrder);
        }
        for watch_id in &self.watch_removals {
            validate_watch_identifier(watch_id)?;
        }
        Ok(())
    }
}

fn validate_task_identifier(
    task_id: &str,
) -> std::result::Result<(), RegistryDeltaValidationError> {
    if task_id.trim().is_empty() {
        return Err(RegistryDeltaValidationError::EmptyTaskIdentifier);
    }
    Ok(())
}

fn validate_watch_identifier(
    watch_id: &str,
) -> std::result::Result<(), RegistryDeltaValidationError> {
    if watch_id.trim().is_empty() {
        return Err(RegistryDeltaValidationError::EmptyWatchIdentifier);
    }
    Ok(())
}

/// Authoritative task/watch state after checkpoint loading and WAL replay.
#[derive(Clone, Debug)]
pub struct LoadedTaskWatchRegistry {
    /// Materialized task registry.
    pub tasks: TaskRegistry,
    /// Materialized watch registry.
    pub watches: WatchRegistry,
    /// WAL revision included by the committed checkpoint manifest.
    pub checkpoint_revision: RegistryRevision,
    /// Latest revision after valid WAL frames were replayed.
    pub replayed_revision: RegistryRevision,
}

/// Returns the diagnostic path of the task/watch registry delta WAL.
pub fn registry_delta_wal_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(REGISTRY_DELTA_WAL_FILE_NAME)
}

/// Appends and synchronizes one atomic task/watch registry delta.
///
/// The revision range must begin immediately after the durable WAL tail.
/// Multiple logical revisions may be coalesced into one frame, but replay
/// treats the resulting task/watch mutation as one indivisible transaction.
///
/// # Errors
///
/// Returns typed batch, revision, size, corruption, path-safety, locking, and
/// synchronization errors without appending a partial in-process frame.
pub fn append_task_watch_registry_delta(
    root: &Path,
    revisions: RegistryRevisionRange,
    batch: &RegistryDeltaBatch,
) -> Result<()> {
    revisions
        .validate()
        .map_err(|error| invalid_batch(root, error))?;
    batch
        .validate()
        .map_err(|error| invalid_batch(root, error))?;
    let path = registry_delta_wal_path(root);
    let payload = serde_json::to_vec(batch).map_err(|source| {
        DaemonCoreError::json("failed to encode registry delta for", &path, source)
    })?;
    if payload.len() > MAX_REGISTRY_DELTA_FRAME_BYTES {
        return Err(DaemonCoreError::RegistryDeltaFrameTooLarge {
            path,
            encoded_bytes: payload.len() as u64,
            max_bytes: MAX_REGISTRY_DELTA_FRAME_BYTES as u64,
        });
    }
    let frame_header = encode_frame_header(revisions, &payload);
    let frame_footer = encode_frame_footer(&frame_header, payload.len());
    let _writer_lease = acquire_task_store_writer_lease(root)?;

    #[cfg(unix)]
    {
        with_anchored_task_registry_lock(
            root,
            RegistryLockMode::Exclusive,
            || Ok(()),
            |daemon| {
                append_under_task_lock(
                    root,
                    revisions,
                    &frame_header,
                    &payload,
                    &frame_footer,
                    || checkpoint_revision_anchored(root, daemon),
                )
            },
        )
    }
    #[cfg(not(unix))]
    {
        let task_path = task_registry_path(root);
        with_registry_lock(root, &task_path, RegistryLockMode::Exclusive, || {
            append_under_task_lock(
                root,
                revisions,
                &frame_header,
                &payload,
                &frame_footer,
                || checkpoint_revision_portable(root),
            )
        })
    }
}

/// Loads one committed checkpoint and replays every newer valid WAL frame.
///
/// A crash-torn final frame is truncated and synchronized under the exclusive
/// registry lock. Complete checksum, schema, revision, or relationship
/// corruption fails closed.
///
/// # Errors
///
/// Returns the same checkpoint errors as
/// [`load_task_watch_registry_checkpoint_with_event_tails`], plus typed WAL
/// corruption, size, path-safety, and repair errors.
pub fn load_task_watch_registry_with_deltas(root: &Path) -> Result<LoadedTaskWatchRegistry> {
    let _writer_lease = acquire_task_store_writer_lease(root)?;
    #[cfg(unix)]
    {
        with_anchored_task_registry_lock(
            root,
            RegistryLockMode::Exclusive,
            || Ok(()),
            |daemon| load_under_task_lock_anchored(root, daemon),
        )
    }
    #[cfg(not(unix))]
    {
        let task_path = task_registry_path(root);
        with_registry_lock(root, &task_path, RegistryLockMode::Exclusive, || {
            load_under_task_lock_portable(root)
        })
    }
}

/// Loads checkpoint-plus-WAL registry authority and authenticated event tails
/// beneath the same task-registry lock.
///
/// # Errors
///
/// Returns the same errors as [`load_task_watch_registry_with_deltas`] and the
/// strict task-event tail reader.
pub fn load_task_watch_registry_with_deltas_and_event_tails(
    root: &Path,
) -> Result<(LoadedTaskWatchRegistry, BTreeMap<String, Option<u64>>)> {
    let writer_lease = acquire_task_store_writer_lease(root)?;
    #[cfg(unix)]
    {
        with_anchored_task_registry_lock(
            root,
            RegistryLockMode::Exclusive,
            || Ok(()),
            |daemon| {
                let loaded = load_under_task_lock_anchored(root, daemon)?;
                let mut tails = BTreeMap::new();
                for task_id in loaded.tasks.tasks.keys() {
                    let storage_id = checked_task_storage_id(root, task_id)?;
                    let tail = event_tail::task_event_log_tail_sequence_admitted(
                        root,
                        &storage_id,
                        &writer_lease,
                    )?;
                    tails.insert(task_id.clone(), tail);
                }
                Ok((loaded, tails))
            },
        )
    }
    #[cfg(not(unix))]
    {
        let task_path = task_registry_path(root);
        with_registry_lock(root, &task_path, RegistryLockMode::Exclusive, || {
            let loaded = load_under_task_lock_portable(root)?;
            let mut tails = BTreeMap::new();
            for task_id in loaded.tasks.tasks.keys() {
                let storage_id = checked_task_storage_id(root, task_id)?;
                let tail = event_tail::task_event_log_tail_sequence_portable(root, &storage_id)?;
                tails.insert(task_id.clone(), tail);
            }
            let _ = &writer_lease;
            Ok((loaded, tails))
        })
    }
}

/// Commits a full task/watch checkpoint at one replayed WAL revision.
///
/// The supplied registries must equal checkpoint-plus-WAL replay at
/// `revision`. The new manifest is committed before the WAL is atomically
/// reset. If reset fails, the committed watermark remains authoritative and a
/// later load safely skips retained frames at or below it.
///
/// # Errors
///
/// Returns validation, replay, revision mismatch, checkpoint publication,
/// path-safety, and WAL-reset errors.
pub fn save_task_watch_registry_checkpoint_at_revision(
    root: &Path,
    tasks: &TaskRegistry,
    watches: &WatchRegistry,
    revision: RegistryRevision,
) -> Result<()> {
    validate_task_watch_registry_relationships(root, tasks, watches)?;
    let task_path = task_registry_path(root);
    let watch_path = watch_registry_path(root);
    let task_bytes = encode_task_registry(&task_path, tasks)?;
    let _ = encode_watch_registry(&watch_path, watches, None)?;
    let _writer_lease = acquire_task_store_writer_lease(root)?;

    #[cfg(unix)]
    {
        with_anchored_task_registry_lock(
            root,
            RegistryLockMode::Exclusive,
            || Ok(()),
            |daemon| {
                let loaded = load_under_task_lock_anchored(root, daemon)?;
                validate_checkpoint_candidate(root, &loaded, tasks, watches, revision)?;
                save_task_watch_registry_checkpoint_anchored(
                    root,
                    daemon,
                    tasks,
                    watches,
                    task_bytes,
                    Some(revision.get()),
                )?;
                reset_wal(root, revision)
            },
        )
    }
    #[cfg(not(unix))]
    {
        with_registry_lock(root, &task_path, RegistryLockMode::Exclusive, || {
            let loaded = load_under_task_lock_portable(root)?;
            validate_checkpoint_candidate(root, &loaded, tasks, watches, revision)?;
            save_task_watch_registry_checkpoint_portable(
                root,
                tasks,
                watches,
                task_bytes,
                Some(revision.get()),
            )?;
            reset_wal(root, revision)
        })
    }
}

#[cfg(unix)]
fn checkpoint_revision_anchored(root: &Path, daemon: &CapabilityDir) -> Result<RegistryRevision> {
    let loaded =
        load_task_watch_registry_checkpoint_with_delta_revision_under_task_lock(root, daemon)?;
    Ok(RegistryRevision::new(loaded.applied_delta_revision))
}

#[cfg(not(unix))]
fn checkpoint_revision_portable(root: &Path) -> Result<RegistryRevision> {
    let loaded =
        load_task_watch_registry_checkpoint_with_delta_revision_portable_under_task_lock(root)?;
    Ok(RegistryRevision::new(loaded.applied_delta_revision))
}

#[cfg(unix)]
fn load_under_task_lock_anchored(
    root: &Path,
    daemon: &CapabilityDir,
) -> Result<LoadedTaskWatchRegistry> {
    let loaded =
        load_task_watch_registry_checkpoint_with_delta_revision_under_task_lock(root, daemon)?;
    replay_wal(
        root,
        loaded.tasks,
        loaded.watches,
        RegistryRevision::new(loaded.applied_delta_revision),
    )
}

#[cfg(not(unix))]
fn load_under_task_lock_portable(root: &Path) -> Result<LoadedTaskWatchRegistry> {
    let loaded =
        load_task_watch_registry_checkpoint_with_delta_revision_portable_under_task_lock(root)?;
    replay_wal(
        root,
        loaded.tasks,
        loaded.watches,
        RegistryRevision::new(loaded.applied_delta_revision),
    )
}

fn replay_wal(
    root: &Path,
    mut tasks: TaskRegistry,
    mut watches: WatchRegistry,
    checkpoint_revision: RegistryRevision,
) -> Result<LoadedTaskWatchRegistry> {
    let path = registry_delta_wal_path(root);
    let state = open_daemon_state(root)?;
    let Some(mut wal) = state
        .open_existing(REGISTRY_DELTA_WAL_FILE_NAME, FileAccess::ReadWrite)
        .map_err(|source| wal_io("failed to open registry delta WAL", &path, source))?
    else {
        return Ok(LoadedTaskWatchRegistry {
            tasks,
            watches,
            checkpoint_revision,
            replayed_revision: checkpoint_revision,
        });
    };

    let mut replayed_revision = checkpoint_revision;
    let inspection = scan_wal(&mut wal, &path, true, |revisions, batch| {
        if revisions.last <= checkpoint_revision {
            return Ok(());
        }
        let expected = replayed_revision.checked_next().ok_or_else(|| {
            invalid_wal(
                &path,
                "registry revision is exhausted before a replayed frame",
            )
        })?;
        if revisions.first != expected {
            return Err(invalid_wal(
                &path,
                format!(
                    "frame {}..={} does not continue checkpoint/replay revision {}",
                    revisions.first.get(),
                    revisions.last.get(),
                    replayed_revision.get()
                ),
            ));
        }
        batch
            .apply_to(&mut tasks, &mut watches)
            .map_err(|error| invalid_wal(&path, error.to_string()))?;
        validate_task_watch_registry_relationships(root, &tasks, &watches)
            .map_err(|error| invalid_wal(&path, error.to_string()))?;
        replayed_revision = revisions.last;
        Ok(())
    })?;
    if inspection.base_revision > checkpoint_revision {
        return Err(invalid_wal(
            &path,
            format!(
                "WAL base revision {} is newer than committed checkpoint revision {}",
                inspection.base_revision.get(),
                checkpoint_revision.get()
            ),
        ));
    }
    if inspection.last_revision < checkpoint_revision {
        // This is valid after a committed checkpoint whose obsolete WAL was
        // retained or externally archived before the reset header appeared.
        replayed_revision = checkpoint_revision;
    }
    Ok(LoadedTaskWatchRegistry {
        tasks,
        watches,
        checkpoint_revision,
        replayed_revision,
    })
}

fn append_under_task_lock(
    root: &Path,
    revisions: RegistryRevisionRange,
    frame_header: &[u8; FRAME_HEADER_BYTES],
    payload: &[u8],
    frame_footer: &[u8; FRAME_FOOTER_BYTES],
    checkpoint_revision: impl FnOnce() -> Result<RegistryRevision>,
) -> Result<()> {
    let path = registry_delta_wal_path(root);
    let state = open_daemon_state(root)?;
    let mut wal = match state
        .open_existing(REGISTRY_DELTA_WAL_FILE_NAME, FileAccess::ReadWrite)
        .map_err(|source| wal_io("failed to open registry delta WAL", &path, source))?
    {
        Some(wal) => wal,
        None => {
            let checkpoint_revision = checkpoint_revision()?;
            state
                .write_atomic(
                    REGISTRY_DELTA_WAL_FILE_NAME,
                    &encode_wal_header(checkpoint_revision),
                )
                .map_err(|source| {
                    wal_io("failed to initialize registry delta WAL", &path, source)
                })?;
            state
                .open_existing(REGISTRY_DELTA_WAL_FILE_NAME, FileAccess::ReadWrite)
                .map_err(|source| {
                    wal_io(
                        "failed to reopen initialized registry delta WAL",
                        &path,
                        source,
                    )
                })?
                .ok_or_else(|| {
                    wal_io(
                        "failed to reopen initialized registry delta WAL",
                        &path,
                        std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "initialized WAL is missing",
                        ),
                    )
                })?
        }
    };
    let inspection = inspect_wal_tail(&mut wal, &path)?;
    if let Some(last_frame) = inspection.last_frame {
        verify_last_frame_payload(&mut wal, &path, last_frame)?;
    }
    if let Some(last_frame) = inspection.last_frame {
        if revisions == last_frame.revisions {
            let payload_checksum = blake3::hash(payload);
            if payload_checksum.as_bytes() != &last_frame.payload_checksum {
                return Err(invalid_wal(
                    &path,
                    format!(
                        "retry for durable revision range {}..={} carries different payload bytes",
                        revisions.first.get(),
                        revisions.last.get()
                    ),
                ));
            }
            wal.file_mut().sync_all().map_err(|source| {
                wal_io(
                    "failed to synchronize idempotent registry delta WAL retry",
                    &path,
                    source,
                )
            })?;
            return wal.validate_attachment().map_err(|source| {
                DaemonCoreError::StorageMutationAuthorityLost {
                    operation: "registry-delta WAL idempotent retry",
                    path,
                    source,
                }
            });
        }
    }
    let expected_first = inspection
        .last_revision
        .checked_next()
        .ok_or_else(|| invalid_wal(&path, "registry revision is exhausted"))?;
    if revisions.first != expected_first {
        return Err(DaemonCoreError::RegistryDeltaRevisionMismatch {
            path,
            expected_first: expected_first.get(),
            actual_first: revisions.first.get(),
            actual_last: revisions.last.get(),
        });
    }

    let frame_bytes = FRAME_HEADER_BYTES
        .checked_add(payload.len())
        .and_then(|bytes| bytes.checked_add(FRAME_FOOTER_BYTES))
        .ok_or_else(|| invalid_wal(&path, "registry delta frame length overflow"))?;
    let resulting_bytes = inspection
        .complete_len
        .checked_add(frame_bytes as u64)
        .ok_or_else(|| invalid_wal(&path, "registry delta WAL length overflow"))?;
    if resulting_bytes > MAX_REGISTRY_DELTA_WAL_BYTES as u64 {
        return Err(DaemonCoreError::RegistryDeltaWalTooLarge {
            path,
            encoded_bytes: resulting_bytes,
            max_bytes: MAX_REGISTRY_DELTA_WAL_BYTES as u64,
        });
    }

    wal.file_mut()
        .seek(SeekFrom::Start(inspection.complete_len))
        .and_then(|_| wal.file_mut().write_all(frame_header))
        .and_then(|_| wal.file_mut().write_all(payload))
        .and_then(|_| wal.file_mut().write_all(frame_footer))
        .and_then(|_| wal.file_mut().sync_all())
        .map_err(|source| {
            wal_io(
                "failed to append and synchronize registry delta WAL",
                &path,
                source,
            )
        })?;
    wal.validate_attachment()
        .map_err(|source| DaemonCoreError::StorageMutationAuthorityLost {
            operation: "registry-delta WAL append",
            path,
            source,
        })
}

fn reset_wal(root: &Path, revision: RegistryRevision) -> Result<()> {
    let path = registry_delta_wal_path(root);
    let state = open_daemon_state(root)?;
    state
        .write_atomic(REGISTRY_DELTA_WAL_FILE_NAME, &encode_wal_header(revision))
        .map_err(|source| wal_io("failed to reset committed registry delta WAL", path, source))
}

fn validate_checkpoint_candidate(
    root: &Path,
    loaded: &LoadedTaskWatchRegistry,
    tasks: &TaskRegistry,
    watches: &WatchRegistry,
    revision: RegistryRevision,
) -> Result<()> {
    if revision != loaded.replayed_revision {
        return Err(DaemonCoreError::RegistryDeltaRevisionMismatch {
            path: registry_delta_wal_path(root),
            expected_first: loaded.replayed_revision.get(),
            actual_first: revision.get(),
            actual_last: revision.get(),
        });
    }
    let expected_tasks = serde_json::to_value(&loaded.tasks).map_err(|source| {
        DaemonCoreError::json(
            "failed to compare replayed task registry for",
            task_registry_path(root),
            source,
        )
    })?;
    let supplied_tasks = serde_json::to_value(tasks).map_err(|source| {
        DaemonCoreError::json(
            "failed to compare checkpoint task registry for",
            task_registry_path(root),
            source,
        )
    })?;
    if expected_tasks != supplied_tasks {
        return Err(DaemonCoreError::InvalidRegistryDeltaBatch {
            root: root.to_path_buf(),
            message: format!(
                "checkpoint task registry does not equal WAL replay at revision {}",
                revision.get()
            ),
        });
    }
    let expected_watches = serde_json::to_value(&loaded.watches).map_err(|source| {
        DaemonCoreError::json(
            "failed to compare replayed watch registry for",
            watch_registry_path(root),
            source,
        )
    })?;
    let supplied_watches = serde_json::to_value(watches).map_err(|source| {
        DaemonCoreError::json(
            "failed to compare checkpoint watch registry for",
            watch_registry_path(root),
            source,
        )
    })?;
    if expected_watches != supplied_watches {
        return Err(DaemonCoreError::InvalidRegistryDeltaBatch {
            root: root.to_path_buf(),
            message: format!(
                "checkpoint watch registry does not equal WAL replay at revision {}",
                revision.get()
            ),
        });
    }
    Ok(())
}

fn open_daemon_state(root: &Path) -> Result<StateDir> {
    let path = daemon_dir(root);
    StateDir::open(root, &[".packet28", "daemon"], true)
        .map_err(|source| wal_io("failed to open registry delta WAL directory", path, source))
}

#[derive(Clone, Copy, Debug)]
struct WalInspection {
    base_revision: RegistryRevision,
    last_revision: RegistryRevision,
    complete_len: u64,
    last_frame: Option<FrameTail>,
}

#[derive(Clone, Copy, Debug)]
struct FrameTail {
    offset: u64,
    payload_len: u64,
    payload_checksum: [u8; 32],
    revisions: RegistryRevisionRange,
}

fn scan_wal(
    wal: &mut StateFile,
    path: &Path,
    repair_torn_suffix: bool,
    mut visit: impl FnMut(RegistryRevisionRange, RegistryDeltaBatch) -> Result<()>,
) -> Result<WalInspection> {
    let file_len = wal
        .len()
        .map_err(|source| wal_io("failed to inspect registry delta WAL", path, source))?;
    if file_len > MAX_REGISTRY_DELTA_WAL_BYTES as u64 {
        return Err(DaemonCoreError::RegistryDeltaWalTooLarge {
            path: path.to_path_buf(),
            encoded_bytes: file_len,
            max_bytes: MAX_REGISTRY_DELTA_WAL_BYTES as u64,
        });
    }
    if file_len < WAL_HEADER_BYTES as u64 {
        return Err(invalid_wal(
            path,
            format!("WAL header is truncated: {file_len} bytes, expected {WAL_HEADER_BYTES}"),
        ));
    }
    wal.file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|source| wal_io("failed to seek registry delta WAL", path, source))?;
    let mut wal_header = [0_u8; WAL_HEADER_BYTES];
    wal.read_exact(&mut wal_header)
        .map_err(|source| wal_io("failed to read registry delta WAL header", path, source))?;
    let base_revision = decode_wal_header(path, &wal_header)?;
    let mut last_revision = base_revision;
    let mut last_frame = None;
    let mut offset = WAL_HEADER_BYTES as u64;

    loop {
        let remaining = file_len.saturating_sub(offset);
        if remaining == 0 {
            break;
        }
        if remaining < FRAME_HEADER_BYTES as u64 {
            return repair_or_reject_torn_suffix(
                wal,
                path,
                repair_torn_suffix,
                offset,
                base_revision,
                last_revision,
                last_frame,
            );
        }

        wal.file_mut()
            .seek(SeekFrom::Start(offset))
            .map_err(|source| wal_io("failed to seek registry delta frame", path, source))?;
        let mut frame_header = [0_u8; FRAME_HEADER_BYTES];
        wal.read_exact(&mut frame_header)
            .map_err(|source| wal_io("failed to read registry delta frame header", path, source))?;
        let decoded = decode_frame_header(path, offset, &frame_header)?;
        let frame_len = (FRAME_HEADER_BYTES as u64)
            .checked_add(decoded.payload_len)
            .and_then(|bytes| bytes.checked_add(FRAME_FOOTER_BYTES as u64))
            .ok_or_else(|| invalid_wal(path, "registry delta frame length overflow"))?;
        if remaining < frame_len {
            return repair_or_reject_torn_suffix(
                wal,
                path,
                repair_torn_suffix,
                offset,
                base_revision,
                last_revision,
                last_frame,
            );
        }
        let expected_first = last_revision
            .checked_next()
            .ok_or_else(|| invalid_wal(path, "registry revision is exhausted before WAL end"))?;
        if decoded.revisions.first != expected_first {
            return Err(invalid_wal(
                path,
                format!(
                    "revision gap at byte {offset}: expected {}, found {}..={}",
                    expected_first.get(),
                    decoded.revisions.first.get(),
                    decoded.revisions.last.get()
                ),
            ));
        }

        let payload_len = usize::try_from(decoded.payload_len)
            .map_err(|_| invalid_wal(path, "registry delta payload length does not fit memory"))?;
        let mut payload = Vec::new();
        payload.try_reserve_exact(payload_len).map_err(|source| {
            wal_io(
                "failed to reserve registry delta frame",
                path,
                source.into(),
            )
        })?;
        payload.resize(payload_len, 0);
        wal.read_exact(&mut payload)
            .map_err(|source| wal_io("failed to read registry delta frame", path, source))?;
        if blake3::hash(&payload).as_bytes() != &decoded.payload_checksum {
            return Err(invalid_wal(
                path,
                format!("payload checksum mismatch for complete frame at byte {offset}"),
            ));
        }
        let batch: RegistryDeltaBatch = serde_json::from_slice(&payload).map_err(|source| {
            invalid_wal(
                path,
                format!("complete frame at byte {offset} has invalid JSON: {source}"),
            )
        })?;
        batch.validate().map_err(|error| {
            invalid_wal(
                path,
                format!("complete frame at byte {offset} is invalid: {error}"),
            )
        })?;
        let mut footer = [0_u8; FRAME_FOOTER_BYTES];
        wal.read_exact(&mut footer)
            .map_err(|source| wal_io("failed to read registry delta frame footer", path, source))?;
        decode_frame_footer(path, offset, frame_len, &decoded, &frame_header, &footer)?;
        visit(decoded.revisions, batch)?;
        last_revision = decoded.revisions.last;
        last_frame = Some(FrameTail {
            offset,
            payload_len: decoded.payload_len,
            payload_checksum: decoded.payload_checksum,
            revisions: decoded.revisions,
        });
        offset = offset
            .checked_add(frame_len)
            .ok_or_else(|| invalid_wal(path, "registry delta WAL offset overflow"))?;
    }

    wal.validate_attachment()
        .map_err(|source| wal_io("registry delta WAL detached during read", path, source))?;
    Ok(WalInspection {
        base_revision,
        last_revision,
        complete_len: offset,
        last_frame,
    })
}

fn repair_or_reject_torn_suffix(
    wal: &mut StateFile,
    path: &Path,
    repair: bool,
    complete_len: u64,
    base_revision: RegistryRevision,
    last_revision: RegistryRevision,
    last_frame: Option<FrameTail>,
) -> Result<WalInspection> {
    if !repair {
        return Err(invalid_wal(path, "registry delta WAL ends in a torn frame"));
    }
    wal.file_mut()
        .set_len(complete_len)
        .and_then(|_| wal.file_mut().sync_all())
        .map_err(|source| {
            wal_io(
                "failed to truncate and synchronize torn registry delta WAL suffix",
                path,
                source,
            )
        })?;
    wal.validate_attachment()
        .map_err(|source| DaemonCoreError::StorageMutationAuthorityLost {
            operation: "registry-delta WAL suffix repair",
            path: path.to_path_buf(),
            source,
        })?;
    Ok(WalInspection {
        base_revision,
        last_revision,
        complete_len,
        last_frame,
    })
}

fn inspect_wal_tail(wal: &mut StateFile, path: &Path) -> Result<WalInspection> {
    let file_len = wal
        .len()
        .map_err(|source| wal_io("failed to inspect registry delta WAL", path, source))?;
    if file_len > MAX_REGISTRY_DELTA_WAL_BYTES as u64 {
        return Err(DaemonCoreError::RegistryDeltaWalTooLarge {
            path: path.to_path_buf(),
            encoded_bytes: file_len,
            max_bytes: MAX_REGISTRY_DELTA_WAL_BYTES as u64,
        });
    }
    if file_len < WAL_HEADER_BYTES as u64 {
        return Err(invalid_wal(
            path,
            format!("WAL header is truncated: {file_len} bytes, expected {WAL_HEADER_BYTES}"),
        ));
    }
    wal.file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|source| wal_io("failed to seek registry delta WAL", path, source))?;
    let mut wal_header = [0_u8; WAL_HEADER_BYTES];
    wal.read_exact(&mut wal_header)
        .map_err(|source| wal_io("failed to read registry delta WAL header", path, source))?;
    record_fast_tail_read(WAL_HEADER_BYTES);
    let base_revision = decode_wal_header(path, &wal_header)?;
    if file_len == WAL_HEADER_BYTES as u64 {
        return Ok(WalInspection {
            base_revision,
            last_revision: base_revision,
            complete_len: file_len,
            last_frame: None,
        });
    }
    if file_len - (WAL_HEADER_BYTES as u64) < (FRAME_HEADER_BYTES + FRAME_FOOTER_BYTES) as u64 {
        return scan_wal(wal, path, true, |_, _| Ok(()));
    }

    wal.file_mut()
        .seek(SeekFrom::Start(file_len - FRAME_FOOTER_BYTES as u64))
        .map_err(|source| wal_io("failed to seek registry delta WAL footer", path, source))?;
    let mut footer = [0_u8; FRAME_FOOTER_BYTES];
    wal.read_exact(&mut footer)
        .map_err(|source| wal_io("failed to read registry delta WAL footer", path, source))?;
    record_fast_tail_read(FRAME_FOOTER_BYTES);
    let Some(frame_len) = frame_length_from_footer(&footer) else {
        return scan_wal(wal, path, true, |_, _| Ok(()));
    };
    let minimum_frame_len = (FRAME_HEADER_BYTES + FRAME_FOOTER_BYTES) as u64;
    if frame_len < minimum_frame_len || frame_len > file_len.saturating_sub(WAL_HEADER_BYTES as u64)
    {
        return scan_wal(wal, path, true, |_, _| Ok(()));
    }
    let frame_offset = file_len - frame_len;
    wal.file_mut()
        .seek(SeekFrom::Start(frame_offset))
        .map_err(|source| wal_io("failed to seek final registry delta frame", path, source))?;
    let mut frame_header = [0_u8; FRAME_HEADER_BYTES];
    wal.read_exact(&mut frame_header)
        .map_err(|source| wal_io("failed to read final registry delta frame", path, source))?;
    record_fast_tail_read(FRAME_HEADER_BYTES);
    let decoded = match decode_frame_header(path, frame_offset, &frame_header) {
        Ok(decoded) => decoded,
        Err(_) => return scan_wal(wal, path, true, |_, _| Ok(())),
    };
    let expected_frame_len = (FRAME_HEADER_BYTES as u64)
        .checked_add(decoded.payload_len)
        .and_then(|bytes| bytes.checked_add(FRAME_FOOTER_BYTES as u64))
        .ok_or_else(|| invalid_wal(path, "registry delta frame length overflow"))?;
    if expected_frame_len != frame_len {
        return scan_wal(wal, path, true, |_, _| Ok(()));
    }
    if decode_frame_footer(
        path,
        frame_offset,
        frame_len,
        &decoded,
        &frame_header,
        &footer,
    )
    .is_err()
    {
        return scan_wal(wal, path, true, |_, _| Ok(()));
    }

    Ok(WalInspection {
        base_revision,
        last_revision: decoded.revisions.last,
        complete_len: file_len,
        last_frame: Some(FrameTail {
            offset: frame_offset,
            payload_len: decoded.payload_len,
            payload_checksum: decoded.payload_checksum,
            revisions: decoded.revisions,
        }),
    })
}

fn verify_last_frame_payload(wal: &mut StateFile, path: &Path, tail: FrameTail) -> Result<()> {
    wal.file_mut()
        .seek(SeekFrom::Start(
            tail.offset
                .checked_add(FRAME_HEADER_BYTES as u64)
                .ok_or_else(|| invalid_wal(path, "registry delta payload offset overflow"))?,
        ))
        .map_err(|source| wal_io("failed to seek final registry delta payload", path, source))?;
    let payload_len = usize::try_from(tail.payload_len)
        .map_err(|_| invalid_wal(path, "final registry delta payload does not fit memory"))?;
    let mut payload = vec![0_u8; payload_len];
    wal.read_exact(&mut payload)
        .map_err(|source| wal_io("failed to read final registry delta payload", path, source))?;
    record_fast_tail_read(payload_len);
    if blake3::hash(&payload).as_bytes() != &tail.payload_checksum {
        return Err(invalid_wal(
            path,
            "final durable frame payload checksum does not match its header",
        ));
    }
    Ok(())
}

fn encode_wal_header(base_revision: RegistryRevision) -> [u8; WAL_HEADER_BYTES] {
    let mut header = [0_u8; WAL_HEADER_BYTES];
    header[0..8].copy_from_slice(&WAL_MAGIC);
    header[8..12].copy_from_slice(&WAL_FORMAT_VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&(WAL_HEADER_BYTES as u32).to_le_bytes());
    header[16..24].copy_from_slice(&base_revision.get().to_le_bytes());
    let checksum = blake3::hash(&header[..24]);
    header[24..56].copy_from_slice(checksum.as_bytes());
    header
}

fn decode_wal_header(path: &Path, header: &[u8; WAL_HEADER_BYTES]) -> Result<RegistryRevision> {
    if header[0..8] != WAL_MAGIC {
        return Err(invalid_wal(
            path,
            "WAL magic does not identify Packet28 registry deltas",
        ));
    }
    let version = u32::from_le_bytes(header[8..12].try_into().expect("fixed header slice"));
    if version != WAL_FORMAT_VERSION {
        return Err(invalid_wal(
            path,
            format!("unsupported WAL format version {version}; expected {WAL_FORMAT_VERSION}"),
        ));
    }
    let header_len = u32::from_le_bytes(header[12..16].try_into().expect("fixed header slice"));
    if header_len as usize != WAL_HEADER_BYTES {
        return Err(invalid_wal(
            path,
            format!("invalid WAL header length {header_len}"),
        ));
    }
    let expected_checksum = blake3::hash(&header[..24]);
    if header[24..56] != expected_checksum.as_bytes()[..] {
        return Err(invalid_wal(path, "WAL header checksum mismatch"));
    }
    Ok(RegistryRevision::new(u64::from_le_bytes(
        header[16..24].try_into().expect("fixed header slice"),
    )))
}

fn encode_frame_header(
    revisions: RegistryRevisionRange,
    payload: &[u8],
) -> [u8; FRAME_HEADER_BYTES] {
    let mut header = [0_u8; FRAME_HEADER_BYTES];
    header[0..8].copy_from_slice(&FRAME_MAGIC);
    header[8..12].copy_from_slice(&FRAME_FORMAT_VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&(FRAME_HEADER_BYTES as u32).to_le_bytes());
    header[16..24].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    header[24..32].copy_from_slice(&revisions.first.get().to_le_bytes());
    header[32..40].copy_from_slice(&revisions.last.get().to_le_bytes());
    header[40..72].copy_from_slice(blake3::hash(payload).as_bytes());
    header
}

fn encode_frame_footer(
    frame_header: &[u8; FRAME_HEADER_BYTES],
    payload_len: usize,
) -> [u8; FRAME_FOOTER_BYTES] {
    let mut footer = [0_u8; FRAME_FOOTER_BYTES];
    footer[0..8].copy_from_slice(&FRAME_FOOTER_MAGIC);
    let frame_len = FRAME_HEADER_BYTES + payload_len + FRAME_FOOTER_BYTES;
    footer[8..16].copy_from_slice(&(frame_len as u64).to_le_bytes());
    footer[16..24].copy_from_slice(&frame_header[32..40]);
    footer[24..56].copy_from_slice(blake3::hash(frame_header).as_bytes());
    footer
}

fn frame_length_from_footer(footer: &[u8; FRAME_FOOTER_BYTES]) -> Option<u64> {
    if footer[0..8] != FRAME_FOOTER_MAGIC {
        return None;
    }
    Some(u64::from_le_bytes(
        footer[8..16].try_into().expect("fixed footer slice"),
    ))
}

fn decode_frame_footer(
    path: &Path,
    offset: u64,
    frame_len: u64,
    decoded: &DecodedFrameHeader,
    frame_header: &[u8; FRAME_HEADER_BYTES],
    footer: &[u8; FRAME_FOOTER_BYTES],
) -> Result<()> {
    if footer[0..8] != FRAME_FOOTER_MAGIC {
        return Err(invalid_wal(
            path,
            format!("invalid complete frame footer magic at byte {offset}"),
        ));
    }
    let encoded_frame_len =
        u64::from_le_bytes(footer[8..16].try_into().expect("fixed footer slice"));
    if encoded_frame_len != frame_len {
        return Err(invalid_wal(
            path,
            format!(
                "complete frame footer length mismatch at byte {offset}: expected {frame_len}, \
                 found {encoded_frame_len}"
            ),
        ));
    }
    let encoded_last = u64::from_le_bytes(footer[16..24].try_into().expect("fixed footer slice"));
    if encoded_last != decoded.revisions.last.get() {
        return Err(invalid_wal(
            path,
            format!(
                "complete frame footer revision mismatch at byte {offset}: expected {}, found \
                 {encoded_last}",
                decoded.revisions.last.get()
            ),
        ));
    }
    if footer[24..56] != blake3::hash(frame_header).as_bytes()[..] {
        return Err(invalid_wal(
            path,
            format!("complete frame footer checksum mismatch at byte {offset}"),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct DecodedFrameHeader {
    payload_len: u64,
    payload_checksum: [u8; 32],
    revisions: RegistryRevisionRange,
}

fn decode_frame_header(
    path: &Path,
    offset: u64,
    header: &[u8; FRAME_HEADER_BYTES],
) -> Result<DecodedFrameHeader> {
    if header[0..8] != FRAME_MAGIC {
        return Err(invalid_wal(
            path,
            format!("invalid complete frame magic at byte {offset}"),
        ));
    }
    let version = u32::from_le_bytes(header[8..12].try_into().expect("fixed header slice"));
    if version != FRAME_FORMAT_VERSION {
        return Err(invalid_wal(
            path,
            format!(
                "unsupported frame format version {version} at byte {offset}; expected \
                 {FRAME_FORMAT_VERSION}"
            ),
        ));
    }
    let header_len = u32::from_le_bytes(header[12..16].try_into().expect("fixed header slice"));
    if header_len as usize != FRAME_HEADER_BYTES {
        return Err(invalid_wal(
            path,
            format!("invalid frame header length {header_len} at byte {offset}"),
        ));
    }
    let payload_len = u64::from_le_bytes(header[16..24].try_into().expect("fixed header slice"));
    if payload_len > MAX_REGISTRY_DELTA_FRAME_BYTES as u64 {
        return Err(DaemonCoreError::RegistryDeltaFrameTooLarge {
            path: path.to_path_buf(),
            encoded_bytes: payload_len,
            max_bytes: MAX_REGISTRY_DELTA_FRAME_BYTES as u64,
        });
    }
    let first = RegistryRevision::new(u64::from_le_bytes(
        header[24..32].try_into().expect("fixed header slice"),
    ));
    let last = RegistryRevision::new(u64::from_le_bytes(
        header[32..40].try_into().expect("fixed header slice"),
    ));
    let revisions = RegistryRevisionRange::new(first, last).map_err(|error| {
        invalid_wal(
            path,
            format!("invalid revision range at byte {offset}: {error}"),
        )
    })?;
    let mut payload_checksum = [0_u8; 32];
    payload_checksum.copy_from_slice(&header[40..72]);
    Ok(DecodedFrameHeader {
        payload_len,
        payload_checksum,
        revisions,
    })
}

fn invalid_batch(root: &Path, error: RegistryDeltaValidationError) -> DaemonCoreError {
    DaemonCoreError::InvalidRegistryDeltaBatch {
        root: root.to_path_buf(),
        message: error.to_string(),
    }
}

fn invalid_wal(path: &Path, message: impl Into<String>) -> DaemonCoreError {
    DaemonCoreError::InvalidRegistryDeltaWal {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

fn wal_io(
    operation: &'static str,
    path: impl AsRef<Path>,
    source: std::io::Error,
) -> DaemonCoreError {
    DaemonCoreError::io(operation, path.as_ref(), source)
}

fn record_fast_tail_read(bytes: usize) {
    #[cfg(test)]
    FAST_TAIL_BYTES_READ.with(|observed| {
        observed.set(observed.get().saturating_add(bytes as u64));
    });
    #[cfg(not(test))]
    let _ = bytes;
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::{Seek as _, SeekFrom, Write as _};
    use std::sync::{Arc, Barrier};

    use packet28_daemon_protocol::commands::WatchSpec;
    use tempfile::tempdir;

    use super::*;

    fn task(task_id: &str, watch_ids: &[&str]) -> TaskRecord {
        TaskRecord {
            task_id: task_id.to_string(),
            watch_ids: watch_ids
                .iter()
                .map(|watch_id| (*watch_id).to_string())
                .collect(),
            ..TaskRecord::default()
        }
    }

    fn watch(watch_id: &str, task_id: &str) -> WatchRegistration {
        WatchRegistration {
            watch_id: watch_id.to_string(),
            spec: WatchSpec {
                task_id: task_id.to_string(),
                ..WatchSpec::default()
            },
            active: true,
            ..WatchRegistration::default()
        }
    }

    fn checkpoint(
        root: &Path,
        task_records: impl IntoIterator<Item = TaskRecord>,
        watch_records: impl IntoIterator<Item = WatchRegistration>,
    ) {
        let tasks = TaskRegistry {
            tasks: task_records
                .into_iter()
                .map(|task| (task.task_id.clone(), task))
                .collect(),
        };
        let watches = WatchRegistry {
            watches: watch_records.into_iter().collect(),
        };
        save_task_watch_registry_checkpoint(root, &tasks, &watches).unwrap();
    }

    #[test]
    fn frame_bound_preserves_the_existing_paired_registry_size_contract() {
        assert!(
            MAX_REGISTRY_DELTA_FRAME_BYTES
                >= MAX_TASK_REGISTRY_BYTES.saturating_add(MAX_WATCH_REGISTRY_BYTES)
        );
        assert!(MAX_REGISTRY_DELTA_FRAME_BYTES < MAX_REGISTRY_DELTA_WAL_BYTES);
    }

    fn add_watch_delta(task_id: &str, existing: &[&str], added: &str) -> RegistryDeltaBatch {
        let mut watch_ids = existing
            .iter()
            .map(|watch_id| (*watch_id).to_string())
            .collect::<Vec<_>>();
        watch_ids.push(added.to_string());
        RegistryDeltaBatch {
            task_upserts: BTreeMap::from([(
                task_id.to_string(),
                TaskRecord {
                    task_id: task_id.to_string(),
                    watch_ids,
                    ..TaskRecord::default()
                },
            )]),
            watch_upserts: BTreeMap::from([(added.to_string(), watch(added, task_id))]),
            watch_upsert_order: vec![added.to_string()],
            ..RegistryDeltaBatch::default()
        }
    }

    fn encoded_frame(revisions: RegistryRevisionRange, batch: &RegistryDeltaBatch) -> Vec<u8> {
        let payload = serde_json::to_vec(batch).unwrap();
        let header = encode_frame_header(revisions, &payload);
        let footer = encode_frame_footer(&header, payload.len());
        [header.as_slice(), payload.as_slice(), footer.as_slice()].concat()
    }

    #[test]
    fn apply_is_failure_atomic_and_preserves_observable_watch_order() {
        let mut tasks = TaskRegistry {
            tasks: BTreeMap::from([("task".to_string(), task("task", &["one", "two", "three"]))]),
        };
        let mut watches = WatchRegistry {
            watches: vec![
                watch("one", "task"),
                watch("two", "task"),
                watch("three", "task"),
            ],
        };
        let before = serde_json::to_value((&tasks, &watches)).unwrap();
        let invalid = RegistryDeltaBatch {
            task_upserts: BTreeMap::from([("task".to_string(), task("other", &[]))]),
            ..RegistryDeltaBatch::default()
        };

        assert!(matches!(
            invalid.apply_to(&mut tasks, &mut watches),
            Err(RegistryDeltaValidationError::TaskIdentifierMismatch { .. })
        ));
        assert_eq!(serde_json::to_value((&tasks, &watches)).unwrap(), before);

        let delta = RegistryDeltaBatch {
            watch_upserts: BTreeMap::from([
                ("one".to_string(), watch("one", "task")),
                ("two".to_string(), watch("two", "task")),
            ]),
            // `two` is explicitly removed then reinserted. `one` is a plain
            // replacement and therefore keeps its original position.
            watch_upsert_order: vec!["two".to_string(), "one".to_string()],
            watch_removals: BTreeSet::from(["two".to_string()]),
            ..RegistryDeltaBatch::default()
        };
        delta.apply_to(&mut tasks, &mut watches).unwrap();

        assert_eq!(
            watches
                .watches
                .iter()
                .map(|watch| watch.watch_id.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "three", "two"]
        );
    }

    #[test]
    fn merge_distinguishes_upsert_then_remove_from_remove_then_upsert() {
        let upsert = RegistryDeltaBatch {
            watch_upserts: BTreeMap::from([("watch".to_string(), watch("watch", "task"))]),
            watch_upsert_order: vec!["watch".to_string()],
            ..RegistryDeltaBatch::default()
        };
        let removal = RegistryDeltaBatch {
            watch_removals: BTreeSet::from(["watch".to_string()]),
            ..RegistryDeltaBatch::default()
        };

        let mut upsert_then_remove = upsert.clone();
        upsert_then_remove
            .merge_later_wins(removal.clone())
            .unwrap();
        assert!(upsert_then_remove.watch_upserts.is_empty());
        assert!(upsert_then_remove.watch_upsert_order.is_empty());
        assert_eq!(
            upsert_then_remove.watch_removals,
            BTreeSet::from(["watch".to_string()])
        );

        let mut remove_then_upsert = removal;
        remove_then_upsert.merge_later_wins(upsert).unwrap();
        assert!(remove_then_upsert.watch_upserts.contains_key("watch"));
        assert_eq!(
            remove_then_upsert.watch_removals,
            BTreeSet::from(["watch".to_string()])
        );
        assert_eq!(remove_then_upsert.watch_upsert_order, vec!["watch"]);
    }

    #[test]
    fn consuming_builders_preserve_later_wins_and_reinsertion_order() {
        let task_removed = RegistryDeltaBatch::default()
            .upsert_task(task("task", &[]))
            .remove_task("task");
        assert!(task_removed.task_upserts.is_empty());
        assert_eq!(
            task_removed.task_removals,
            BTreeSet::from(["task".to_string()])
        );

        let task_upserted = task_removed.upsert_task(task("task", &[]));
        assert!(task_upserted.task_removals.is_empty());
        assert!(task_upserted.task_upserts.contains_key("task"));

        let watches = RegistryDeltaBatch::default()
            .upsert_watch(watch("first", "task"))
            .upsert_watch(watch("second", "task"))
            .upsert_watch(watch("first", "task"))
            .remove_watch("second")
            .upsert_watch(watch("second", "task"));

        assert_eq!(watches.watch_upsert_order, vec!["first", "second"]);
        assert_eq!(
            watches.watch_removals,
            BTreeSet::from(["second".to_string()])
        );
        watches.validate().unwrap();

        let removed_last = watches.remove_watch("first");
        assert!(!removed_last.watch_upserts.contains_key("first"));
        assert_eq!(removed_last.watch_upsert_order, vec!["second"]);
        assert!(removed_last.watch_removals.contains("first"));
    }

    #[test]
    fn append_replays_a_coalesced_atomic_task_watch_delta() {
        let root = tempdir().unwrap();
        checkpoint(
            root.path(),
            [task("task", &["one"])],
            [watch("one", "task")],
        );
        let delta = add_watch_delta("task", &["one"], "two");

        append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::new(RegistryRevision::new(1), RegistryRevision::new(3)).unwrap(),
            &delta,
        )
        .unwrap();
        let (loaded, tails) =
            load_task_watch_registry_with_deltas_and_event_tails(root.path()).unwrap();

        assert_eq!(loaded.checkpoint_revision, RegistryRevision::ZERO);
        assert_eq!(loaded.replayed_revision, RegistryRevision::new(3));
        assert_eq!(loaded.tasks.tasks["task"].watch_ids, vec!["one", "two"]);
        assert_eq!(
            loaded
                .watches
                .watches
                .iter()
                .map(|watch| watch.watch_id.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert_eq!(tails, BTreeMap::from([("task".to_string(), None)]));
    }

    #[test]
    fn checkpoint_binds_revision_before_reset_and_skips_retained_wal() {
        let root = tempdir().unwrap();
        checkpoint(
            root.path(),
            [task("task", &["one"])],
            [watch("one", "task")],
        );
        let delta = add_watch_delta("task", &["one"], "two");
        append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::single(RegistryRevision::new(1)).unwrap(),
            &delta,
        )
        .unwrap();
        let loaded = load_task_watch_registry_with_deltas(root.path()).unwrap();
        let retained_wal = fs::read(registry_delta_wal_path(root.path())).unwrap();

        save_task_watch_registry_checkpoint_at_revision(
            root.path(),
            &loaded.tasks,
            &loaded.watches,
            loaded.replayed_revision,
        )
        .unwrap();
        let reset_wal = fs::read(registry_delta_wal_path(root.path())).unwrap();
        assert_eq!(reset_wal.len(), WAL_HEADER_BYTES);
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(daemon_dir(root.path()).join("task-watch-checkpoint-v1.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest["checkpoint"]["applied_delta_revision"],
            serde_json::Value::from(1)
        );

        // Model a crash after manifest commit but before WAL reset. Retained
        // frames are safe because replay skips the committed watermark.
        fs::write(registry_delta_wal_path(root.path()), retained_wal).unwrap();
        let restarted = load_task_watch_registry_with_deltas(root.path()).unwrap();
        assert_eq!(restarted.checkpoint_revision, RegistryRevision::new(1));
        assert_eq!(restarted.replayed_revision, RegistryRevision::new(1));
        assert_eq!(restarted.tasks.tasks["task"].watch_ids, vec!["one", "two"]);
    }

    #[test]
    fn legacy_unpaired_registries_load_at_revision_zero() {
        let root = tempdir().unwrap();
        let tasks = TaskRegistry {
            tasks: BTreeMap::from([("task".to_string(), task("task", &[]))]),
        };
        save_task_registry(root.path(), &tasks).unwrap();
        save_watch_registry(root.path(), &WatchRegistry::default()).unwrap();

        let loaded = load_task_watch_registry_with_deltas(root.path()).unwrap();

        assert!(loaded.tasks.tasks.contains_key("task"));
        assert_eq!(loaded.checkpoint_revision, RegistryRevision::ZERO);
        assert_eq!(loaded.replayed_revision, RegistryRevision::ZERO);
    }

    #[test]
    fn torn_final_frame_is_repaired_without_discarding_complete_frames() {
        let root = tempdir().unwrap();
        checkpoint(
            root.path(),
            [task("task", &["one"])],
            [watch("one", "task")],
        );
        let first = add_watch_delta("task", &["one"], "two");
        append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::single(RegistryRevision::new(1)).unwrap(),
            &first,
        )
        .unwrap();
        let complete_len = fs::metadata(registry_delta_wal_path(root.path()))
            .unwrap()
            .len();
        let second = add_watch_delta("task", &["one", "two"], "three");
        let encoded = encoded_frame(
            RegistryRevisionRange::single(RegistryRevision::new(2)).unwrap(),
            &second,
        );
        let torn_len = FRAME_HEADER_BYTES + 7;
        let mut wal = OpenOptions::new()
            .append(true)
            .open(registry_delta_wal_path(root.path()))
            .unwrap();
        wal.write_all(&encoded[..torn_len]).unwrap();
        wal.sync_all().unwrap();

        let loaded = load_task_watch_registry_with_deltas(root.path()).unwrap();

        assert_eq!(loaded.replayed_revision, RegistryRevision::new(1));
        assert_eq!(
            fs::metadata(registry_delta_wal_path(root.path()))
                .unwrap()
                .len(),
            complete_len
        );
    }

    #[test]
    fn complete_checksum_corruption_is_rejected_without_truncation() {
        let root = tempdir().unwrap();
        checkpoint(
            root.path(),
            [task("task", &["one"])],
            [watch("one", "task")],
        );
        let delta = add_watch_delta("task", &["one"], "two");
        append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::single(RegistryRevision::new(1)).unwrap(),
            &delta,
        )
        .unwrap();
        let path = registry_delta_wal_path(root.path());
        let original_len = fs::metadata(&path).unwrap().len();
        let mut wal = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        wal.seek(SeekFrom::Start(
            (WAL_HEADER_BYTES + FRAME_HEADER_BYTES) as u64,
        ))
        .unwrap();
        wal.write_all(b"!").unwrap();
        wal.sync_all().unwrap();

        let error = load_task_watch_registry_with_deltas(root.path()).unwrap_err();

        assert!(matches!(
            error,
            DaemonCoreError::InvalidRegistryDeltaWal { .. }
        ));
        assert_eq!(fs::metadata(path).unwrap().len(), original_len);
    }

    #[test]
    fn interior_checksum_corruption_is_rejected_without_truncation() {
        let root = tempdir().unwrap();
        checkpoint(root.path(), [], []);
        let batch = RegistryDeltaBatch::default();
        for revision in 1..=2 {
            append_task_watch_registry_delta(
                root.path(),
                RegistryRevisionRange::single(RegistryRevision::new(revision)).unwrap(),
                &batch,
            )
            .unwrap();
        }
        let path = registry_delta_wal_path(root.path());
        let original_len = fs::metadata(&path).unwrap().len();
        let mut wal = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        wal.seek(SeekFrom::Start(
            (WAL_HEADER_BYTES + FRAME_HEADER_BYTES) as u64,
        ))
        .unwrap();
        wal.write_all(b"!").unwrap();
        wal.sync_all().unwrap();

        let error = load_task_watch_registry_with_deltas(root.path()).unwrap_err();

        assert!(matches!(
            error,
            DaemonCoreError::InvalidRegistryDeltaWal { .. }
        ));
        assert_eq!(fs::metadata(path).unwrap().len(), original_len);
    }

    #[test]
    fn complete_revision_gap_is_rejected() {
        let root = tempdir().unwrap();
        checkpoint(root.path(), [], []);
        let path = registry_delta_wal_path(root.path());
        let batch = RegistryDeltaBatch::default();
        let frame = encoded_frame(
            RegistryRevisionRange::single(RegistryRevision::new(2)).unwrap(),
            &batch,
        );
        fs::write(
            &path,
            [
                encode_wal_header(RegistryRevision::ZERO).as_slice(),
                frame.as_slice(),
            ]
            .concat(),
        )
        .unwrap();

        let error = load_task_watch_registry_with_deltas(root.path()).unwrap_err();

        assert!(matches!(
            error,
            DaemonCoreError::InvalidRegistryDeltaWal { .. }
        ));
    }

    #[test]
    fn relationship_invalid_complete_frame_fails_closed() {
        let root = tempdir().unwrap();
        checkpoint(root.path(), [task("task", &[])], []);
        let delta = RegistryDeltaBatch {
            watch_upserts: BTreeMap::from([("orphan".to_string(), watch("orphan", "task"))]),
            watch_upsert_order: vec!["orphan".to_string()],
            ..RegistryDeltaBatch::default()
        };
        append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::single(RegistryRevision::new(1)).unwrap(),
            &delta,
        )
        .unwrap();

        let error = load_task_watch_registry_with_deltas(root.path()).unwrap_err();

        assert!(matches!(
            error,
            DaemonCoreError::InvalidRegistryDeltaWal { .. }
        ));
    }

    #[test]
    fn exact_tail_retry_is_idempotent_but_changed_payload_is_rejected() {
        let root = tempdir().unwrap();
        checkpoint(root.path(), [], []);
        let revisions = RegistryRevisionRange::single(RegistryRevision::new(1)).unwrap();
        let batch = RegistryDeltaBatch::default();
        append_task_watch_registry_delta(root.path(), revisions, &batch).unwrap();
        let path = registry_delta_wal_path(root.path());
        let committed_len = fs::metadata(&path).unwrap().len();

        append_task_watch_registry_delta(root.path(), revisions, &batch).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), committed_len);

        let changed = RegistryDeltaBatch {
            task_removals: BTreeSet::from(["missing".to_string()]),
            ..RegistryDeltaBatch::default()
        };
        let error = append_task_watch_registry_delta(root.path(), revisions, &changed).unwrap_err();
        assert!(matches!(
            error,
            DaemonCoreError::InvalidRegistryDeltaWal { .. }
        ));
        assert_eq!(fs::metadata(path).unwrap().len(), committed_len);
    }

    #[test]
    fn append_rejects_a_corrupted_previous_payload() {
        let root = tempdir().unwrap();
        checkpoint(root.path(), [], []);
        let batch = RegistryDeltaBatch::default();
        append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::single(RegistryRevision::new(1)).unwrap(),
            &batch,
        )
        .unwrap();
        let path = registry_delta_wal_path(root.path());
        let mut wal = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        wal.seek(SeekFrom::Start(
            (WAL_HEADER_BYTES + FRAME_HEADER_BYTES) as u64,
        ))
        .unwrap();
        wal.write_all(b"!").unwrap();
        wal.sync_all().unwrap();
        let corrupted_len = fs::metadata(&path).unwrap().len();

        let error = append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::single(RegistryRevision::new(2)).unwrap(),
            &batch,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            DaemonCoreError::InvalidRegistryDeltaWal { .. }
        ));
        assert_eq!(fs::metadata(path).unwrap().len(), corrupted_len);
    }

    #[test]
    fn concurrent_identical_retry_commits_one_frame() {
        let root = tempdir().unwrap();
        checkpoint(root.path(), [], []);
        let root_path = root.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(2));
        let batch = Arc::new(RegistryDeltaBatch::default());
        let revisions = RegistryRevisionRange::single(RegistryRevision::new(1)).unwrap();

        std::thread::scope(|scope| {
            for _ in 0..2 {
                let barrier = Arc::clone(&barrier);
                let batch = Arc::clone(&batch);
                let root_path = root_path.clone();
                scope.spawn(move || {
                    barrier.wait();
                    append_task_watch_registry_delta(&root_path, revisions, &batch).unwrap();
                });
            }
        });

        let payload_len = serde_json::to_vec(batch.as_ref()).unwrap().len();
        assert_eq!(
            fs::metadata(registry_delta_wal_path(root.path()))
                .unwrap()
                .len(),
            (WAL_HEADER_BYTES + FRAME_HEADER_BYTES + payload_len + FRAME_FOOTER_BYTES) as u64
        );
    }

    #[test]
    fn steady_state_append_reads_a_constant_size_tail() {
        let root = tempdir().unwrap();
        checkpoint(root.path(), [], []);
        let batch = RegistryDeltaBatch::default();
        for revision in 1..=64 {
            append_task_watch_registry_delta(
                root.path(),
                RegistryRevisionRange::single(RegistryRevision::new(revision)).unwrap(),
                &batch,
            )
            .unwrap();
        }
        FAST_TAIL_BYTES_READ.with(|observed| observed.set(0));

        append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::single(RegistryRevision::new(65)).unwrap(),
            &batch,
        )
        .unwrap();

        FAST_TAIL_BYTES_READ.with(|observed| {
            let payload_len = serde_json::to_vec(&batch).unwrap().len() as u64;
            assert_eq!(
                observed.get(),
                (WAL_HEADER_BYTES + FRAME_HEADER_BYTES + FRAME_FOOTER_BYTES) as u64 + payload_len
            );
        });
    }

    #[test]
    fn checkpoint_rejects_a_snapshot_with_different_watch_order() {
        let root = tempdir().unwrap();
        checkpoint(
            root.path(),
            [task("task", &["one", "two"])],
            [watch("one", "task"), watch("two", "task")],
        );
        let loaded = load_task_watch_registry_with_deltas(root.path()).unwrap();
        let mut reordered = loaded.watches.clone();
        reordered.watches.reverse();

        let error = save_task_watch_registry_checkpoint_at_revision(
            root.path(),
            &loaded.tasks,
            &reordered,
            loaded.replayed_revision,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            DaemonCoreError::InvalidRegistryDeltaBatch { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn wal_symlink_never_redirects_an_append() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let victim = tempdir().unwrap();
        checkpoint(root.path(), [], []);
        let victim_path = victim.path().join("victim.wal");
        fs::write(&victim_path, b"victim").unwrap();
        symlink(&victim_path, registry_delta_wal_path(root.path())).unwrap();

        let error = append_task_watch_registry_delta(
            root.path(),
            RegistryRevisionRange::single(RegistryRevision::new(1)).unwrap(),
            &RegistryDeltaBatch::default(),
        )
        .unwrap_err();

        assert!(matches!(error, DaemonCoreError::Io { .. }));
        assert_eq!(fs::read(victim_path).unwrap(), b"victim");
    }
}
