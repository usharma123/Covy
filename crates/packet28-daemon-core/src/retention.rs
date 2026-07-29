//! Safe inspection and bounded retention for workspace-local task state.
//!
//! Retention is dry-run unless [`RetentionOptions::apply`] is explicitly set.
//! Only task artifacts, event logs, and inactive task-registry records beneath
//! a real workspace-local `.packet28` directory are eligible. Symlinks,
//! unreadable entries, ambiguous task identifiers, active tasks, and state
//! owned by a running daemon are never removed.

use std::cmp::Ordering as CmpOrdering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use packet28_daemon_protocol::hooks::ActiveTaskRecord;
use packet28_daemon_protocol::paths::{
    active_task_path, agent_runtime_dir, daemon_dir, ready_path, task_artifact_dir,
    task_artifacts_dir, task_events_dir, task_registry_path,
};
use packet28_daemon_protocol::task::{TaskRecord, TaskRegistry};
use serde::{Deserialize, Serialize};

use crate::storage::remove_task_registry_records_if_unchanged;
use crate::task_store_lease::{
    task_store_lifecycle_lock_path, try_acquire_task_store_retention_lease,
};
use crate::{DaemonCoreError, Result};

/// Schema version for serialized [`TaskStoreReport`] values.
pub const TASK_STORE_REPORT_SCHEMA_VERSION: u32 = 1;

const STATE_DIR_NAME: &str = ".packet28";
const EVENT_SUFFIX: &str = ".events.jsonl";
const QUARANTINE_DIR_NAME: &str = ".retention-trash";
static QUARANTINE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Retention execution mode represented in a report.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetentionMode {
    /// Metrics-only inspection without a retention bound.
    Inspect,
    /// A bounded plan that did not mutate the store.
    DryRun,
    /// An explicitly requested cleanup attempt.
    Apply,
}

/// Bounds and execution mode for task-store retention.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionOptions {
    /// Maximum permitted age in seconds.
    ///
    /// Candidates exactly on the boundary are retained; only candidates older
    /// than this value are selected by age.
    pub max_age_seconds: Option<u64>,
    /// Maximum permitted logical bytes across task records, artifacts, and events.
    ///
    /// Candidates are selected oldest-first only while the store is strictly
    /// larger than this value.
    pub max_bytes: Option<u64>,
    /// Whether to apply the plan. `false` is always a non-mutating dry run.
    pub apply: bool,
}

impl RetentionOptions {
    /// Creates an unbounded metrics-only inspection.
    pub const fn inspect() -> Self {
        Self {
            max_age_seconds: None,
            max_bytes: None,
            apply: false,
        }
    }

    /// Creates a non-mutating retention plan.
    pub const fn dry_run(max_age_seconds: Option<u64>, max_bytes: Option<u64>) -> Self {
        Self {
            max_age_seconds,
            max_bytes,
            apply: false,
        }
    }

    /// Returns the same bounds with explicit deletion enabled.
    pub const fn apply(mut self) -> Self {
        self.apply = true;
        self
    }
}

/// Current logical-size and entry-count observations for a Packet28 state tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskStoreMetrics {
    /// Logical bytes beneath the entire workspace-local `.packet28` directory.
    pub state_logical_bytes: u64,
    /// Allocated filesystem bytes beneath `.packet28`.
    pub state_allocated_bytes: u64,
    /// Whether allocated-byte metrics use native filesystem block counts.
    ///
    /// When false, allocated-byte fields fall back to logical byte counts.
    pub allocated_bytes_supported: bool,
    /// Regular files beneath `.packet28`.
    pub state_files: u64,
    /// Directories beneath `.packet28`, including the state root.
    pub state_directories: u64,
    /// Symlinks encountered without following them.
    pub state_symlinks: u64,
    /// Actual bytes in the serialized task-registry file.
    pub task_registry_file_bytes: u64,
    /// Allocated filesystem bytes for the task-registry file.
    pub task_registry_allocated_bytes: u64,
    /// Successfully decoded records in the task registry.
    pub task_registry_records: u64,
    /// Whether the task registry was absent or decoded without error.
    pub task_registry_reliable: bool,
    /// Logical bytes under the task-artifact root.
    pub task_artifact_logical_bytes: u64,
    /// Allocated filesystem bytes under the task-artifact root.
    pub task_artifact_allocated_bytes: u64,
    /// Regular files under the task-artifact root.
    pub task_artifact_files: u64,
    /// Task-artifact directories, including the artifact root when present.
    pub task_artifact_directories: u64,
    /// Logical bytes under the daemon task-event root.
    pub task_event_logical_bytes: u64,
    /// Allocated filesystem bytes under the daemon task-event root.
    pub task_event_allocated_bytes: u64,
    /// Regular files under the task-event root.
    pub task_event_files: u64,
    /// Logical bytes governed by retention: compact task records, artifacts, and events.
    pub managed_task_logical_bytes: u64,
    /// Allocated bytes occupied by the registry file, artifacts, and events.
    pub managed_task_allocated_bytes: u64,
    /// Task records or active-task pointers currently protected as active.
    pub active_tasks: u64,
    /// Oldest known task record, artifact, or event timestamp.
    pub oldest_task_timestamp_unix: Option<u64>,
    /// Newest known task record, artifact, or event timestamp.
    pub newest_task_timestamp_unix: Option<u64>,
}

/// Why a task-store candidate was selected.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RetentionReason {
    /// The candidate is strictly older than the configured maximum age.
    AgeLimit,
    /// Removing the candidate is required to approach the configured byte limit.
    SizeLimit,
}

/// Result of one planned retention action.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetentionOutcome {
    /// The default dry run left the candidate unchanged.
    WouldRemove,
    /// Explicit apply removed the candidate.
    Removed,
    /// Revalidation protected a candidate that changed or became active.
    Skipped,
}

/// One task selected by the retention planner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetentionAction {
    /// Storage-safe task key used by artifact and event paths.
    pub storage_key: String,
    /// Original registry task identifiers associated with the storage key.
    pub task_ids: Vec<String>,
    /// Logical bytes expected to be reclaimed.
    pub logical_bytes: u64,
    /// Latest known modification timestamp.
    pub latest_timestamp_unix: Option<u64>,
    /// Bounds that selected this candidate.
    pub reasons: Vec<RetentionReason>,
    /// Dry-run or apply outcome.
    pub outcome: RetentionOutcome,
}

/// Non-fatal corruption, unreadable-entry, or safety observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskStoreIssue {
    /// Stable machine-readable issue category.
    pub kind: String,
    /// Affected path.
    pub path: String,
    /// Human-readable diagnostic.
    pub message: String,
}

/// Retention planning and cleanup accounting.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetentionAccounting {
    /// Configured maximum age.
    pub max_age_seconds: Option<u64>,
    /// Configured maximum logical bytes.
    pub max_bytes: Option<u64>,
    /// Candidates protected by active, ambiguous, corrupt, or unsafe state.
    pub protected_tasks: u64,
    /// Logical bytes associated with protected candidates.
    pub protected_logical_bytes: u64,
    /// Candidates selected by the planner.
    pub planned_tasks: u64,
    /// Logical bytes selected by the planner.
    pub planned_logical_bytes: u64,
    /// Candidates actually removed by explicit apply.
    pub removed_tasks: u64,
    /// Logical bytes removed by explicit apply.
    pub removed_logical_bytes: u64,
    /// Planned candidates skipped after revalidation.
    pub skipped_tasks: u64,
    /// Planned logical bytes after a dry run, or observed bytes after apply.
    pub remaining_managed_logical_bytes: u64,
    /// Bytes still above the configured size bound because they are protected.
    pub remaining_over_limit_bytes: u64,
}

/// Timestamped inspection and optional retention result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskStoreReport {
    /// Report schema version.
    pub schema_version: u32,
    /// Timestamp supplied by the caller for this observation.
    pub observed_at_unix: u64,
    /// Canonical workspace root.
    pub workspace_root: String,
    /// Validated workspace-local Packet28 state root.
    pub state_root: String,
    /// Inspection, dry-run, or explicit-apply mode.
    pub mode: RetentionMode,
    /// Metrics before retention planning or cleanup.
    pub metrics_before: TaskStoreMetrics,
    /// Metrics after cleanup, equal to `metrics_before` for inspect and dry-run.
    pub metrics_after: TaskStoreMetrics,
    /// Retention accounting.
    pub retention: RetentionAccounting,
    /// Deterministically ordered planned actions.
    pub actions: Vec<RetentionAction>,
    /// Non-fatal safety and corruption observations.
    pub issues: Vec<TaskStoreIssue>,
}

/// Inspects task storage without applying a retention bound.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the workspace or state root cannot be
/// inspected. Returns [`DaemonCoreError::UnsafeStateRoot`] if `.packet28` is a
/// symlink, is not a directory, or resolves outside the workspace.
pub fn inspect_task_store(root: &Path, observed_at_unix: u64) -> Result<TaskStoreReport> {
    retain_task_store(root, observed_at_unix, RetentionOptions::inspect())
}

/// Plans or explicitly applies bounded task-store retention.
///
/// Dry-run is the default. Actual deletion requires
/// [`RetentionOptions::apply`] to be `true`, a configured age or byte bound, a
/// stopped daemon, reliable active-task state, and a race-safe candidate
/// revalidation.
///
/// # Errors
///
/// Returns [`DaemonCoreError::InvalidRetentionPolicy`] when apply is requested
/// without a bound. Returns [`DaemonCoreError::UnsafeStateRoot`] for an unsafe
/// state root, [`DaemonCoreError::RetentionBlockedByDaemon`] while the daemon
/// readiness marker exists, [`DaemonCoreError::RetentionApplyUnsupported`] on
/// platforms without Unix file identity, or [`DaemonCoreError::Io`] for fatal
/// filesystem operations.
pub fn retain_task_store(
    root: &Path,
    observed_at_unix: u64,
    options: RetentionOptions,
) -> Result<TaskStoreReport> {
    retain_task_store_with_lease_observer(root, observed_at_unix, options, || {})
}

fn retain_task_store_with_lease_observer(
    root: &Path,
    observed_at_unix: u64,
    options: RetentionOptions,
    after_lease_acquired: impl FnOnce(),
) -> Result<TaskStoreReport> {
    if options.apply && options.max_age_seconds.is_none() && options.max_bytes.is_none() {
        return Err(DaemonCoreError::InvalidRetentionPolicy {
            message: "explicit apply requires max_age_seconds, max_bytes, or both",
        });
    }

    #[cfg(not(unix))]
    if options.apply {
        return Err(DaemonCoreError::RetentionApplyUnsupported);
    }

    let mut snapshot = StoreSnapshot::load(root, observed_at_unix)?;
    let mode = if options.max_age_seconds.is_none() && options.max_bytes.is_none() {
        RetentionMode::Inspect
    } else if options.apply {
        RetentionMode::Apply
    } else {
        RetentionMode::DryRun
    };
    let mut plan = build_plan(&snapshot, options);
    let metrics_before = snapshot.metrics.clone();
    let _task_store_lease =
        if options.apply && !plan.items.is_empty() {
            let lease = try_acquire_task_store_retention_lease(&snapshot.workspace_root)?
                .ok_or_else(|| DaemonCoreError::RetentionBlockedByDaemon {
                    path: task_store_lifecycle_lock_path(&snapshot.workspace_root),
                })?;
            after_lease_acquired();
            Some(lease)
        } else {
            None
        };

    if options.apply && !plan.items.is_empty() {
        let readiness = ready_path(&snapshot.workspace_root);
        if path_entry_exists(&readiness)? {
            return Err(DaemonCoreError::RetentionBlockedByDaemon { path: readiness });
        }
        apply_plan(&mut snapshot, &mut plan)?;
    }

    let metrics_after = if options.apply {
        StoreSnapshot::load(&snapshot.workspace_root, observed_at_unix)?.metrics
    } else {
        metrics_before.clone()
    };
    let remaining_managed_logical_bytes = if options.apply {
        metrics_after.managed_task_logical_bytes
    } else {
        plan.projected_managed_logical_bytes
    };
    let remaining_over_limit_bytes = options
        .max_bytes
        .map(|limit| remaining_managed_logical_bytes.saturating_sub(limit))
        .unwrap_or(0);
    let removed_tasks = plan
        .actions
        .iter()
        .filter(|action| action.outcome == RetentionOutcome::Removed)
        .count() as u64;
    let removed_logical_bytes = plan
        .actions
        .iter()
        .filter(|action| action.outcome == RetentionOutcome::Removed)
        .map(|action| action.logical_bytes)
        .sum();
    let skipped_tasks = plan
        .actions
        .iter()
        .filter(|action| action.outcome == RetentionOutcome::Skipped)
        .count() as u64;
    snapshot.issues.sort_by(|left, right| {
        (&left.kind, &left.path, &left.message).cmp(&(&right.kind, &right.path, &right.message))
    });
    snapshot.issues.dedup();

    Ok(TaskStoreReport {
        schema_version: TASK_STORE_REPORT_SCHEMA_VERSION,
        observed_at_unix,
        workspace_root: snapshot.workspace_root.display().to_string(),
        state_root: snapshot.state_root.display().to_string(),
        mode,
        metrics_before,
        metrics_after,
        retention: RetentionAccounting {
            max_age_seconds: options.max_age_seconds,
            max_bytes: options.max_bytes,
            protected_tasks: plan.protected_tasks,
            protected_logical_bytes: plan.protected_logical_bytes,
            planned_tasks: plan.actions.len() as u64,
            planned_logical_bytes: plan.actions.iter().map(|action| action.logical_bytes).sum(),
            removed_tasks,
            removed_logical_bytes,
            skipped_tasks,
            remaining_managed_logical_bytes,
            remaining_over_limit_bytes,
        },
        actions: plan.actions,
        issues: snapshot.issues,
    })
}

#[derive(Debug, Clone)]
struct StoreSnapshot {
    workspace_root: PathBuf,
    state_root: PathBuf,
    observed_at_unix: u64,
    metrics: TaskStoreMetrics,
    state_root_identity: Option<FileIdentity>,
    candidates: BTreeMap<String, Candidate>,
    issues: Vec<TaskStoreIssue>,
}

impl StoreSnapshot {
    fn load(root: &Path, observed_at_unix: u64) -> Result<Self> {
        let workspace_root = fs::canonicalize(root).map_err(|source| {
            DaemonCoreError::io("failed to resolve workspace root", root, source)
        })?;
        let state_root = workspace_root.join(STATE_DIR_NAME);
        match fs::symlink_metadata(&state_root) {
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    workspace_root,
                    state_root,
                    observed_at_unix,
                    metrics: TaskStoreMetrics {
                        task_registry_reliable: true,
                        allocated_bytes_supported: cfg!(unix),
                        ..TaskStoreMetrics::default()
                    },
                    state_root_identity: None,
                    candidates: BTreeMap::new(),
                    issues: Vec::new(),
                });
            }
            Err(source) => {
                return Err(DaemonCoreError::io(
                    "failed to inspect Packet28 state root",
                    &state_root,
                    source,
                ));
            }
        }

        let state_root = validate_state_root(&workspace_root, &state_root)?;
        let mut issues = Vec::new();
        let state_scan = scan_path(&state_root, &mut issues, "state_entry");
        let registry_snapshot = read_registry(&workspace_root, &mut issues)?;
        let active_snapshot = read_active_task(&workspace_root, &mut issues)?;
        let managed_layout_reliable =
            managed_layout_is_reliable(&workspace_root, &state_root, &mut issues);
        let mut candidates = BTreeMap::<String, Candidate>::new();

        add_registry_candidates(
            &workspace_root,
            &registry_snapshot.registry,
            &mut candidates,
        )?;
        add_artifact_candidates(&workspace_root, &mut candidates, &mut issues);
        add_event_candidates(&workspace_root, &mut candidates, &mut issues);

        let active_storage_keys = active_storage_keys(
            &workspace_root,
            &registry_snapshot.registry,
            active_snapshot.task_id.as_deref(),
        );
        let reliable_protection =
            registry_snapshot.reliable && active_snapshot.reliable && managed_layout_reliable;
        for candidate in candidates.values_mut() {
            if !reliable_protection {
                candidate.protected_reasons.insert(
                    "active-task, registry, or managed layout state is corrupt, unreadable, or unsafe"
                        .to_string(),
                );
            }
            if active_storage_keys.contains(&candidate.storage_key) {
                candidate
                    .protected_reasons
                    .insert("task is active".to_string());
            }
            if candidate.task_ids.len() > 1 {
                candidate
                    .protected_reasons
                    .insert("multiple task identifiers map to the same storage key".to_string());
            }
            if !candidate.safe {
                candidate
                    .protected_reasons
                    .insert("candidate contains an unsafe or unreadable entry".to_string());
            }
        }

        let artifact_scan = scan_path(
            &task_artifacts_dir(&workspace_root),
            &mut Vec::new(),
            "artifact_entry",
        );
        let event_scan = scan_path(
            &task_events_dir(&workspace_root),
            &mut Vec::new(),
            "event_entry",
        );
        let managed_task_logical_bytes = candidates
            .values()
            .map(Candidate::logical_bytes)
            .fold(0_u64, u64::saturating_add);
        let managed_task_allocated_bytes = registry_snapshot
            .allocated_bytes
            .saturating_add(artifact_scan.allocated_bytes)
            .saturating_add(event_scan.allocated_bytes);
        let active_tasks = active_storage_keys.len() as u64;
        let oldest_task_timestamp_unix = candidates
            .values()
            .filter_map(|candidate| candidate.latest_timestamp_unix)
            .min();
        let newest_task_timestamp_unix = candidates
            .values()
            .filter_map(|candidate| candidate.latest_timestamp_unix)
            .max();
        let metrics = TaskStoreMetrics {
            state_logical_bytes: state_scan.logical_bytes,
            state_allocated_bytes: state_scan.allocated_bytes,
            allocated_bytes_supported: cfg!(unix),
            state_files: state_scan.files,
            state_directories: state_scan.directories,
            state_symlinks: state_scan.symlinks,
            task_registry_file_bytes: registry_snapshot.file_bytes,
            task_registry_allocated_bytes: registry_snapshot.allocated_bytes,
            task_registry_records: registry_snapshot.registry.tasks.len() as u64,
            task_registry_reliable: registry_snapshot.reliable,
            task_artifact_logical_bytes: artifact_scan.logical_bytes,
            task_artifact_allocated_bytes: artifact_scan.allocated_bytes,
            task_artifact_files: artifact_scan.files,
            task_artifact_directories: artifact_scan.directories,
            task_event_logical_bytes: event_scan.logical_bytes,
            task_event_allocated_bytes: event_scan.allocated_bytes,
            task_event_files: event_scan.files,
            managed_task_logical_bytes,
            managed_task_allocated_bytes,
            active_tasks,
            oldest_task_timestamp_unix,
            newest_task_timestamp_unix,
        };
        issues.sort_by(|left, right| {
            (&left.kind, &left.path, &left.message).cmp(&(&right.kind, &right.path, &right.message))
        });
        issues.dedup();

        Ok(Self {
            workspace_root,
            state_root,
            observed_at_unix,
            metrics,
            state_root_identity: state_scan.identity,
            candidates,
            issues,
        })
    }
}

#[derive(Debug, Clone, Default)]
struct Candidate {
    storage_key: String,
    task_ids: Vec<String>,
    record_serializations: BTreeMap<String, Vec<u8>>,
    record_logical_bytes: u64,
    artifact: Option<ManagedComponent>,
    event: Option<ManagedComponent>,
    latest_timestamp_unix: Option<u64>,
    safe: bool,
    protected_reasons: BTreeSet<String>,
}

impl Candidate {
    fn new(storage_key: String) -> Self {
        Self {
            storage_key,
            safe: true,
            ..Self::default()
        }
    }

    fn logical_bytes(&self) -> u64 {
        self.record_logical_bytes
            .saturating_add(
                self.artifact
                    .as_ref()
                    .map_or(0, |component| component.scan.logical_bytes),
            )
            .saturating_add(
                self.event
                    .as_ref()
                    .map_or(0, |component| component.scan.logical_bytes),
            )
    }

    fn update_timestamp(&mut self, timestamp: Option<u64>) {
        self.latest_timestamp_unix = match (self.latest_timestamp_unix, timestamp) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (Some(value), None) | (None, Some(value)) => Some(value),
            (None, None) => None,
        };
    }
}

#[derive(Debug, Clone)]
struct ManagedComponent {
    path: PathBuf,
    scan: ScanSummary,
}

#[derive(Debug, Clone, Copy, Default)]
struct ScanSummary {
    logical_bytes: u64,
    allocated_bytes: u64,
    files: u64,
    directories: u64,
    symlinks: u64,
    latest_timestamp_unix: Option<u64>,
    metadata_fingerprint: [u8; 32],
    safe: bool,
    identity: Option<FileIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    length: u64,
    #[cfg(not(unix))]
    modified_unix_nanos: u128,
}

#[derive(Debug)]
struct RegistrySnapshot {
    registry: TaskRegistry,
    reliable: bool,
    file_bytes: u64,
    allocated_bytes: u64,
}

#[derive(Debug)]
struct ActiveTaskSnapshot {
    task_id: Option<String>,
    reliable: bool,
}

#[derive(Debug)]
struct TargetedCandidateSnapshot {
    candidate: Option<Candidate>,
    active_storage_keys: BTreeSet<String>,
    reliable: bool,
    issues: Vec<TaskStoreIssue>,
}

#[derive(Debug)]
struct RetentionPlan {
    items: Vec<PlanItem>,
    actions: Vec<RetentionAction>,
    protected_tasks: u64,
    protected_logical_bytes: u64,
    projected_managed_logical_bytes: u64,
}

#[derive(Debug, Clone)]
struct PlanItem {
    candidate: Candidate,
    reasons: BTreeSet<RetentionReason>,
}

fn validate_state_root(workspace_root: &Path, state_root: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(state_root).map_err(|source| {
        DaemonCoreError::io("failed to inspect Packet28 state root", state_root, source)
    })?;
    if metadata.file_type().is_symlink() {
        return Err(DaemonCoreError::UnsafeStateRoot {
            workspace_root: workspace_root.to_path_buf(),
            state_root: state_root.to_path_buf(),
            reason: "state root is a symlink",
        });
    }
    if !metadata.is_dir() {
        return Err(DaemonCoreError::UnsafeStateRoot {
            workspace_root: workspace_root.to_path_buf(),
            state_root: state_root.to_path_buf(),
            reason: "state root is not a directory",
        });
    }
    let canonical = fs::canonicalize(state_root).map_err(|source| {
        DaemonCoreError::io("failed to resolve Packet28 state root", state_root, source)
    })?;
    if !canonical.starts_with(workspace_root) {
        return Err(DaemonCoreError::UnsafeStateRoot {
            workspace_root: workspace_root.to_path_buf(),
            state_root: canonical,
            reason: "state root resolves outside the workspace",
        });
    }
    Ok(canonical)
}

fn scan_path(path: &Path, issues: &mut Vec<TaskStoreIssue>, issue_kind: &str) -> ScanSummary {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return ScanSummary {
                safe: true,
                ..ScanSummary::default()
            };
        }
        Err(source) => {
            push_issue(
                issues,
                issue_kind,
                path,
                format!("failed to inspect entry: {source}"),
            );
            return ScanSummary::default();
        }
    };
    let mut summary = ScanSummary {
        allocated_bytes: filesystem_allocated_bytes(&metadata),
        latest_timestamp_unix: modified_unix(&metadata),
        safe: true,
        identity: Some(file_identity(&metadata)),
        ..ScanSummary::default()
    };
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        0
    } else if metadata.is_file() {
        1
    } else if metadata.is_dir() {
        2
    } else {
        3
    };
    let mut fingerprint = metadata_hasher(&metadata, kind);
    if file_type.is_symlink() {
        summary.logical_bytes = metadata.len();
        summary.symlinks = 1;
        summary.safe = false;
        push_issue(
            issues,
            "symlink_entry",
            path,
            "symlinks are never followed or removed by retention".to_string(),
        );
        summary.metadata_fingerprint = *fingerprint.finalize().as_bytes();
        return summary;
    }
    if metadata.is_file() {
        summary.logical_bytes = metadata.len();
        summary.files = 1;
        summary.metadata_fingerprint = *fingerprint.finalize().as_bytes();
        return summary;
    }
    if !metadata.is_dir() {
        summary.logical_bytes = metadata.len();
        summary.safe = false;
        push_issue(
            issues,
            "special_entry",
            path,
            "non-file, non-directory entry is not eligible for retention".to_string(),
        );
        summary.metadata_fingerprint = *fingerprint.finalize().as_bytes();
        return summary;
    }

    summary.directories = 1;
    let directory_entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(source) => {
            summary.safe = false;
            push_issue(
                issues,
                "unreadable_entry",
                path,
                format!("failed to enumerate directory: {source}"),
            );
            summary.metadata_fingerprint = *fingerprint.finalize().as_bytes();
            return summary;
        }
    };
    let mut entries = Vec::new();
    for entry in directory_entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(source) => {
                summary.safe = false;
                push_issue(
                    issues,
                    "unreadable_entry",
                    path,
                    format!("failed to enumerate directory entry: {source}"),
                );
                continue;
            }
        };
        entries.push(entry);
    }
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let child = scan_path(&entry.path(), issues, issue_kind);
        let encoded_name = name.as_encoded_bytes();
        fingerprint.update(&(encoded_name.len() as u64).to_le_bytes());
        fingerprint.update(encoded_name);
        fingerprint.update(&child.metadata_fingerprint);
        if !same_device(summary.identity, child.identity) {
            summary.safe = false;
            push_issue(
                issues,
                "cross_device_entry",
                &entry.path(),
                "entries on another filesystem are not eligible for retention".to_string(),
            );
        }
        summary.logical_bytes = summary.logical_bytes.saturating_add(child.logical_bytes);
        summary.allocated_bytes = summary
            .allocated_bytes
            .saturating_add(child.allocated_bytes);
        summary.files = summary.files.saturating_add(child.files);
        summary.directories = summary.directories.saturating_add(child.directories);
        summary.symlinks = summary.symlinks.saturating_add(child.symlinks);
        summary.latest_timestamp_unix =
            latest_timestamp(summary.latest_timestamp_unix, child.latest_timestamp_unix);
        summary.safe &= child.safe;
    }
    summary.metadata_fingerprint = *fingerprint.finalize().as_bytes();
    summary
}

#[cfg(unix)]
fn filesystem_allocated_bytes(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn filesystem_allocated_bytes(metadata: &fs::Metadata) -> u64 {
    if metadata.is_dir() {
        0
    } else {
        metadata.len()
    }
}

#[cfg(unix)]
fn same_device(parent: Option<FileIdentity>, child: Option<FileIdentity>) -> bool {
    matches!(
        (parent, child),
        (Some(parent), Some(child)) if parent.device == child.device
    )
}

#[cfg(not(unix))]
fn same_device(_parent: Option<FileIdentity>, _child: Option<FileIdentity>) -> bool {
    true
}

fn metadata_hasher(metadata: &fs::Metadata, kind: u8) -> blake3::Hasher {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"packet28-retention-metadata-v1");
    hasher.update(&[kind]);
    hasher.update(&metadata.len().to_le_bytes());
    match modified_unix_nanos(metadata) {
        Some(timestamp) => {
            hasher.update(&[1]);
            hasher.update(&timestamp.to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    hash_file_identity(&mut hasher, file_identity(metadata));
    hasher
}

#[cfg(unix)]
fn hash_file_identity(hasher: &mut blake3::Hasher, identity: FileIdentity) {
    hasher.update(&identity.device.to_le_bytes());
    hasher.update(&identity.inode.to_le_bytes());
}

#[cfg(not(unix))]
fn hash_file_identity(hasher: &mut blake3::Hasher, identity: FileIdentity) {
    hasher.update(&identity.length.to_le_bytes());
    hasher.update(&identity.modified_unix_nanos.to_le_bytes());
}

fn read_registry(root: &Path, issues: &mut Vec<TaskStoreIssue>) -> Result<RegistrySnapshot> {
    let path = task_registry_path(root);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RegistrySnapshot {
                registry: TaskRegistry::default(),
                reliable: true,
                file_bytes: 0,
                allocated_bytes: 0,
            });
        }
        Err(source) => {
            push_issue(
                issues,
                "registry_unreadable",
                &path,
                format!("failed to inspect task registry: {source}"),
            );
            return Ok(RegistrySnapshot {
                registry: TaskRegistry::default(),
                reliable: false,
                file_bytes: 0,
                allocated_bytes: 0,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        push_issue(
            issues,
            "registry_unsafe",
            &path,
            "task registry is not a regular file".to_string(),
        );
        return Ok(RegistrySnapshot {
            registry: TaskRegistry::default(),
            reliable: false,
            file_bytes: metadata.len(),
            allocated_bytes: filesystem_allocated_bytes(&metadata),
        });
    }
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(source) => {
            push_issue(
                issues,
                "registry_unreadable",
                &path,
                format!("failed to read task registry: {source}"),
            );
            return Ok(RegistrySnapshot {
                registry: TaskRegistry::default(),
                reliable: false,
                file_bytes: metadata.len(),
                allocated_bytes: filesystem_allocated_bytes(&metadata),
            });
        }
    };
    match serde_json::from_slice(&raw) {
        Ok(registry) => Ok(RegistrySnapshot {
            registry,
            reliable: true,
            file_bytes: raw.len() as u64,
            allocated_bytes: filesystem_allocated_bytes(&metadata),
        }),
        Err(source) => {
            push_issue(
                issues,
                "registry_corrupt",
                &path,
                format!("failed to decode task registry: {source}"),
            );
            Ok(RegistrySnapshot {
                registry: TaskRegistry::default(),
                reliable: false,
                file_bytes: raw.len() as u64,
                allocated_bytes: filesystem_allocated_bytes(&metadata),
            })
        }
    }
}

fn read_active_task(root: &Path, issues: &mut Vec<TaskStoreIssue>) -> Result<ActiveTaskSnapshot> {
    let path = active_task_path(root);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ActiveTaskSnapshot {
                task_id: None,
                reliable: true,
            });
        }
        Err(source) => {
            push_issue(
                issues,
                "active_task_unreadable",
                &path,
                format!("failed to inspect active-task record: {source}"),
            );
            return Ok(ActiveTaskSnapshot {
                task_id: None,
                reliable: false,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        push_issue(
            issues,
            "active_task_unsafe",
            &path,
            "active-task record is not a regular file".to_string(),
        );
        return Ok(ActiveTaskSnapshot {
            task_id: None,
            reliable: false,
        });
    }
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(source) => {
            push_issue(
                issues,
                "active_task_unreadable",
                &path,
                format!("failed to read active-task record: {source}"),
            );
            return Ok(ActiveTaskSnapshot {
                task_id: None,
                reliable: false,
            });
        }
    };
    let record = match serde_json::from_slice::<ActiveTaskRecord>(&raw) {
        Ok(record) => record,
        Err(source) => {
            push_issue(
                issues,
                "active_task_corrupt",
                &path,
                format!("failed to decode active-task record: {source}"),
            );
            return Ok(ActiveTaskSnapshot {
                task_id: None,
                reliable: false,
            });
        }
    };
    let task_id = record.task_id.trim();
    if task_id.is_empty() {
        push_issue(
            issues,
            "active_task_corrupt",
            &path,
            "active-task record has an empty task identifier".to_string(),
        );
        return Ok(ActiveTaskSnapshot {
            task_id: None,
            reliable: false,
        });
    }
    Ok(ActiveTaskSnapshot {
        task_id: Some(task_id.to_string()),
        reliable: true,
    })
}

fn add_registry_candidates(
    root: &Path,
    registry: &TaskRegistry,
    candidates: &mut BTreeMap<String, Candidate>,
) -> Result<()> {
    let registry_path = task_registry_path(root);
    for (task_id, record) in &registry.tasks {
        let storage_key = storage_key_for_task(root, task_id);
        let record_bytes = serde_json::to_vec(record).map_err(|source| {
            DaemonCoreError::json(
                "failed to measure task registry record for",
                &registry_path,
                source,
            )
        })?;
        let candidate = candidates
            .entry(storage_key.clone())
            .or_insert_with(|| Candidate::new(storage_key));
        candidate.task_ids.push(task_id.clone());
        candidate
            .record_serializations
            .insert(task_id.clone(), record_bytes.clone());
        candidate.record_logical_bytes = candidate
            .record_logical_bytes
            .saturating_add(record_bytes.len() as u64);
        candidate.update_timestamp(latest_record_timestamp(record));
    }
    Ok(())
}

fn add_artifact_candidates(
    root: &Path,
    candidates: &mut BTreeMap<String, Candidate>,
    issues: &mut Vec<TaskStoreIssue>,
) {
    let artifact_root = task_artifacts_dir(root);
    let entries = match read_managed_root(&artifact_root, issues, "artifact_root") {
        Some(entries) => entries,
        None => return,
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(source) => {
                push_issue(
                    issues,
                    "artifact_entry_unreadable",
                    &artifact_root,
                    format!("failed to enumerate task artifact: {source}"),
                );
                continue;
            }
        };
        let path = entry.path();
        let Some(storage_key) = entry.file_name().to_str().map(str::to_string) else {
            push_issue(
                issues,
                "artifact_name_invalid",
                &path,
                "non-UTF-8 task artifact names are protected".to_string(),
            );
            continue;
        };
        let scan = scan_path(&path, issues, "artifact_entry_unreadable");
        let expected_directory = fs::symlink_metadata(&path)
            .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
            .unwrap_or(false);
        let candidate = candidates
            .entry(storage_key.clone())
            .or_insert_with(|| Candidate::new(storage_key));
        candidate.update_timestamp(scan.latest_timestamp_unix);
        candidate.safe &= scan.safe && expected_directory;
        candidate.artifact = Some(ManagedComponent { path, scan });
    }
}

fn add_event_candidates(
    root: &Path,
    candidates: &mut BTreeMap<String, Candidate>,
    issues: &mut Vec<TaskStoreIssue>,
) {
    let event_root = task_events_dir(root);
    let entries = match read_managed_root(&event_root, issues, "event_root") {
        Some(entries) => entries,
        None => return,
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(source) => {
                push_issue(
                    issues,
                    "event_entry_unreadable",
                    &event_root,
                    format!("failed to enumerate task event: {source}"),
                );
                continue;
            }
        };
        let path = entry.path();
        let file_name = entry.file_name();
        let (storage_key, recognized) = file_name
            .to_str()
            .and_then(|name| name.strip_suffix(EVENT_SUFFIX))
            .map_or_else(
                || (file_name.to_string_lossy().into_owned(), false),
                |key| (key.to_string(), true),
            );
        if !recognized {
            push_issue(
                issues,
                "event_name_invalid",
                &path,
                "unrecognized task-event entry is protected".to_string(),
            );
        }
        let scan = scan_path(&path, issues, "event_entry_unreadable");
        let expected_file = fs::symlink_metadata(&path)
            .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            .unwrap_or(false);
        let candidate = candidates
            .entry(storage_key.clone())
            .or_insert_with(|| Candidate::new(storage_key));
        candidate.update_timestamp(scan.latest_timestamp_unix);
        candidate.safe &= scan.safe && expected_file && recognized;
        candidate.event = Some(ManagedComponent { path, scan });
    }
}

fn load_targeted_candidate(
    snapshot: &StoreSnapshot,
    storage_key: &str,
) -> Result<TargetedCandidateSnapshot> {
    let current_state_root = validate_state_root(
        &snapshot.workspace_root,
        &snapshot.workspace_root.join(STATE_DIR_NAME),
    )?;
    let current_identity = fs::symlink_metadata(&current_state_root)
        .map(|metadata| file_identity(&metadata))
        .map_err(|source| {
            DaemonCoreError::io(
                "failed to revalidate Packet28 state root",
                &current_state_root,
                source,
            )
        })?;
    if current_state_root != snapshot.state_root
        || snapshot.state_root_identity != Some(current_identity)
    {
        return Err(DaemonCoreError::RetentionCandidateChanged {
            path: current_state_root,
        });
    }

    let mut issues = Vec::new();
    if !storage_key_is_safe(storage_key) {
        push_issue(
            &mut issues,
            "storage_key_invalid",
            &snapshot.state_root,
            "retention candidate storage key is not one path component".to_string(),
        );
        return Ok(TargetedCandidateSnapshot {
            candidate: None,
            active_storage_keys: BTreeSet::new(),
            reliable: false,
            issues,
        });
    }

    let registry_snapshot = read_registry(&snapshot.workspace_root, &mut issues)?;
    let active_snapshot = read_active_task(&snapshot.workspace_root, &mut issues)?;
    let managed_layout_reliable =
        managed_layout_is_reliable(&snapshot.workspace_root, &snapshot.state_root, &mut issues);
    let mut candidate = Candidate::new(storage_key.to_string());
    let mut present = false;
    let registry_path = task_registry_path(&snapshot.workspace_root);
    for (task_id, record) in &registry_snapshot.registry.tasks {
        if storage_key_for_task(&snapshot.workspace_root, task_id) != storage_key {
            continue;
        }
        let record_bytes = serde_json::to_vec(record).map_err(|source| {
            DaemonCoreError::json(
                "failed to verify task registry record in",
                &registry_path,
                source,
            )
        })?;
        candidate.task_ids.push(task_id.clone());
        candidate
            .record_serializations
            .insert(task_id.clone(), record_bytes.clone());
        candidate.record_logical_bytes = candidate
            .record_logical_bytes
            .saturating_add(record_bytes.len() as u64);
        candidate.update_timestamp(latest_record_timestamp(record));
        present = true;
    }

    let artifact_path = task_artifacts_dir(&snapshot.workspace_root).join(storage_key);
    if path_entry_exists(&artifact_path)? {
        let scan = scan_path(&artifact_path, &mut issues, "artifact_entry_unreadable");
        let expected_directory = fs::symlink_metadata(&artifact_path)
            .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
            .unwrap_or(false);
        candidate.update_timestamp(scan.latest_timestamp_unix);
        candidate.safe &= scan.safe && expected_directory;
        candidate.artifact = Some(ManagedComponent {
            path: artifact_path,
            scan,
        });
        present = true;
    }

    let event_path =
        task_events_dir(&snapshot.workspace_root).join(format!("{storage_key}{EVENT_SUFFIX}"));
    if path_entry_exists(&event_path)? {
        let scan = scan_path(&event_path, &mut issues, "event_entry_unreadable");
        let expected_file = fs::symlink_metadata(&event_path)
            .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            .unwrap_or(false);
        candidate.update_timestamp(scan.latest_timestamp_unix);
        candidate.safe &= scan.safe && expected_file;
        candidate.event = Some(ManagedComponent {
            path: event_path,
            scan,
        });
        present = true;
    }

    let active_storage_keys = active_storage_keys(
        &snapshot.workspace_root,
        &registry_snapshot.registry,
        active_snapshot.task_id.as_deref(),
    );
    let reliable =
        registry_snapshot.reliable && active_snapshot.reliable && managed_layout_reliable;
    if !reliable {
        candidate.protected_reasons.insert(
            "active-task, registry, or managed layout state is corrupt, unreadable, or unsafe"
                .to_string(),
        );
    }
    if active_storage_keys.contains(storage_key) {
        candidate
            .protected_reasons
            .insert("task is active".to_string());
    }
    if candidate.task_ids.len() > 1 {
        candidate
            .protected_reasons
            .insert("multiple task identifiers map to the same storage key".to_string());
    }
    if !candidate.safe {
        candidate
            .protected_reasons
            .insert("candidate contains an unsafe or unreadable entry".to_string());
    }
    issues.sort_by(|left, right| {
        (&left.kind, &left.path, &left.message).cmp(&(&right.kind, &right.path, &right.message))
    });
    issues.dedup();

    Ok(TargetedCandidateSnapshot {
        candidate: present.then_some(candidate),
        active_storage_keys,
        reliable,
        issues,
    })
}

fn managed_layout_is_reliable(
    workspace_root: &Path,
    state_root: &Path,
    issues: &mut Vec<TaskStoreIssue>,
) -> bool {
    let daemon_root = daemon_dir(workspace_root);
    let agent_root = agent_runtime_dir(workspace_root);
    let mut reliable = true;
    for (path, issue_kind) in [
        (task_artifacts_dir(workspace_root), "artifact_root"),
        (daemon_root, "daemon_root"),
        (task_events_dir(workspace_root), "event_root"),
        (agent_root, "agent_root"),
    ] {
        reliable &= optional_state_directory_is_reliable(&path, state_root, issues, issue_kind);
    }
    reliable
}

fn optional_state_directory_is_reliable(
    path: &Path,
    state_root: &Path,
    issues: &mut Vec<TaskStoreIssue>,
    issue_kind: &str,
) -> bool {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return true,
        Err(source) => {
            push_issue(
                issues,
                issue_kind,
                path,
                format!("failed to inspect managed directory: {source}"),
            );
            return false;
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        push_issue(
            issues,
            issue_kind,
            path,
            "managed directory is not a real directory".to_string(),
        );
        return false;
    }
    match fs::canonicalize(path) {
        Ok(canonical) if canonical.starts_with(state_root) => true,
        Ok(canonical) => {
            push_issue(
                issues,
                issue_kind,
                &canonical,
                "managed directory resolves outside Packet28 state".to_string(),
            );
            false
        }
        Err(source) => {
            push_issue(
                issues,
                issue_kind,
                path,
                format!("failed to resolve managed directory: {source}"),
            );
            false
        }
    }
}

fn storage_key_is_safe(storage_key: &str) -> bool {
    let mut components = Path::new(storage_key).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn path_entry_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(DaemonCoreError::io(
            "failed to inspect retention candidate",
            path,
            source,
        )),
    }
}

fn read_managed_root(
    root: &Path,
    issues: &mut Vec<TaskStoreIssue>,
    issue_kind: &str,
) -> Option<fs::ReadDir> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return None,
        Err(source) => {
            push_issue(
                issues,
                issue_kind,
                root,
                format!("failed to inspect managed root: {source}"),
            );
            return None;
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        push_issue(
            issues,
            issue_kind,
            root,
            "managed root is not a real directory".to_string(),
        );
        return None;
    }
    match fs::read_dir(root) {
        Ok(entries) => Some(entries),
        Err(source) => {
            push_issue(
                issues,
                issue_kind,
                root,
                format!("failed to enumerate managed root: {source}"),
            );
            None
        }
    }
}

fn active_storage_keys(
    root: &Path,
    registry: &TaskRegistry,
    active_task_id: Option<&str>,
) -> BTreeSet<String> {
    let mut active = registry
        .tasks
        .iter()
        .filter(|(_, record)| task_record_is_active(record))
        .map(|(task_id, _)| storage_key_for_task(root, task_id))
        .collect::<BTreeSet<_>>();
    if let Some(task_id) = active_task_id {
        active.insert(storage_key_for_task(root, task_id));
    }
    active
}

fn task_record_is_active(record: &TaskRecord) -> bool {
    if record.lifecycle.is_running()
        || record.lifecycle.is_cancelling()
        || record.lifecycle.has_pending_replan()
    {
        return true;
    }
    match (
        record.latest_agent_started_at_unix,
        record.latest_agent_completed_at_unix,
    ) {
        (Some(started), Some(completed)) => {
            normalize_timestamp_seconds(completed) < normalize_timestamp_seconds(started)
        }
        (Some(_), None) => true,
        _ => false,
    }
}

fn latest_record_timestamp(record: &TaskRecord) -> Option<u64> {
    [
        record.last_started_at_unix,
        record.last_completed_at_unix,
        record.last_replan_at_unix,
        record.last_context_refresh_at_unix,
        record.latest_brief_generated_at_unix,
        record.latest_handoff_generated_at_unix,
        record.latest_agent_started_at_unix,
        record.latest_agent_completed_at_unix,
        record.latest_hook_event_at_unix,
        record.latest_hook_boundary_at_unix,
        record.latest_hook_bootstrap_at_unix,
        record.latest_hook_progress_at_unix,
    ]
    .into_iter()
    .flatten()
    .map(normalize_timestamp_seconds)
    .max()
}

fn normalize_timestamp_seconds(value: u64) -> u64 {
    // Persisted task fields historically mix Unix seconds and milliseconds.
    // This threshold matches the daemon's compatibility normalization: values
    // this large cannot be contemporary Unix seconds.
    if value < 100_000_000_000 {
        value
    } else {
        value / 1_000
    }
}

fn storage_key_for_task(root: &Path, task_id: &str) -> String {
    task_artifact_dir(root, task_id)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("task")
        .to_string()
}

fn build_plan(snapshot: &StoreSnapshot, options: RetentionOptions) -> RetentionPlan {
    let mut selected = BTreeMap::<String, PlanItem>::new();
    let mut removable = snapshot
        .candidates
        .values()
        .filter(|candidate| candidate.protected_reasons.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    removable.sort_by(compare_candidate_age);

    if let Some(max_age_seconds) = options.max_age_seconds {
        for candidate in &removable {
            let is_older = candidate.latest_timestamp_unix.is_some_and(|timestamp| {
                snapshot.observed_at_unix.saturating_sub(timestamp) > max_age_seconds
            });
            if is_older {
                selected.insert(
                    candidate.storage_key.clone(),
                    PlanItem {
                        candidate: candidate.clone(),
                        reasons: BTreeSet::from([RetentionReason::AgeLimit]),
                    },
                );
            }
        }
    }

    let mut projected_managed_logical_bytes = snapshot.metrics.managed_task_logical_bytes;
    for item in selected.values() {
        projected_managed_logical_bytes =
            projected_managed_logical_bytes.saturating_sub(item.candidate.logical_bytes());
    }
    if let Some(max_bytes) = options.max_bytes {
        for candidate in &removable {
            if projected_managed_logical_bytes <= max_bytes {
                break;
            }
            if let Some(item) = selected.get_mut(&candidate.storage_key) {
                item.reasons.insert(RetentionReason::SizeLimit);
                continue;
            }
            let logical_bytes = candidate.logical_bytes();
            if logical_bytes == 0 {
                continue;
            }
            selected.insert(
                candidate.storage_key.clone(),
                PlanItem {
                    candidate: candidate.clone(),
                    reasons: BTreeSet::from([RetentionReason::SizeLimit]),
                },
            );
            projected_managed_logical_bytes =
                projected_managed_logical_bytes.saturating_sub(logical_bytes);
        }
    }

    let mut items = selected.into_values().collect::<Vec<_>>();
    items.sort_by(|left, right| compare_candidate_age(&left.candidate, &right.candidate));
    let actions = items
        .iter()
        .map(|item| RetentionAction {
            storage_key: item.candidate.storage_key.clone(),
            task_ids: item.candidate.task_ids.clone(),
            logical_bytes: item.candidate.logical_bytes(),
            latest_timestamp_unix: item.candidate.latest_timestamp_unix,
            reasons: item.reasons.iter().copied().collect(),
            outcome: RetentionOutcome::WouldRemove,
        })
        .collect();
    let protected_tasks = snapshot
        .candidates
        .values()
        .filter(|candidate| !candidate.protected_reasons.is_empty())
        .count() as u64;
    let protected_logical_bytes = snapshot
        .candidates
        .values()
        .filter(|candidate| !candidate.protected_reasons.is_empty())
        .map(Candidate::logical_bytes)
        .fold(0_u64, u64::saturating_add);

    RetentionPlan {
        items,
        actions,
        protected_tasks,
        protected_logical_bytes,
        projected_managed_logical_bytes,
    }
}

fn compare_candidate_age(left: &Candidate, right: &Candidate) -> CmpOrdering {
    match (left.latest_timestamp_unix, right.latest_timestamp_unix) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => CmpOrdering::Less,
        (None, Some(_)) => CmpOrdering::Greater,
        (None, None) => CmpOrdering::Equal,
    }
    .then_with(|| left.storage_key.cmp(&right.storage_key))
}

#[cfg(unix)]
fn apply_plan(snapshot: &mut StoreSnapshot, plan: &mut RetentionPlan) -> Result<()> {
    let quarantine_root = snapshot.state_root.join(QUARANTINE_DIR_NAME);
    for (index, item) in plan.items.iter().enumerate() {
        let readiness = ready_path(&snapshot.workspace_root);
        if path_entry_exists(&readiness)? {
            return Err(DaemonCoreError::RetentionBlockedByDaemon { path: readiness });
        }
        let current = match load_targeted_candidate(snapshot, &item.candidate.storage_key) {
            Ok(current) => current,
            Err(DaemonCoreError::RetentionCandidateChanged { path }) => {
                snapshot.issues.push(TaskStoreIssue {
                    kind: "candidate_changed".to_string(),
                    path: path.display().to_string(),
                    message: "Packet28 state root changed after inspection".to_string(),
                });
                if let Some(action) = plan.actions.get_mut(index) {
                    action.outcome = RetentionOutcome::Skipped;
                }
                continue;
            }
            Err(error) => return Err(error),
        };
        snapshot.issues.extend(current.issues);
        let issue_count_before_outcome = snapshot.issues.len();
        let outcome = match current.candidate.as_ref() {
            Some(candidate)
                if candidate.protected_reasons.is_empty()
                    && candidate_matches(&item.candidate, candidate) =>
            {
                match apply_candidate(snapshot, candidate, &quarantine_root) {
                    Ok(outcome) => outcome,
                    Err(DaemonCoreError::RetentionCandidateChanged { path }) => {
                        snapshot.issues.push(TaskStoreIssue {
                            kind: "candidate_changed".to_string(),
                            path: path.display().to_string(),
                            message: "candidate identity changed during cleanup".to_string(),
                        });
                        RetentionOutcome::Skipped
                    }
                    Err(error) => return Err(error),
                }
            }
            _ => {
                snapshot.issues.push(TaskStoreIssue {
                    kind: "candidate_changed".to_string(),
                    path: item
                        .candidate
                        .artifact
                        .as_ref()
                        .or(item.candidate.event.as_ref())
                        .map(|component| component.path.display().to_string())
                        .unwrap_or_else(|| {
                            task_registry_path(&snapshot.workspace_root)
                                .display()
                                .to_string()
                        }),
                    message: "candidate changed or became protected after inspection".to_string(),
                });
                RetentionOutcome::Skipped
            }
        };
        if outcome == RetentionOutcome::Skipped
            && snapshot.issues.len() == issue_count_before_outcome
        {
            snapshot.issues.push(TaskStoreIssue {
                kind: "candidate_changed".to_string(),
                path: item
                    .candidate
                    .artifact
                    .as_ref()
                    .or(item.candidate.event.as_ref())
                    .map(|component| component.path.display().to_string())
                    .unwrap_or_else(|| {
                        task_registry_path(&snapshot.workspace_root)
                            .display()
                            .to_string()
                    }),
                message: "candidate changed or became active during cleanup".to_string(),
            });
        }
        if let Some(action) = plan.actions.get_mut(index) {
            action.outcome = outcome;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_plan(_snapshot: &mut StoreSnapshot, _plan: &mut RetentionPlan) -> Result<()> {
    Err(DaemonCoreError::RetentionApplyUnsupported)
}

#[cfg(unix)]
fn apply_candidate(
    snapshot: &StoreSnapshot,
    candidate: &Candidate,
    quarantine_root: &Path,
) -> Result<RetentionOutcome> {
    ensure_quarantine_root(&snapshot.state_root, quarantine_root)?;
    let group = quarantine_root.join(format!(
        "task-{}-{}",
        std::process::id(),
        QUARANTINE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&group).map_err(|source| {
        DaemonCoreError::io("failed to create retention quarantine", &group, source)
    })?;

    let mut staged = Vec::<StagedComponent>::new();
    let stage_result = (|| -> Result<()> {
        if let Some(component) = &candidate.artifact {
            stage_component(
                &snapshot.state_root,
                component,
                &group.join("artifacts"),
                &mut staged,
            )?;
        }
        if let Some(component) = &candidate.event {
            stage_component(
                &snapshot.state_root,
                component,
                &group.join("events.jsonl"),
                &mut staged,
            )?;
        }
        Ok(())
    })();
    if let Err(error) = stage_result {
        rollback_staged(&staged)?;
        remove_empty_dir(&group)?;
        return Err(error);
    }

    if !candidate_remains_safe_after_staging(snapshot, candidate)? {
        rollback_staged(&staged)?;
        remove_empty_dir(&group)?;
        return Ok(RetentionOutcome::Skipped);
    }

    let readiness = ready_path(&snapshot.workspace_root);
    if path_entry_exists(&readiness)? {
        rollback_staged(&staged)?;
        remove_empty_dir(&group)?;
        return Err(DaemonCoreError::RetentionBlockedByDaemon { path: readiness });
    }

    if !candidate.record_serializations.is_empty() {
        let removed = remove_task_registry_records_if_unchanged(
            &snapshot.workspace_root,
            &candidate.record_serializations,
        );
        let removed = match removed {
            Ok(removed) => removed,
            Err(error) => {
                rollback_staged(&staged)?;
                remove_empty_dir(&group)?;
                return Err(error);
            }
        };
        if !removed {
            rollback_staged(&staged)?;
            remove_empty_dir(&group)?;
            return Ok(RetentionOutcome::Skipped);
        }
    }

    fs::remove_dir_all(&group).map_err(|source| {
        DaemonCoreError::io("failed to remove retention quarantine", &group, source)
    })?;
    remove_empty_dir(quarantine_root)?;
    Ok(RetentionOutcome::Removed)
}

#[cfg(unix)]
fn candidate_remains_safe_after_staging(
    snapshot: &StoreSnapshot,
    candidate: &Candidate,
) -> Result<bool> {
    let latest = load_targeted_candidate(snapshot, &candidate.storage_key)?;
    Ok(match latest.candidate.as_ref() {
        Some(current) => {
            latest.reliable
                && current.protected_reasons.is_empty()
                && current.task_ids == candidate.task_ids
                && current.record_serializations == candidate.record_serializations
                && current.artifact.is_none()
                && current.event.is_none()
        }
        None => {
            latest.reliable
                && candidate.task_ids.is_empty()
                && !latest.active_storage_keys.contains(&candidate.storage_key)
        }
    })
}

#[cfg(unix)]
#[derive(Debug)]
struct StagedComponent {
    original: PathBuf,
    staged: PathBuf,
}

#[cfg(unix)]
fn stage_component(
    state_root: &Path,
    component: &ManagedComponent,
    destination: &Path,
    staged: &mut Vec<StagedComponent>,
) -> Result<()> {
    let canonical = fs::canonicalize(&component.path).map_err(|source| {
        DaemonCoreError::io(
            "failed to resolve retention candidate",
            &component.path,
            source,
        )
    })?;
    if !canonical.starts_with(state_root) {
        return Err(DaemonCoreError::UnsafeStateRoot {
            workspace_root: state_root.parent().unwrap_or(state_root).to_path_buf(),
            state_root: canonical,
            reason: "retention candidate resolves outside state root",
        });
    }
    let metadata = fs::symlink_metadata(&component.path).map_err(|source| {
        DaemonCoreError::io(
            "failed to revalidate retention candidate",
            &component.path,
            source,
        )
    })?;
    let identity = file_identity(&metadata);
    if component.scan.identity != Some(identity) || metadata.file_type().is_symlink() {
        return Err(DaemonCoreError::RetentionCandidateChanged {
            path: component.path.clone(),
        });
    }
    let mut issues = Vec::new();
    let current_scan = scan_path(
        &component.path,
        &mut issues,
        "candidate_revalidation_failed",
    );
    if !current_scan.safe || !scan_matches(&component.scan, &current_scan) {
        return Err(DaemonCoreError::RetentionCandidateChanged {
            path: component.path.clone(),
        });
    }

    fs::rename(&component.path, destination).map_err(|source| {
        DaemonCoreError::io(
            "failed to stage retention candidate",
            &component.path,
            source,
        )
    })?;
    staged.push(StagedComponent {
        original: component.path.clone(),
        staged: destination.to_path_buf(),
    });
    let mut staged_issues = Vec::new();
    let staged_scan = scan_path(
        destination,
        &mut staged_issues,
        "staged_candidate_revalidation_failed",
    );
    if !staged_scan.safe || !scan_matches(&component.scan, &staged_scan) {
        return Err(DaemonCoreError::RetentionCandidateChanged {
            path: destination.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(unix)]
fn rollback_staged(staged: &[StagedComponent]) -> Result<()> {
    for component in staged.iter().rev() {
        if component.staged.exists() && !component.original.exists() {
            fs::rename(&component.staged, &component.original).map_err(|source| {
                DaemonCoreError::io(
                    "failed to restore retention candidate",
                    &component.original,
                    source,
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn ensure_quarantine_root(state_root: &Path, quarantine_root: &Path) -> Result<()> {
    match fs::symlink_metadata(quarantine_root) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(DaemonCoreError::UnsafeStateRoot {
                    workspace_root: state_root.parent().unwrap_or(state_root).to_path_buf(),
                    state_root: quarantine_root.to_path_buf(),
                    reason: "retention quarantine is not a real directory",
                });
            }
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(quarantine_root).map_err(|source| {
                DaemonCoreError::io(
                    "failed to create retention quarantine root",
                    quarantine_root,
                    source,
                )
            })?;
        }
        Err(source) => {
            return Err(DaemonCoreError::io(
                "failed to inspect retention quarantine root",
                quarantine_root,
                source,
            ));
        }
    }
    let canonical = fs::canonicalize(quarantine_root).map_err(|source| {
        DaemonCoreError::io(
            "failed to resolve retention quarantine root",
            quarantine_root,
            source,
        )
    })?;
    if !canonical.starts_with(state_root) {
        return Err(DaemonCoreError::UnsafeStateRoot {
            workspace_root: state_root.parent().unwrap_or(state_root).to_path_buf(),
            state_root: canonical,
            reason: "retention quarantine resolves outside state root",
        });
    }
    Ok(())
}

fn remove_empty_dir(path: &Path) -> Result<()> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(source)
            if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(source) => Err(DaemonCoreError::io(
            "failed to remove empty retention directory",
            path,
            source,
        )),
    }
}

fn candidate_matches(planned: &Candidate, current: &Candidate) -> bool {
    planned.task_ids == current.task_ids
        && planned.record_serializations == current.record_serializations
        && planned.record_logical_bytes == current.record_logical_bytes
        && component_matches(planned.artifact.as_ref(), current.artifact.as_ref())
        && component_matches(planned.event.as_ref(), current.event.as_ref())
}

fn component_matches(
    planned: Option<&ManagedComponent>,
    current: Option<&ManagedComponent>,
) -> bool {
    match (planned, current) {
        (Some(planned), Some(current)) => {
            planned.path == current.path && scan_matches(&planned.scan, &current.scan)
        }
        (None, None) => true,
        _ => false,
    }
}

fn scan_matches(planned: &ScanSummary, current: &ScanSummary) -> bool {
    planned.identity == current.identity
        && planned.logical_bytes == current.logical_bytes
        && planned.allocated_bytes == current.allocated_bytes
        && planned.files == current.files
        && planned.directories == current.directories
        && planned.symlinks == current.symlinks
        && planned.metadata_fingerprint == current.metadata_fingerprint
}

fn push_issue(issues: &mut Vec<TaskStoreIssue>, kind: &str, path: &Path, message: String) {
    issues.push(TaskStoreIssue {
        kind: kind.to_string(),
        path: path.display().to_string(),
        message,
    });
}

fn modified_unix(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

fn modified_unix_nanos(metadata: &fs::Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
}

fn latest_timestamp(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;

    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(not(unix))]
fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    FileIdentity {
        length: metadata.len(),
        modified_unix_nanos: modified_unix_nanos(metadata).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File, FileTimes};
    use std::sync::{mpsc, Arc, Barrier};
    use std::thread;
    use std::time::{Duration, UNIX_EPOCH};

    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    use packet28_daemon_protocol::paths::{
        active_task_path, ready_path, task_artifact_dir, task_event_log_path, task_registry_path,
    };
    use packet28_daemon_protocol::task::{TaskLifecycle, TaskRecord, TaskRegistry};
    use tempfile::tempdir;

    use crate::task_store_lease::acquire_daemon_task_store_lease;

    use super::*;

    fn write_artifact(root: &Path, task_id: &str, bytes: &[u8], modified_at_unix: u64) -> PathBuf {
        let directory = task_artifact_dir(root, task_id);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("payload.bin");
        fs::write(&path, bytes).unwrap();
        set_modified(&path, modified_at_unix);
        set_modified(&directory, modified_at_unix);
        path
    }

    fn set_modified(path: &Path, timestamp: u64) {
        File::open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(timestamp)))
            .unwrap();
    }

    fn write_registry(root: &Path, records: impl IntoIterator<Item = TaskRecord>) {
        let mut registry = TaskRegistry::default();
        for record in records {
            registry.tasks.insert(record.task_id.clone(), record);
        }
        crate::storage::save_task_registry(root, &registry).unwrap();
    }

    fn inactive_record(task_id: &str, timestamp: u64) -> TaskRecord {
        TaskRecord {
            task_id: task_id.to_string(),
            lifecycle: TaskLifecycle::Idle,
            last_completed_at_unix: Some(timestamp),
            ..TaskRecord::default()
        }
    }

    #[cfg(unix)]
    fn allocated_bytes(path: &Path) -> u64 {
        let metadata = fs::symlink_metadata(path).unwrap();
        let own_bytes = metadata.blocks().saturating_mul(512);
        if !metadata.is_dir() {
            return own_bytes;
        }
        fs::read_dir(path)
            .unwrap()
            .map(|entry| allocated_bytes(&entry.unwrap().path()))
            .fold(own_bytes, u64::saturating_add)
    }

    #[cfg(unix)]
    #[test]
    fn inspection_reports_native_allocated_bytes_for_each_managed_scope() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "task", b"payload", 10);
        let state_root = root.path().join(STATE_DIR_NAME);
        let artifact_root = task_artifacts_dir(root.path());

        let report = inspect_task_store(root.path(), 100).unwrap();

        assert!(report.metrics_before.allocated_bytes_supported);
        assert_eq!(
            report.metrics_before.state_allocated_bytes,
            allocated_bytes(&state_root)
        );
        assert_eq!(
            report.metrics_before.task_artifact_allocated_bytes,
            allocated_bytes(&artifact_root)
        );
        assert_eq!(
            report.metrics_before.managed_task_allocated_bytes,
            report.metrics_before.task_artifact_allocated_bytes
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn inspection_marks_allocated_byte_fallback_as_logical() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "task", b"payload", 10);

        let report = inspect_task_store(root.path(), 100).unwrap();

        assert!(!report.metrics_before.allocated_bytes_supported);
        assert_eq!(
            report.metrics_before.state_allocated_bytes,
            report.metrics_before.state_logical_bytes
        );
        assert_eq!(
            report.metrics_before.managed_task_allocated_bytes,
            report.metrics_before.managed_task_logical_bytes
        );
    }

    #[test]
    fn dry_run_is_non_mutating_by_default() {
        let root = tempdir().unwrap();
        let artifact = write_artifact(root.path(), "old-task", b"payload", 10);

        let report =
            retain_task_store(root.path(), 100, RetentionOptions::dry_run(Some(20), None)).unwrap();

        assert_eq!(
            (
                report.mode,
                report.retention.planned_tasks,
                report.retention.removed_tasks,
                artifact.exists(),
                report.metrics_after == report.metrics_before,
            ),
            (RetentionMode::DryRun, 1, 0, true, true)
        );
    }

    #[test]
    fn age_limit_retains_exact_boundary_and_selects_strictly_older_task() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "at-boundary", b"a", 90);
        write_artifact(root.path(), "older", b"b", 89);

        let report =
            retain_task_store(root.path(), 100, RetentionOptions::dry_run(Some(10), None)).unwrap();

        assert_eq!(
            report
                .actions
                .iter()
                .map(|action| action.storage_key.as_str())
                .collect::<Vec<_>>(),
            vec!["older"]
        );
    }

    #[test]
    fn age_limit_normalizes_historical_millisecond_record_timestamps() {
        let root = tempdir().unwrap();
        write_registry(
            root.path(),
            [
                inactive_record("at-boundary", 1_799_999_990_000),
                inactive_record("older", 1_799_999_989_000),
            ],
        );

        let report = retain_task_store(
            root.path(),
            1_800_000_000,
            RetentionOptions::dry_run(Some(10), None),
        )
        .unwrap();

        assert_eq!(
            report
                .actions
                .iter()
                .map(|action| action.storage_key.as_str())
                .collect::<Vec<_>>(),
            vec!["older"]
        );
        assert_eq!(
            report.metrics_before.newest_task_timestamp_unix,
            Some(1_799_999_990)
        );
    }

    #[test]
    fn mixed_timestamp_units_do_not_falsely_mark_completed_agent_active() {
        let root = tempdir().unwrap();
        write_registry(
            root.path(),
            [TaskRecord {
                task_id: "completed".to_string(),
                lifecycle: TaskLifecycle::Idle,
                latest_agent_started_at_unix: Some(1_800_000_000_000),
                latest_agent_completed_at_unix: Some(1_800_000_001),
                ..TaskRecord::default()
            }],
        );

        let report = retain_task_store(
            root.path(),
            1_800_000_100,
            RetentionOptions::dry_run(Some(1), None),
        )
        .unwrap();

        assert_eq!(
            (
                report.metrics_before.active_tasks,
                report.retention.protected_tasks,
                report.retention.planned_tasks,
            ),
            (0, 0, 1)
        );
    }

    #[test]
    fn byte_limit_retains_exact_boundary() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "task", b"four", 10);

        let report =
            retain_task_store(root.path(), 100, RetentionOptions::dry_run(None, Some(4))).unwrap();

        assert_eq!(report.retention.planned_tasks, 0);
    }

    #[test]
    fn byte_limit_selects_oldest_candidate_when_one_byte_over() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "task", b"four", 10);

        let report =
            retain_task_store(root.path(), 100, RetentionOptions::dry_run(None, Some(3))).unwrap();

        assert_eq!(
            (
                report.retention.planned_tasks,
                report.retention.planned_logical_bytes,
                report.retention.remaining_managed_logical_bytes,
            ),
            (1, 4, 0)
        );
    }

    #[test]
    fn active_registry_task_is_protected() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "active", b"payload", 10);
        write_registry(
            root.path(),
            [TaskRecord {
                task_id: "active".to_string(),
                lifecycle: TaskLifecycle::Running,
                last_started_at_unix: Some(10),
                ..TaskRecord::default()
            }],
        );

        let report = retain_task_store(
            root.path(),
            100,
            RetentionOptions::dry_run(Some(1), Some(0)),
        )
        .unwrap();

        assert_eq!(
            (
                report.retention.planned_tasks,
                report.retention.protected_tasks,
                report.metrics_before.active_tasks,
                report.retention.remaining_over_limit_bytes,
            ),
            (0, 1, 1, report.metrics_before.managed_task_logical_bytes,)
        );
    }

    #[test]
    fn active_task_pointer_protects_orphan_artifacts() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "pointer-task", b"payload", 10);
        let path = active_task_path(root.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            serde_json::to_vec(&ActiveTaskRecord {
                task_id: "pointer-task".to_string(),
                session_id: None,
                updated_at_unix: 10,
            })
            .unwrap(),
        )
        .unwrap();

        let report = retain_task_store(
            root.path(),
            100,
            RetentionOptions::dry_run(Some(1), Some(0)),
        )
        .unwrap();

        assert_eq!(report.retention.protected_tasks, 1);
    }

    #[test]
    fn dry_run_accounting_sums_selected_candidate_bytes() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "first", b"one", 10);
        write_artifact(root.path(), "second", b"12345", 20);

        let report =
            retain_task_store(root.path(), 100, RetentionOptions::dry_run(None, Some(0))).unwrap();

        assert_eq!(
            (
                report.retention.planned_tasks,
                report.retention.planned_logical_bytes,
                report.metrics_before.managed_task_logical_bytes,
            ),
            (2, 8, 8)
        );
    }

    #[test]
    fn planned_actions_are_reported_oldest_first() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "z-oldest", b"one", 10);
        write_artifact(root.path(), "a-newest", b"two", 20);

        let report =
            retain_task_store(root.path(), 100, RetentionOptions::dry_run(None, Some(0))).unwrap();

        assert_eq!(
            report
                .actions
                .iter()
                .map(|action| action.storage_key.as_str())
                .collect::<Vec<_>>(),
            vec!["z-oldest", "a-newest"]
        );
    }

    #[test]
    fn size_plans_order_unknown_timestamps_after_known_ages() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "z-known", b"one", 10);
        write_registry(
            root.path(),
            [TaskRecord {
                task_id: "a-unknown".to_string(),
                lifecycle: TaskLifecycle::Idle,
                ..TaskRecord::default()
            }],
        );

        let report =
            retain_task_store(root.path(), 100, RetentionOptions::dry_run(None, Some(0))).unwrap();

        assert_eq!(
            report
                .actions
                .iter()
                .map(|action| action.storage_key.as_str())
                .collect::<Vec<_>>(),
            vec!["z-known", "a-unknown"]
        );
    }

    #[test]
    fn corrupt_registry_protects_all_candidates() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "task", b"payload", 10);
        let path = task_registry_path(root.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "{not-json").unwrap();

        let report = retain_task_store(
            root.path(),
            100,
            RetentionOptions::dry_run(Some(1), Some(0)),
        )
        .unwrap();

        assert_eq!(
            (
                report.metrics_before.task_registry_reliable,
                report.retention.planned_tasks,
                report.retention.protected_tasks,
            ),
            (false, 0, 1)
        );
    }

    #[test]
    fn unreadable_registry_shape_is_reported_and_protected() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "task", b"payload", 10);
        fs::create_dir_all(task_registry_path(root.path())).unwrap();

        let report = retain_task_store(
            root.path(),
            100,
            RetentionOptions::dry_run(Some(1), Some(0)),
        )
        .unwrap();

        assert!(report
            .issues
            .iter()
            .any(|issue| issue.kind == "registry_unsafe"));
        assert_eq!(report.retention.protected_tasks, 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_never_followed_or_removed() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_file = outside.path().join("keep.txt");
        fs::write(&outside_file, "keep").unwrap();
        let task_dir = task_artifact_dir(root.path(), "linked");
        fs::create_dir_all(&task_dir).unwrap();
        symlink(outside.path(), task_dir.join("escape")).unwrap();

        let report = retain_task_store(
            root.path(),
            100,
            RetentionOptions::dry_run(None, Some(0)).apply(),
        )
        .unwrap();

        assert_eq!(
            (
                report.retention.removed_tasks,
                report.retention.protected_tasks,
                outside_file.exists(),
            ),
            (0, 1, true)
        );
        assert_eq!(
            report
                .issues
                .iter()
                .filter(|issue| issue.kind == "symlink_entry")
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_managed_root_protects_hidden_task_state() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_file = outside.path().join("keep.txt");
        fs::write(&outside_file, b"keep").unwrap();
        write_registry(root.path(), [inactive_record("old-task", 10)]);
        symlink(outside.path(), task_artifacts_dir(root.path())).unwrap();

        let report =
            retain_task_store(root.path(), 100, RetentionOptions::dry_run(Some(1), None)).unwrap();

        assert_eq!(
            (
                report.retention.planned_tasks,
                report.retention.protected_tasks,
                outside_file.exists(),
            ),
            (0, 1, true)
        );
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.kind == "artifact_root"));
    }

    #[cfg(unix)]
    #[test]
    fn explicit_apply_removes_artifacts_and_registry_record() {
        let root = tempdir().unwrap();
        let artifact = write_artifact(root.path(), "old-task", b"payload", 10);
        let event_log = task_event_log_path(root.path(), "old-task");
        fs::create_dir_all(event_log.parent().unwrap()).unwrap();
        fs::write(&event_log, b"event\n").unwrap();
        set_modified(&event_log, 10);
        let unrelated = root.path().join(".packet28/index/keep.bin");
        fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
        fs::write(&unrelated, b"keep").unwrap();
        write_registry(
            root.path(),
            [
                inactive_record("old-task", 10),
                inactive_record("keep-task", 100),
            ],
        );

        let report = retain_task_store(
            root.path(),
            100,
            RetentionOptions::dry_run(Some(1), None).apply(),
        )
        .unwrap();
        let registry = crate::storage::load_task_registry(root.path()).unwrap();

        assert_eq!(
            (
                report.retention.removed_tasks,
                artifact.exists(),
                event_log.exists(),
                registry.tasks.contains_key("old-task"),
                registry.tasks.contains_key("keep-task"),
                unrelated.exists(),
            ),
            (1, false, false, false, true, true)
        );
    }

    #[cfg(unix)]
    #[test]
    fn identity_change_between_plan_and_apply_is_skipped() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "raced", b"old", 10);
        let mut snapshot = StoreSnapshot::load(root.path(), 100).unwrap();
        let mut plan = build_plan(&snapshot, RetentionOptions::dry_run(Some(1), None).apply());
        let task_dir = task_artifact_dir(root.path(), "raced");
        fs::remove_dir_all(&task_dir).unwrap();
        write_artifact(root.path(), "raced", b"new", 10);

        apply_plan(&mut snapshot, &mut plan).unwrap();

        assert_eq!(plan.actions[0].outcome, RetentionOutcome::Skipped);
        assert_eq!(fs::read(task_dir.join("payload.bin")).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn same_size_content_change_between_plan_and_apply_is_skipped() {
        let root = tempdir().unwrap();
        let artifact = write_artifact(root.path(), "raced", b"old", 10);
        let mut snapshot = StoreSnapshot::load(root.path(), 100).unwrap();
        let mut plan = build_plan(&snapshot, RetentionOptions::dry_run(Some(1), None).apply());
        fs::write(&artifact, b"new").unwrap();
        set_modified(&artifact, 11);

        apply_plan(&mut snapshot, &mut plan).unwrap();

        assert_eq!(plan.actions[0].outcome, RetentionOutcome::Skipped);
        assert_eq!(fs::read(artifact).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn apply_revalidates_only_managed_candidate_paths() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_file = outside.path().join("keep.txt");
        fs::write(&outside_file, b"keep").unwrap();
        let artifact = write_artifact(root.path(), "stale", b"old", 10);
        let unrelated = root.path().join(".packet28/index");
        fs::create_dir_all(&unrelated).unwrap();
        symlink(outside.path(), unrelated.join("external")).unwrap();
        let mut snapshot = StoreSnapshot::load(root.path(), 100).unwrap();
        let mut plan = build_plan(&snapshot, RetentionOptions::dry_run(Some(1), None).apply());

        apply_plan(&mut snapshot, &mut plan).unwrap();

        assert_eq!(plan.actions[0].outcome, RetentionOutcome::Removed);
        assert!(!artifact.exists());
        assert!(outside_file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn orphan_that_becomes_active_after_staging_is_preserved() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "newly-active", b"payload", 10);
        let snapshot = StoreSnapshot::load(root.path(), 100).unwrap();
        let candidate = snapshot.candidates.get("newly-active").unwrap();
        let staged = root.path().join(".packet28/staged-for-test");
        fs::rename(task_artifact_dir(root.path(), "newly-active"), &staged).unwrap();
        let path = active_task_path(root.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            serde_json::to_vec(&ActiveTaskRecord {
                task_id: "newly-active".to_string(),
                session_id: None,
                updated_at_unix: 50,
            })
            .unwrap(),
        )
        .unwrap();

        assert!(!candidate_remains_safe_after_staging(&snapshot, candidate).unwrap());
        assert!(staged.exists());
    }

    #[test]
    fn apply_requires_an_explicit_bound() {
        let root = tempdir().unwrap();

        let error =
            retain_task_store(root.path(), 100, RetentionOptions::inspect().apply()).unwrap_err();

        assert!(matches!(
            error,
            DaemonCoreError::InvalidRetentionPolicy { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn ready_daemon_blocks_explicit_apply() {
        let root = tempdir().unwrap();
        write_artifact(root.path(), "old-task", b"payload", 10);
        let ready = ready_path(root.path());
        fs::create_dir_all(ready.parent().unwrap()).unwrap();
        fs::write(&ready, "ready").unwrap();
        let expected_ready = fs::canonicalize(&ready).unwrap();

        let error = retain_task_store(
            root.path(),
            100,
            RetentionOptions::dry_run(Some(1), None).apply(),
        )
        .unwrap_err();

        assert!(
            matches!(
            error,
            DaemonCoreError::RetentionBlockedByDaemon { ref path } if path == &expected_ready
            ),
            "unexpected error: {error:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn daemon_lease_acquired_first_blocks_apply_without_mutation() {
        let root = tempdir().unwrap();
        let artifact = write_artifact(root.path(), "old-task", b"payload", 10);
        let daemon_lease = acquire_daemon_task_store_lease(root.path()).unwrap();

        let error = retain_task_store(
            root.path(),
            100,
            RetentionOptions::dry_run(Some(1), None).apply(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            DaemonCoreError::RetentionBlockedByDaemon { .. }
        ));
        assert!(artifact.exists());
        drop(daemon_lease);
    }

    #[cfg(unix)]
    #[test]
    fn apply_lease_acquired_first_blocks_daemon_until_cleanup_finishes() {
        let root = tempdir().unwrap();
        let artifact = write_artifact(root.path(), "old-task", b"payload", 10);
        let lease_acquired = Arc::new(Barrier::new(2));
        let continue_cleanup = Arc::new(Barrier::new(2));
        let cleanup_root = root.path().to_path_buf();
        let cleanup_acquired = lease_acquired.clone();
        let cleanup_continue = continue_cleanup.clone();
        let cleanup = thread::spawn(move || {
            retain_task_store_with_lease_observer(
                &cleanup_root,
                100,
                RetentionOptions::dry_run(Some(1), None).apply(),
                || {
                    cleanup_acquired.wait();
                    cleanup_continue.wait();
                },
            )
        });
        lease_acquired.wait();

        let daemon_root = root.path().to_path_buf();
        let (daemon_acquired_tx, daemon_acquired_rx) = mpsc::channel();
        let daemon = thread::spawn(move || {
            let lease = acquire_daemon_task_store_lease(&daemon_root).unwrap();
            daemon_acquired_tx.send(()).unwrap();
            lease
        });
        assert!(matches!(
            daemon_acquired_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        continue_cleanup.wait();
        let report = cleanup.join().unwrap().unwrap();
        daemon_acquired_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        drop(daemon.join().unwrap());

        assert_eq!(report.retention.removed_tasks, 1);
        assert!(!artifact.exists());
    }

    #[cfg(unix)]
    #[test]
    fn broken_ready_marker_still_blocks_explicit_apply() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        write_artifact(root.path(), "old-task", b"payload", 10);
        let ready = ready_path(root.path());
        fs::create_dir_all(ready.parent().unwrap()).unwrap();
        symlink(root.path().join("missing-ready-target"), &ready).unwrap();
        let expected_ready = ready_path(&fs::canonicalize(root.path()).unwrap());

        let error = retain_task_store(
            root.path(),
            100,
            RetentionOptions::dry_run(Some(1), None).apply(),
        )
        .unwrap_err();

        assert!(
            matches!(
                error,
                DaemonCoreError::RetentionBlockedByDaemon { ref path } if path == &expected_ready
            ),
            "unexpected error: {error:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_state_root_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        symlink(outside.path(), root.path().join(STATE_DIR_NAME)).unwrap();

        let error = inspect_task_store(root.path(), 100).unwrap_err();

        assert!(matches!(error, DaemonCoreError::UnsafeStateRoot { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlink_state_root_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        symlink(
            root.path().join("missing"),
            root.path().join(STATE_DIR_NAME),
        )
        .unwrap();

        let error = inspect_task_store(root.path(), 100).unwrap_err();

        assert!(matches!(error, DaemonCoreError::UnsafeStateRoot { .. }));
    }
}
