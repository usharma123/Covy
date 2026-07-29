//! Cross-process ownership for daemon task storage.
//!
//! This module provides the lock primitive; enforcement requires daemon and
//! maintenance callers to acquire their corresponding lease. Daemons use a
//! shared lease and destructive maintenance uses the exclusive lease so the
//! two operations can be serialized before runtime readiness files exist.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use packet28_daemon_protocol::paths::daemon_dir;

use crate::{DaemonCoreError, Result};

const TASK_STORE_LIFECYCLE_LOCK_FILE_NAME: &str = ".task-store-lifecycle.lock";

/// RAII ownership of the workspace task-store lifecycle lock.
///
/// Dropping the value releases the underlying advisory lock. The persistent
/// lock file itself is deliberately never unlinked, which prevents concurrent
/// processes from locking different inodes for the same workspace.
#[derive(Debug)]
pub struct TaskStoreLease {
    file: File,
    path: PathBuf,
}

impl TaskStoreLease {
    /// Returns the persistent lock-file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TaskStoreLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Returns the persistent lifecycle lock path for a workspace task store.
pub fn task_store_lifecycle_lock_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(TASK_STORE_LIFECYCLE_LOCK_FILE_NAME)
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
    let (path, file) = open_task_store_lifecycle_lock(root)?;
    FileExt::lock_shared(&file).map_err(|source| {
        DaemonCoreError::io("failed to acquire daemon task-store lease", &path, source)
    })?;
    validate_open_lock_file(root, &path, &file)?;
    Ok(TaskStoreLease { file, path })
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
    let (path, file) = open_task_store_lifecycle_lock(root)?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            validate_open_lock_file(root, &path, &file)?;
            Ok(Some(TaskStoreLease { file, path }))
        }
        Err(source) if source.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(source) => Err(DaemonCoreError::io(
            "failed to acquire task-retention lease",
            &path,
            source,
        )),
    }
}

fn open_task_store_lifecycle_lock(root: &Path) -> Result<(PathBuf, File)> {
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

    let path = task_store_lifecycle_lock_path(root);
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(unsafe_lock_path(
                &path,
                "task-store lifecycle lock is not a regular file",
            ));
        }
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|source| {
            DaemonCoreError::io("failed to open task-store lifecycle lock", &path, source)
        })?;
    validate_open_lock_file(root, &path, &file)?;
    Ok((path, file))
}

fn validate_open_lock_file(root: &Path, path: &Path, file: &File) -> Result<()> {
    let path_metadata = fs::symlink_metadata(path).map_err(|source| {
        DaemonCoreError::io("failed to inspect task-store lifecycle lock", path, source)
    })?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(unsafe_lock_path(
            path,
            "task-store lifecycle lock is not a regular file",
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
            "task-store lifecycle lock resolves outside the workspace",
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
            "task-store lifecycle lock changed while it was opened",
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
}
