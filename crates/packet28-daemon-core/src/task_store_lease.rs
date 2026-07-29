//! Cross-process ownership for daemon task storage.
//!
//! This module provides two independent lock primitives:
//!
//! - task-store writers and daemons share the lifecycle lock while destructive
//!   maintenance owns it exclusively;
//! - one daemon owns the instance lock exclusively for its entire lifetime.
//!
//! The instance lock prevents two daemons from loading and publishing
//! independent mutable state. It does not replace the lifecycle lock because
//! non-daemon writers also need to exclude retention.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs2::FileExt;
use packet28_daemon_protocol::paths::daemon_dir;

use crate::{DaemonCoreError, Result};

const TASK_STORE_LIFECYCLE_LOCK_FILE_NAME: &str = ".task-store-lifecycle.lock";
const DAEMON_INSTANCE_LOCK_FILE_NAME: &str = ".daemon-instance.lock";

/// RAII ownership of the workspace task-store lifecycle lock.
///
/// Clones share one underlying advisory lock. The final clone releases it.
/// The persistent lock file itself is deliberately never unlinked, which
/// prevents concurrent processes from locking different inodes for the same
/// workspace.
#[derive(Clone, Debug)]
pub struct TaskStoreLease {
    inner: Arc<LeaseInner>,
}

#[derive(Debug)]
struct LeaseInner {
    file: File,
    path: PathBuf,
}

impl TaskStoreLease {
    /// Returns the persistent lock-file path.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }
}

impl Drop for LeaseInner {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Returns the persistent lifecycle lock path for a workspace task store.
pub fn task_store_lifecycle_lock_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(TASK_STORE_LIFECYCLE_LOCK_FILE_NAME)
}

/// Returns the persistent single-daemon instance lock path for a workspace.
pub fn daemon_instance_lock_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(DAEMON_INSTANCE_LOCK_FILE_NAME)
}

/// Acquires a shared task-store writer lease.
///
/// Every production mutation of task registry, active-task state, task
/// artifacts, or task events must retain this guard through the complete
/// filesystem transaction. This call blocks while destructive retention owns
/// the exclusive lifecycle lease.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the daemon directory or lifecycle lock
/// cannot be safely created, opened, validated, or locked.
pub fn acquire_task_store_writer_lease(root: &Path) -> Result<TaskStoreLease> {
    let path = task_store_lifecycle_lock_path(root);
    let file = open_persistent_daemon_lock(root, &path)?;
    FileExt::lock_shared(&file).map_err(|source| {
        DaemonCoreError::io("failed to acquire task-store writer lease", &path, source)
    })?;
    validate_open_lock_file(root, &path, &file)?;
    Ok(TaskStoreLease::new(file, path))
}

/// Acquires the shared lease intended for daemon startup through shutdown.
///
/// This call blocks while destructive task-store maintenance owns the
/// exclusive lease. Acquire it before loading mutable daemon state, and retain
/// the returned value until all daemon workers have stopped and runtime-file
/// cleanup has completed.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the daemon directory or lifecycle lock
/// cannot be safely created, opened, validated, or locked.
pub fn acquire_daemon_task_store_lease(root: &Path) -> Result<TaskStoreLease> {
    acquire_task_store_writer_lease(root)
}

/// Acquires exclusive ownership of the daemon instance for a workspace.
///
/// This is intentionally non-blocking: a second daemon must fail before
/// loading mutable state instead of waiting and later publishing stale state.
/// Retain the returned guard until worker shutdown and runtime-file cleanup
/// have completed.
///
/// # Errors
///
/// Returns [`DaemonCoreError::DaemonInstanceAlreadyRunning`] when another
/// daemon owns the workspace. Returns [`DaemonCoreError::Io`] if the lock path
/// cannot be safely created, opened, validated, or locked.
pub fn acquire_daemon_instance_lease(root: &Path) -> Result<TaskStoreLease> {
    let path = daemon_instance_lock_path(root);
    let file = open_persistent_daemon_lock(root, &path)?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            validate_open_lock_file(root, &path, &file)?;
            Ok(TaskStoreLease::new(file, path))
        }
        Err(source) if source.kind() == io::ErrorKind::WouldBlock => {
            Err(DaemonCoreError::DaemonInstanceAlreadyRunning { path })
        }
        Err(source) => Err(DaemonCoreError::io(
            "failed to acquire daemon instance lease",
            &path,
            source,
        )),
    }
}

/// Attempts to acquire the exclusive lease for destructive task maintenance.
///
/// Returns `Ok(None)` without waiting when a daemon already owns the shared
/// lease. The returned lease must remain alive for the complete maintenance
/// window.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Io`] if the daemon directory or lifecycle lock
/// cannot be safely created, opened, validated, or locked.
pub fn try_acquire_task_store_retention_lease(root: &Path) -> Result<Option<TaskStoreLease>> {
    let path = task_store_lifecycle_lock_path(root);
    let file = open_persistent_daemon_lock(root, &path)?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            validate_open_lock_file(root, &path, &file)?;
            Ok(Some(TaskStoreLease::new(file, path)))
        }
        Err(source) if source.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(source) => Err(DaemonCoreError::io(
            "failed to acquire task-retention lease",
            &path,
            source,
        )),
    }
}

impl TaskStoreLease {
    fn new(file: File, path: PathBuf) -> Self {
        Self {
            inner: Arc::new(LeaseInner { file, path }),
        }
    }
}

fn open_persistent_daemon_lock(root: &Path, path: &Path) -> Result<File> {
    let daemon_root = daemon_dir(root);
    fs::create_dir_all(&daemon_root).map_err(|source| {
        DaemonCoreError::io(
            "failed to create daemon directory for task-store lease",
            &daemon_root,
            source,
        )
    })?;
    let daemon_metadata = fs::symlink_metadata(&daemon_root).map_err(|source| {
        DaemonCoreError::io(
            "failed to inspect daemon directory for task-store lease",
            &daemon_root,
            source,
        )
    })?;
    if daemon_metadata.file_type().is_symlink() || !daemon_metadata.is_dir() {
        return Err(unsafe_lock_path(
            &daemon_root,
            "daemon task-store directory is not a real directory",
        ));
    }
    let canonical_root = fs::canonicalize(root).map_err(|source| {
        DaemonCoreError::io(
            "failed to resolve workspace for task-store lease",
            root,
            source,
        )
    })?;
    let canonical_daemon_root = fs::canonicalize(&daemon_root).map_err(|source| {
        DaemonCoreError::io(
            "failed to resolve daemon directory for task-store lease",
            &daemon_root,
            source,
        )
    })?;
    if !canonical_daemon_root.starts_with(&canonical_root) {
        return Err(unsafe_lock_path(
            &daemon_root,
            "daemon task-store directory resolves outside the workspace",
        ));
    }

    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(unsafe_lock_path(
                path,
                "persistent lock is not a regular file",
            ));
        }
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|source| {
            DaemonCoreError::io("failed to open persistent daemon lock", path, source)
        })?;
    validate_open_lock_file(root, path, &file)?;
    Ok(file)
}

fn validate_open_lock_file(root: &Path, path: &Path, file: &File) -> Result<()> {
    let path_metadata = fs::symlink_metadata(path).map_err(|source| {
        DaemonCoreError::io("failed to inspect task-store lifecycle lock", path, source)
    })?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(unsafe_lock_path(
            path,
            "persistent lock is not a regular file",
        ));
    }
    let canonical_root = fs::canonicalize(root).map_err(|source| {
        DaemonCoreError::io(
            "failed to resolve workspace for task-store lease",
            root,
            source,
        )
    })?;
    let canonical_path = fs::canonicalize(path).map_err(|source| {
        DaemonCoreError::io("failed to resolve task-store lifecycle lock", path, source)
    })?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(unsafe_lock_path(
            path,
            "persistent lock resolves outside the workspace",
        ));
    }
    let file_metadata = file.metadata().map_err(|source| {
        DaemonCoreError::io(
            "failed to inspect open task-store lifecycle lock",
            path,
            source,
        )
    })?;
    if !same_file_identity(&path_metadata, &file_metadata) {
        return Err(unsafe_lock_path(
            path,
            "persistent lock changed while it was opened",
        ));
    }
    Ok(())
}

fn unsafe_lock_path(path: &Path, message: &'static str) -> DaemonCoreError {
    DaemonCoreError::io(
        message,
        path,
        io::Error::new(io::ErrorKind::InvalidData, message),
    )
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.is_file() == right.is_file() && left.len() == right.len()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn daemon_first_prevents_retention_lease_without_waiting() {
        let root = tempdir().unwrap();
        let daemon_lease = acquire_daemon_task_store_lease(root.path()).unwrap();

        assert!(try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .is_none());

        drop(daemon_lease);
        assert!(try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .is_some());
    }

    #[test]
    fn writer_first_prevents_retention_lease_without_waiting() {
        let root = tempdir().unwrap();
        let writer_lease = acquire_task_store_writer_lease(root.path()).unwrap();

        assert!(try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .is_none());

        drop(writer_lease);
        assert!(try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .is_some());
    }

    #[test]
    fn cloned_writer_lease_unlocks_only_after_final_drop() {
        let root = tempdir().unwrap();
        let first = acquire_task_store_writer_lease(root.path()).unwrap();
        let final_clone = first.clone();
        drop(first);

        assert!(try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .is_none());

        drop(final_clone);
        assert!(try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .is_some());
    }

    #[test]
    fn only_one_daemon_instance_can_own_a_workspace() {
        let root = tempdir().unwrap();
        let first = acquire_daemon_instance_lease(root.path()).unwrap();

        let error = acquire_daemon_instance_lease(root.path()).unwrap_err();
        assert!(matches!(
            error,
            DaemonCoreError::DaemonInstanceAlreadyRunning { path }
                if path == daemon_instance_lock_path(root.path())
        ));

        drop(first);
        assert!(acquire_daemon_instance_lease(root.path()).is_ok());
    }

    #[test]
    fn instance_and_task_store_leases_are_independent() {
        let root = tempdir().unwrap();
        let _instance = acquire_daemon_instance_lease(root.path()).unwrap();
        let lifecycle = acquire_daemon_task_store_lease(root.path()).unwrap();

        assert!(try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .is_none());
        assert!(matches!(
            acquire_daemon_instance_lease(root.path()),
            Err(DaemonCoreError::DaemonInstanceAlreadyRunning { .. })
        ));

        drop(lifecycle);
        assert!(try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .is_some());
    }

    #[test]
    fn retention_first_blocks_daemon_start_until_release() {
        let root = tempdir().unwrap();
        let retention_lease = try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .unwrap();
        let root_path = root.path().to_path_buf();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let daemon = thread::spawn(move || {
            let lease = acquire_daemon_task_store_lease(&root_path).unwrap();
            acquired_tx.send(()).unwrap();
            lease
        });

        assert!(matches!(
            acquired_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(retention_lease);
        acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(daemon.join().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_lifecycle_lock_is_rejected_without_touching_target() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let target = outside.path().join("keep");
        fs::write(&target, b"keep").unwrap();
        let daemon_root = daemon_dir(root.path());
        fs::create_dir_all(&daemon_root).unwrap();
        symlink(&target, task_store_lifecycle_lock_path(root.path())).unwrap();

        assert!(acquire_daemon_task_store_lease(root.path()).is_err());
        assert_eq!(fs::read(target).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_instance_lock_is_rejected_without_touching_target() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let target = outside.path().join("keep");
        fs::write(&target, b"keep").unwrap();
        let daemon_root = daemon_dir(root.path());
        fs::create_dir_all(&daemon_root).unwrap();
        symlink(&target, daemon_instance_lock_path(root.path())).unwrap();

        assert!(acquire_daemon_instance_lease(root.path()).is_err());
        assert_eq!(fs::read(target).unwrap(), b"keep");
    }
}
