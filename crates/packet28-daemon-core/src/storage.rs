//! Durable daemon runtime metadata, registries, and append-only task events.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fs2::FileExt;
use packet28_daemon_protocol::message::{DaemonEventFrame, DaemonRuntimeInfo};
use packet28_daemon_protocol::paths::{
    daemon_dir, pid_path, ready_path, runtime_path, socket_path, task_event_log_path,
    task_events_dir, task_registry_path, watch_registry_path, workspace_socket_path,
};
use packet28_daemon_protocol::task::{TaskRegistry, WatchRegistry};

use crate::{DaemonCoreError, Result};

static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Events read from one byte offset in an append-only task event log.
#[derive(Debug, Clone)]
pub struct TaskEventLogRead {
    /// Complete JSON-line event frames decoded from the requested offset.
    pub events: Vec<DaemonEventFrame>,
    /// Byte offset immediately after the final complete line that was read.
    pub next_offset: u64,
}

/// Creates the daemon state and socket directories for `root`.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if either directory cannot be created.
pub fn ensure_daemon_dir(root: &Path) -> Result<PathBuf> {
    let dir = daemon_dir(root);
    fs::create_dir_all(&dir)
        .map_err(|source| DaemonCoreError::io("failed to create daemon directory", &dir, source))?;
    let socket_dir = socket_path(root)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&socket_dir).map_err(|source| {
        DaemonCoreError::io(
            "failed to create daemon socket directory",
            &socket_dir,
            source,
        )
    })?;
    Ok(dir)
}

/// Persists process and runtime discovery metadata for a daemon.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if daemon directories or metadata files
/// cannot be created, written, synchronized, or replaced. Returns
/// [`DaemonCoreError::Json`] if `info` cannot be encoded.
pub fn write_runtime_info(root: &Path, info: &DaemonRuntimeInfo) -> Result<()> {
    ensure_daemon_dir(root)?;
    write_atomically(&pid_path(root), format!("{}\n", info.pid).as_bytes())?;
    let path = runtime_path(root);
    let bytes = serde_json::to_vec_pretty(info).map_err(|source| {
        DaemonCoreError::json("failed to encode runtime metadata for", &path, source)
    })?;
    write_atomically(&path, &bytes)?;
    Ok(())
}

/// Loads persisted runtime discovery metadata for a daemon.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the runtime file cannot be read, or
/// [`DaemonCoreError::Json`] if it is not valid runtime metadata.
pub fn read_runtime_info(root: &Path) -> Result<DaemonRuntimeInfo> {
    let path = runtime_path(root);
    let raw = fs::read(&path)
        .map_err(|source| DaemonCoreError::io("failed to read runtime metadata", &path, source))?;
    serde_json::from_slice(&raw).map_err(|source| {
        DaemonCoreError::json("failed to decode runtime metadata from", &path, source)
    })
}

/// Removes daemon socket and runtime discovery files that currently exist.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] when an existing runtime file cannot be
/// removed. Files that are already absent are ignored.
pub fn remove_runtime_files(root: &Path) -> Result<()> {
    for path in [
        socket_path(root),
        workspace_socket_path(root),
        pid_path(root),
        runtime_path(root),
        ready_path(root),
    ] {
        if path.exists() {
            fs::remove_file(&path).map_err(|source| {
                DaemonCoreError::io("failed to remove daemon runtime file", &path, source)
            })?;
        }
    }
    Ok(())
}

/// Loads the workspace watch registry under a shared interprocess lock.
///
/// Returns an empty registry when no file has been persisted.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the registry or lock file cannot be
/// opened, read, locked, or unlocked. Returns [`DaemonCoreError::Json`] if the
/// persisted registry is malformed.
pub fn load_watch_registry(root: &Path) -> Result<WatchRegistry> {
    let path = watch_registry_path(root);
    with_registry_lock(root, &path, RegistryLockMode::Shared, || {
        if !path.exists() {
            return Ok(WatchRegistry::default());
        }
        let raw = fs::read(&path).map_err(|source| {
            DaemonCoreError::io("failed to read watch registry", &path, source)
        })?;
        serde_json::from_slice(&raw).map_err(|source| {
            DaemonCoreError::json("failed to decode watch registry from", &path, source)
        })
    })
}

/// Persists the workspace watch registry under an exclusive interprocess lock.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Json`] if the registry cannot be encoded.
/// Returns [`DaemonCoreError::Io`] if the daemon directory, lock, or registry
/// file cannot be created, written, synchronized, replaced, or unlocked.
pub fn save_watch_registry(root: &Path, registry: &WatchRegistry) -> Result<()> {
    let path = watch_registry_path(root);
    let bytes = serde_json::to_vec_pretty(registry).map_err(|source| {
        DaemonCoreError::json("failed to encode watch registry for", &path, source)
    })?;
    with_registry_lock(root, &path, RegistryLockMode::Exclusive, || {
        write_atomically(&path, &bytes)
    })
}

/// Loads the task registry under a shared interprocess lock.
///
/// Returns an empty registry when no file has been persisted.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the registry or lock file cannot be
/// opened, read, locked, or unlocked. Returns [`DaemonCoreError::Json`] if the
/// persisted registry is malformed.
pub fn load_task_registry(root: &Path) -> Result<TaskRegistry> {
    let path = task_registry_path(root);
    with_registry_lock(root, &path, RegistryLockMode::Shared, || {
        if !path.exists() {
            return Ok(TaskRegistry::default());
        }
        let raw = fs::read(&path)
            .map_err(|source| DaemonCoreError::io("failed to read task registry", &path, source))?;
        serde_json::from_slice(&raw).map_err(|source| {
            DaemonCoreError::json("failed to decode task registry from", &path, source)
        })
    })
}

/// Persists the task registry under an exclusive interprocess lock.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Json`] if the registry cannot be encoded.
/// Returns [`DaemonCoreError::Io`] if the daemon directory, lock, or registry
/// file cannot be created, written, synchronized, replaced, or unlocked.
pub fn save_task_registry(root: &Path, registry: &TaskRegistry) -> Result<()> {
    let path = task_registry_path(root);
    let bytes = serde_json::to_vec_pretty(registry).map_err(|source| {
        DaemonCoreError::json("failed to encode task registry for", &path, source)
    })?;
    with_registry_lock(root, &path, RegistryLockMode::Exclusive, || {
        write_atomically(&path, &bytes)
    })
}

pub(crate) fn remove_task_registry_records_if_unchanged(
    root: &Path,
    expected_records: &BTreeMap<String, Vec<u8>>,
) -> Result<bool> {
    if expected_records.is_empty() {
        return Ok(true);
    }

    let path = task_registry_path(root);
    with_registry_lock(root, &path, RegistryLockMode::Exclusive, || {
        let raw = fs::read(&path)
            .map_err(|source| DaemonCoreError::io("failed to read task registry", &path, source))?;
        let mut registry: TaskRegistry = serde_json::from_slice(&raw).map_err(|source| {
            DaemonCoreError::json("failed to decode task registry from", &path, source)
        })?;
        for (task_id, expected) in expected_records {
            let Some(current) = registry.tasks.get(task_id) else {
                return Ok(false);
            };
            let current = serde_json::to_vec(current).map_err(|source| {
                DaemonCoreError::json("failed to verify task registry record in", &path, source)
            })?;
            if &current != expected {
                return Ok(false);
            }
        }
        for task_id in expected_records.keys() {
            registry.tasks.remove(task_id);
        }
        let bytes = serde_json::to_vec_pretty(&registry).map_err(|source| {
            DaemonCoreError::json("failed to encode task registry for", &path, source)
        })?;
        write_atomically(&path, &bytes)?;
        Ok(true)
    })
}

/// Appends one complete JSON-line event to a task's durable event log.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Json`] if `frame` cannot be encoded. Returns
/// [`DaemonCoreError::Io`] if the event directory or log cannot be opened,
/// locked, appended, or unlocked.
pub fn append_task_event(root: &Path, frame: &DaemonEventFrame) -> Result<()> {
    let dir = task_events_dir(root);
    fs::create_dir_all(&dir).map_err(|source| {
        DaemonCoreError::io("failed to create task events directory", &dir, source)
    })?;
    let path = task_event_log_path(root, &frame.task_id);
    let mut bytes = serde_json::to_vec(frame).map_err(|source| {
        DaemonCoreError::json("failed to encode task event for", &path, source)
    })?;
    bytes.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|source| DaemonCoreError::io("failed to open task event log", &path, source))?;
    FileExt::lock_exclusive(&file)
        .map_err(|source| DaemonCoreError::io("failed to lock task event log", &path, source))?;
    file.write_all(&bytes)
        .map_err(|source| DaemonCoreError::io("failed to append task event log", &path, source))?;
    FileExt::unlock(&file)
        .map_err(|source| DaemonCoreError::io("failed to unlock task event log", &path, source))?;
    Ok(())
}

/// Loads all complete, valid event frames for one task.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the event log cannot be opened, locked,
/// inspected, read, sought, or unlocked.
pub fn load_task_events(root: &Path, task_id: &str) -> Result<Vec<DaemonEventFrame>> {
    Ok(load_task_events_from_offset(root, task_id, 0)?.events)
}

/// Returns the current byte length of a task's event log.
///
/// A missing log has length zero.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if metadata for an existing log cannot be
/// read.
pub fn task_event_log_len(root: &Path, task_id: &str) -> Result<u64> {
    let path = task_event_log_path(root, task_id);
    if !path.exists() {
        return Ok(0);
    }
    Ok(fs::metadata(&path)
        .map_err(|source| DaemonCoreError::io("failed to inspect task event log", &path, source))?
        .len())
}

/// Loads complete, valid event frames beginning at a byte offset.
///
/// The offset is clamped to the current log length. A trailing partial line is
/// left unread so a caller can retry it after the append completes. Malformed
/// complete lines are skipped for compatibility with existing event logs.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the event log cannot be opened, locked,
/// inspected, sought, read, or unlocked.
pub fn load_task_events_from_offset(
    root: &Path,
    task_id: &str,
    offset: u64,
) -> Result<TaskEventLogRead> {
    let path = task_event_log_path(root, task_id);
    if !path.exists() {
        return Ok(TaskEventLogRead {
            events: Vec::new(),
            next_offset: 0,
        });
    }
    let mut file = fs::File::open(&path)
        .map_err(|source| DaemonCoreError::io("failed to open task event log", &path, source))?;
    FileExt::lock_shared(&file)
        .map_err(|source| DaemonCoreError::io("failed to lock task event log", &path, source))?;
    let len = file
        .metadata()
        .map_err(|source| DaemonCoreError::io("failed to inspect task event log", &path, source))?
        .len();
    let start = offset.min(len);
    file.seek(SeekFrom::Start(start))
        .map_err(|source| DaemonCoreError::io("failed to seek task event log", &path, source))?;
    let mut reader = BufReader::new(file);
    let mut next_offset = start;
    let mut line = String::new();
    let mut events = Vec::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).map_err(|source| {
            DaemonCoreError::io("failed to read task event log", &path, source)
        })?;
        if read == 0 {
            break;
        }
        if !line.ends_with('\n') {
            break;
        }
        next_offset = next_offset.saturating_add(read as u64);
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.trim().is_empty() {
            continue;
        }
        if let Ok(frame) = serde_json::from_str(trimmed) {
            events.push(frame);
        }
    }
    FileExt::unlock(reader.get_ref())
        .map_err(|source| DaemonCoreError::io("failed to unlock task event log", &path, source))?;
    Ok(TaskEventLogRead {
        events,
        next_offset,
    })
}

/// Returns the current Unix timestamp in whole seconds.
///
/// If the system clock is before the Unix epoch, this returns zero.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp_path = atomic_temp_path(path);
    let mut file = fs::File::create(&temp_path).map_err(|source| {
        DaemonCoreError::io("failed to create atomic temporary file", &temp_path, source)
    })?;
    file.write_all(bytes).map_err(|source| {
        DaemonCoreError::io("failed to write atomic temporary file", &temp_path, source)
    })?;
    file.sync_all().map_err(|source| {
        DaemonCoreError::io(
            "failed to synchronize atomic temporary file",
            &temp_path,
            source,
        )
    })?;
    fs::rename(&temp_path, path)
        .map_err(|source| DaemonCoreError::io("failed to atomically replace", path, source))?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum RegistryLockMode {
    Shared,
    Exclusive,
}

fn with_registry_lock<T>(
    root: &Path,
    registry_path: &Path,
    mode: RegistryLockMode,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    ensure_daemon_dir(root)?;
    let lock_path = registry_lock_path(registry_path);
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|source| {
            DaemonCoreError::io("failed to open registry lock", &lock_path, source)
        })?;
    match mode {
        RegistryLockMode::Shared => FileExt::lock_shared(&file).map_err(|source| {
            DaemonCoreError::io("failed to acquire shared lock for", registry_path, source)
        })?,
        RegistryLockMode::Exclusive => FileExt::lock_exclusive(&file).map_err(|source| {
            DaemonCoreError::io(
                "failed to acquire exclusive lock for",
                registry_path,
                source,
            )
        })?,
    }

    let result = operation();
    let unlock_result = FileExt::unlock(&file)
        .map_err(|source| DaemonCoreError::io("failed to unlock registry", registry_path, source));
    match (result, unlock_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(err), _) => Err(err),
        (Ok(_), Err(err)) => Err(err),
    }
}

fn registry_lock_path(registry_path: &Path) -> PathBuf {
    let file_name = registry_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("registry");
    registry_path.with_file_name(format!(".{file_name}.lock"))
}

fn atomic_temp_path(path: &Path) -> PathBuf {
    let counter = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("packet28");
    path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        counter
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use packet28_daemon_protocol::message::DaemonEvent;
    use packet28_daemon_protocol::task::{TaskLifecycle, TaskRecord};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn appends_and_loads_task_events() {
        let dir = tempdir().unwrap();
        let frame = DaemonEventFrame {
            seq: 1,
            task_id: "task/demo".to_string(),
            event: DaemonEvent {
                kind: "task_started".to_string(),
                occurred_at_unix: 1,
                data: serde_json::json!({"task_id":"task/demo"}),
            },
        };
        append_task_event(dir.path(), &frame).unwrap();
        append_task_event(
            dir.path(),
            &DaemonEventFrame {
                seq: 2,
                task_id: "task/demo".to_string(),
                event: DaemonEvent {
                    kind: "task_completed".to_string(),
                    occurred_at_unix: 2,
                    data: serde_json::json!({"task_id":"task/demo"}),
                },
            },
        )
        .unwrap();

        let loaded = load_task_events(dir.path(), "task/demo").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].seq, 1);
        assert_eq!(loaded[1].event.kind, "task_completed");
    }

    #[test]
    fn task_event_reads_skip_corrupt_lines_and_report_offsets() {
        let dir = tempdir().unwrap();
        let path = task_event_log_path(dir.path(), "task/demo");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            concat!(
                "{\"seq\":1,\"task_id\":\"task/demo\",\"event\":{\"kind\":\"task_started\",\"occurred_at_unix\":1,\"data\":{}}}\n",
                "{not-json}\n",
                "{\"seq\":2,\"task_id\":\"task/demo\",\"event\":{\"kind\":\"task_completed\",\"occurred_at_unix\":2,\"data\":{}}}\n"
            ),
        )
        .unwrap();

        let full = load_task_events_from_offset(dir.path(), "task/demo", 0).unwrap();
        assert_eq!(full.events.len(), 2);
        assert_eq!(full.events[0].seq, 1);
        assert_eq!(full.events[1].seq, 2);
        assert_eq!(full.next_offset, fs::metadata(&path).unwrap().len());

        let after_full =
            load_task_events_from_offset(dir.path(), "task/demo", full.next_offset).unwrap();
        assert!(after_full.events.is_empty());
        assert_eq!(after_full.next_offset, full.next_offset);
    }

    #[test]
    fn task_event_reads_do_not_advance_past_partial_trailing_line() {
        let dir = tempdir().unwrap();
        let path = task_event_log_path(dir.path(), "task/demo");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let complete = "{\"seq\":1,\"task_id\":\"task/demo\",\"event\":{\"kind\":\"task_started\",\"occurred_at_unix\":1,\"data\":{}}}\n";
        fs::write(
            &path,
            format!(
                "{complete}{{\"seq\":2,\"task_id\":\"task/demo\",\"event\":{{\"kind\":\"task_completed\""
            ),
        )
        .unwrap();

        let read = load_task_events_from_offset(dir.path(), "task/demo", 0).unwrap();
        assert_eq!(read.events.len(), 1);
        assert_eq!(read.events[0].seq, 1);
        assert_eq!(read.next_offset, complete.len() as u64);
    }

    #[test]
    fn atomic_temp_paths_are_unique_for_same_target() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let first = atomic_temp_path(&path);
        let second = atomic_temp_path(&path);
        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(dir.path()));
        assert_eq!(second.parent(), Some(dir.path()));
    }

    #[test]
    fn registry_lock_path_is_hidden_sibling() {
        let dir = tempdir().unwrap();
        let registry = task_registry_path(dir.path());

        assert_eq!(
            registry_lock_path(&registry),
            daemon_dir(dir.path()).join(".task-registry-v1.json.lock")
        );
    }

    #[test]
    fn save_task_registry_waits_for_registry_lock() {
        let dir = tempdir().unwrap();
        ensure_daemon_dir(dir.path()).unwrap();
        let lock_path = registry_lock_path(&task_registry_path(dir.path()));
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .unwrap();
        FileExt::lock_exclusive(&lock_file).unwrap();

        let root = dir.path().to_path_buf();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            tx.send(save_task_registry(&root, &TaskRegistry::default()))
                .unwrap();
        });

        assert!(rx.recv_timeout(Duration::from_millis(75)).is_err());
        FileExt::unlock(&lock_file).unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn load_task_registry_waits_for_registry_lock() {
        let dir = tempdir().unwrap();
        save_task_registry(dir.path(), &TaskRegistry::default()).unwrap();
        let lock_path = registry_lock_path(&task_registry_path(dir.path()));
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .unwrap();
        FileExt::lock_exclusive(&lock_file).unwrap();

        let root = dir.path().to_path_buf();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || tx.send(load_task_registry(&root)).unwrap());

        assert!(rx.recv_timeout(Duration::from_millis(75)).is_err());
        FileExt::unlock(&lock_file).unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn conditional_registry_removal_preserves_a_changed_record() {
        let dir = tempdir().unwrap();
        let original = TaskRecord {
            task_id: "task".to_string(),
            lifecycle: TaskLifecycle::Idle,
            last_completed_at_unix: Some(10),
            ..TaskRecord::default()
        };
        let expected =
            BTreeMap::from([("task".to_string(), serde_json::to_vec(&original).unwrap())]);
        let mut changed = original;
        changed.lifecycle = TaskLifecycle::Running;
        let registry = TaskRegistry {
            tasks: BTreeMap::from([("task".to_string(), changed)]),
        };
        save_task_registry(dir.path(), &registry).unwrap();

        assert!(!remove_task_registry_records_if_unchanged(dir.path(), &expected).unwrap());
        assert!(load_task_registry(dir.path())
            .unwrap()
            .tasks
            .contains_key("task"));
    }
}
