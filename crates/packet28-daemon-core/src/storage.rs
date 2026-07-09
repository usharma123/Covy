use super::*;
use fs2::FileExt;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};

static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct TaskEventLogRead {
    pub events: Vec<DaemonEventFrame>,
    pub next_offset: u64,
}

pub fn ensure_daemon_dir(root: &Path) -> Result<PathBuf> {
    let dir = daemon_dir(root);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create daemon directory '{}'", dir.display()))?;
    let socket_dir = socket_path(root)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&socket_dir).with_context(|| {
        format!(
            "failed to create daemon socket directory '{}'",
            socket_dir.display()
        )
    })?;
    Ok(dir)
}

pub fn write_runtime_info(root: &Path, info: &DaemonRuntimeInfo) -> Result<()> {
    ensure_daemon_dir(root)?;
    write_atomically(&pid_path(root), format!("{}\n", info.pid).as_bytes())
        .with_context(|| format!("failed to write pid file for '{}'", root.display()))?;
    write_atomically(&runtime_path(root), &serde_json::to_vec_pretty(info)?)
        .with_context(|| format!("failed to write runtime file for '{}'", root.display()))?;
    Ok(())
}

pub fn read_runtime_info(root: &Path) -> Result<DaemonRuntimeInfo> {
    let raw = fs::read(runtime_path(root))
        .with_context(|| format!("failed to read runtime file for '{}'", root.display()))?;
    Ok(serde_json::from_slice(&raw)?)
}

pub fn remove_runtime_files(root: &Path) -> Result<()> {
    for path in [
        socket_path(root),
        workspace_socket_path(root),
        pid_path(root),
        runtime_path(root),
        ready_path(root),
    ] {
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove '{}'", path.display()))?;
        }
    }
    Ok(())
}

pub fn load_watch_registry(root: &Path) -> Result<WatchRegistry> {
    let path = watch_registry_path(root);
    with_registry_lock(root, &path, RegistryLockMode::Shared, || {
        if !path.exists() {
            return Ok(WatchRegistry::default());
        }
        let raw = fs::read(&path)
            .with_context(|| format!("failed to read watch registry '{}'", path.display()))?;
        Ok(serde_json::from_slice(&raw)?)
    })
}

pub fn save_watch_registry(root: &Path, registry: &WatchRegistry) -> Result<()> {
    let path = watch_registry_path(root);
    let bytes = serde_json::to_vec_pretty(registry)?;
    with_registry_lock(root, &path, RegistryLockMode::Exclusive, || {
        write_atomically(&path, &bytes)
            .with_context(|| format!("failed to write watch registry '{}'", path.display()))?;
        Ok(())
    })
}

pub fn load_task_registry(root: &Path) -> Result<TaskRegistry> {
    let path = task_registry_path(root);
    with_registry_lock(root, &path, RegistryLockMode::Shared, || {
        if !path.exists() {
            return Ok(TaskRegistry::default());
        }
        let raw = fs::read(&path)
            .with_context(|| format!("failed to read task registry '{}'", path.display()))?;
        Ok(serde_json::from_slice(&raw)?)
    })
}

pub fn save_task_registry(root: &Path, registry: &TaskRegistry) -> Result<()> {
    let path = task_registry_path(root);
    let bytes = serde_json::to_vec_pretty(registry)?;
    with_registry_lock(root, &path, RegistryLockMode::Exclusive, || {
        write_atomically(&path, &bytes)
            .with_context(|| format!("failed to write task registry '{}'", path.display()))?;
        Ok(())
    })
}

pub fn append_task_event(root: &Path, frame: &DaemonEventFrame) -> Result<()> {
    let dir = task_events_dir(root);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create task events dir '{}'", dir.display()))?;
    let path = task_event_log_path(root, &frame.task_id);
    let mut bytes = serde_json::to_vec(frame)?;
    bytes.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open task event log '{}'", path.display()))?;
    FileExt::lock_exclusive(&file)
        .with_context(|| format!("failed to lock task event log '{}'", path.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("failed to append task event log '{}'", path.display()))?;
    FileExt::unlock(&file)
        .with_context(|| format!("failed to unlock task event log '{}'", path.display()))?;
    Ok(())
}

pub fn load_task_events(root: &Path, task_id: &str) -> Result<Vec<DaemonEventFrame>> {
    Ok(load_task_events_from_offset(root, task_id, 0)?.events)
}

pub fn task_event_log_len(root: &Path, task_id: &str) -> Result<u64> {
    let path = task_event_log_path(root, task_id);
    if !path.exists() {
        return Ok(0);
    }
    Ok(fs::metadata(&path)
        .with_context(|| format!("failed to stat task event log '{}'", path.display()))?
        .len())
}

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
        .with_context(|| format!("failed to read task event log '{}'", path.display()))?;
    let len = file
        .metadata()
        .with_context(|| format!("failed to stat task event log '{}'", path.display()))?
        .len();
    let start = offset.min(len);
    file.seek(SeekFrom::Start(start))
        .with_context(|| format!("failed to seek task event log '{}'", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut next_offset = start;
    let mut line = String::new();
    let mut events = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .with_context(|| format!("failed to read task event log '{}'", path.display()))?;
        if read == 0 {
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
    Ok(TaskEventLogRead {
        events,
        next_offset,
    })
}

pub fn resolve_workspace_root(start: &Path) -> PathBuf {
    let mut dir = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        if dir.join(".git").exists() {
            return dir;
        }
        if !dir.pop() {
            return start.to_path_buf();
        }
    }
}

pub fn write_socket_message<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_SOCKET_MESSAGE_BYTES {
        anyhow::bail!(
            "socket frame too large: {} bytes exceeds limit of {}",
            bytes.len(),
            MAX_SOCKET_MESSAGE_BYTES
        );
    }
    let len = bytes.len() as u64;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

pub fn read_socket_message<R: Read, T: for<'de> Deserialize<'de>>(reader: &mut R) -> Result<T> {
    let mut len_bytes = [0_u8; 8];
    reader.read_exact(&mut len_bytes)?;
    let len = usize::try_from(u64::from_be_bytes(len_bytes))
        .context("socket frame length does not fit in usize")?;
    if len == 0 {
        anyhow::bail!("socket frame length must be greater than zero");
    }
    if len > MAX_SOCKET_MESSAGE_BYTES {
        anyhow::bail!(
            "socket frame too large: {len} bytes exceeds limit of {MAX_SOCKET_MESSAGE_BYTES}"
        );
    }
    let mut body = vec![0_u8; len];
    reader.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp_path = atomic_temp_path(path);
    let mut file = fs::File::create(&temp_path)
        .with_context(|| format!("failed to create temp file '{}'", temp_path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write temp file '{}'", temp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync temp file '{}'", temp_path.display()))?;
    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "failed to atomically replace '{}' with '{}'",
            path.display(),
            temp_path.display()
        )
    })?;
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
        .with_context(|| format!("failed to open registry lock '{}'", lock_path.display()))?;
    match mode {
        RegistryLockMode::Shared => FileExt::lock_shared(&file)
            .with_context(|| format!("failed to lock registry '{}'", registry_path.display()))?,
        RegistryLockMode::Exclusive => FileExt::lock_exclusive(&file)
            .with_context(|| format!("failed to lock registry '{}'", registry_path.display()))?,
    }

    let result = operation();
    let unlock_result = FileExt::unlock(&file)
        .with_context(|| format!("failed to unlock registry '{}'", registry_path.display()));
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
}
