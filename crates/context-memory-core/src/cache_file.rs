use std::fs::File;
use std::io;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use packet28_state_fs::{FileAccess, StateDir, StateFile};
use thiserror::Error;

/// The safety invariant violated by a cache filesystem entry.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum CachePathViolation {
    /// The entry is a symbolic link or a Windows reparse point.
    #[error("symbolic links and reparse points are not allowed")]
    SymbolicLink,

    /// The opened entry is not a regular file.
    #[error("only regular files are allowed")]
    NotRegularFile,

    /// The opened entry has another hard-link alias.
    #[error("cache files must have exactly one hard link")]
    MultipleHardLinks,

    /// The path stopped naming the file descriptor retained by this process.
    #[error("the path was replaced while the file was in use")]
    Replaced,
}

#[derive(Debug, Error)]
pub(crate) enum CacheFileError {
    #[error("unsafe cache path `{path}`: {violation}")]
    Unsafe {
        path: PathBuf,
        violation: CachePathViolation,
    },

    #[error("cache file operation failed for `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl CacheFileError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    pub(crate) fn into_io(self) -> io::Error {
        let kind = match &self {
            Self::Unsafe { .. } => io::ErrorKind::PermissionDenied,
            Self::Io { source, .. } => source.kind(),
        };
        io::Error::new(kind, self)
    }
}

#[derive(Debug)]
pub(crate) struct CacheFile {
    retained: StateFile,
    path: PathBuf,
}

impl CacheFile {
    pub(crate) fn validate_attachment(&self) -> Result<(), CacheFileError> {
        self.retained
            .validate_attachment()
            .map_err(|source| map_state_error(&self.path, source))
    }

    pub(crate) fn ensure_same_file(&self, other: &Self) -> Result<(), CacheFileError> {
        self.retained
            .ensure_same_file(&other.retained)
            .map_err(|source| map_state_error(&self.path, source))
    }

    pub(crate) fn sync_parent(&self) -> Result<(), CacheFileError> {
        self.retained
            .sync_parent()
            .map_err(|source| map_state_error(&self.path, source))
    }
}

impl Deref for CacheFile {
    type Target = File;

    fn deref(&self) -> &Self::Target {
        self.retained.file()
    }
}

impl DerefMut for CacheFile {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.retained.file_mut()
    }
}

#[derive(Debug)]
pub(crate) struct OpenedCacheFile {
    pub(crate) file: CacheFile,
    pub(crate) created: bool,
}

pub(crate) fn open_or_create_regular_file(path: &Path) -> Result<OpenedCacheFile, CacheFileError> {
    open_or_create_regular_file_with(path, FileAccess::ReadWrite, || Ok(()))
}

pub(crate) fn open_or_create_regular_file_for_append(
    path: &Path,
) -> Result<OpenedCacheFile, CacheFileError> {
    open_or_create_regular_file_with(path, FileAccess::Append, || Ok(()))
}

fn open_or_create_regular_file_with<F>(
    path: &Path,
    access: FileAccess,
    after_create_collision: F,
) -> Result<OpenedCacheFile, CacheFileError>
where
    F: FnOnce() -> io::Result<()>,
{
    let (directory, name) = cache_directory(path, true)?;
    if directory
        .open_existing(&name, access)
        .map_err(|source| map_state_error(path, source))?
        .is_some()
    {
        after_create_collision().map_err(|source| CacheFileError::io(path, source))?;
    }
    let opened = directory
        .open_or_create(&name, access)
        .map_err(|source| map_state_error(path, source))?;
    Ok(OpenedCacheFile {
        file: CacheFile {
            retained: opened.file,
            path: path.to_path_buf(),
        },
        created: opened.created,
    })
}

pub(crate) fn open_existing_regular_file_read_only(
    path: &Path,
) -> Result<CacheFile, CacheFileError> {
    open_existing_regular_file_with(path, FileAccess::ReadOnly)
}

pub(crate) fn open_existing_regular_file(path: &Path) -> Result<CacheFile, CacheFileError> {
    open_existing_regular_file_with(path, FileAccess::ReadWrite)
}

fn open_existing_regular_file_with(
    path: &Path,
    access: FileAccess,
) -> Result<CacheFile, CacheFileError> {
    let (directory, name) = cache_directory(path, false)?;
    let retained = directory
        .open_existing(&name, access)
        .map_err(|source| map_state_error(path, source))?
        .ok_or_else(|| {
            CacheFileError::io(
                path,
                io::Error::new(io::ErrorKind::NotFound, "cache file does not exist"),
            )
        })?;
    Ok(CacheFile {
        retained,
        path: path.to_path_buf(),
    })
}

pub(crate) fn read_regular_file_bounded(
    path: &Path,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>, CacheFileError> {
    let (directory, name) = cache_directory(path, false)?;
    directory
        .read_bounded(&name, max_bytes)
        .map_err(|source| map_state_error(path, source))
}

pub(crate) fn regular_file_len(path: &Path) -> Result<Option<u64>, CacheFileError> {
    let (directory, name) = cache_directory(path, false)?;
    let Some(file) = directory
        .open_existing(&name, FileAccess::ReadOnly)
        .map_err(|source| map_state_error(path, source))?
    else {
        return Ok(None);
    };
    file.len()
        .map(Some)
        .map_err(|source| CacheFileError::io(path, source))
}

pub(crate) fn write_regular_file_atomically<B, A, P>(
    path: &Path,
    bytes: &[u8],
    before_temp_open: B,
    after_temp_sync: A,
    before_publish: P,
) -> Result<(), CacheFileError>
where
    B: FnMut(&Path) -> io::Result<()>,
    A: FnOnce(&Path) -> io::Result<()>,
    P: FnOnce(&Path, &Path) -> io::Result<()>,
{
    let (directory, name) = cache_directory(path, true)?;
    directory
        .write_atomic_with_observers(
            &name,
            bytes,
            before_temp_open,
            after_temp_sync,
            before_publish,
        )
        .map_err(|source| map_state_error(path, source))
}

pub(crate) fn validate_file_attachment(
    file: &CacheFile,
    _path: &Path,
) -> Result<(), CacheFileError> {
    file.validate_attachment()
}

pub(crate) fn validate_same_file(
    expected: &CacheFile,
    actual: &CacheFile,
    _path: &Path,
) -> Result<(), CacheFileError> {
    expected.ensure_same_file(actual)
}

fn cache_directory(path: &Path, create: bool) -> Result<(StateDir, String), CacheFileError> {
    let parent = path.parent().ok_or_else(|| {
        CacheFileError::io(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "cache path has no parent"),
        )
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            CacheFileError::io(
                path,
                io::Error::new(io::ErrorKind::InvalidInput, "cache file name is not UTF-8"),
            )
        })?
        .to_string();
    let directory = if parent.file_name().is_some_and(|name| name == ".packet28") {
        let root = parent.parent().ok_or_else(|| {
            CacheFileError::io(
                path,
                io::Error::new(io::ErrorKind::InvalidInput, "cache root has no parent"),
            )
        })?;
        StateDir::open(root, &[".packet28"], create)
    } else {
        StateDir::open(parent, &[], false)
    }
    .map_err(|source| map_state_error(path, source))?;
    Ok((directory, name))
}

fn map_state_error(path: &Path, source: io::Error) -> CacheFileError {
    if let Some(violation) = classify_path_violation(path, &source) {
        CacheFileError::Unsafe {
            path: path.to_path_buf(),
            violation,
        }
    } else {
        CacheFileError::io(path, source)
    }
}

fn classify_path_violation(path: &Path, source: &io::Error) -> Option<CachePathViolation> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Some(CachePathViolation::SymbolicLink);
        }
        if !metadata.file_type().is_file() {
            return Some(CachePathViolation::NotRegularFile);
        }
    }
    let message = source.to_string();
    if message.contains("multiple hard links") {
        return Some(CachePathViolation::MultipleHardLinks);
    }
    if message.contains("was replaced") || message.contains("do not have the same identity") {
        return Some(CachePathViolation::Replaced);
    }
    if message.contains("not a regular file") {
        return Some(CachePathViolation::NotRegularFile);
    }
    None
}

#[cfg(test)]
pub(crate) fn open_or_create_regular_file_for_test<F>(
    path: &Path,
    after_create_collision: F,
) -> Result<OpenedCacheFile, CacheFileError>
where
    F: FnOnce() -> io::Result<()>,
{
    open_or_create_regular_file_with(path, FileAccess::ReadWrite, after_create_collision)
}
