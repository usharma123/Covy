use std::collections::hash_map::RandomState;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::hash::BuildHasher;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use thiserror::Error;

const MAX_UNIQUE_TEMP_ATTEMPTS: usize = 16;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static PROCESS_NONCE: OnceLock<[u64; 2]> = OnceLock::new();

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

    #[error(
        "failed to allocate a unique cache temporary file in `{directory}` \
         after {attempts} attempts"
    )]
    TempNameExhausted { directory: PathBuf, attempts: usize },
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
            Self::TempNameExhausted { .. } => io::ErrorKind::AlreadyExists,
        };
        io::Error::new(kind, self)
    }
}

#[derive(Debug)]
pub(crate) struct OpenedCacheFile {
    pub(crate) file: File,
    pub(crate) created: bool,
}

#[derive(Debug)]
pub(crate) struct ExclusiveCacheTemp {
    file: File,
    path: PathBuf,
    published: bool,
}

impl ExclusiveCacheTemp {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    pub(crate) fn validate_attachment(&self) -> Result<(), CacheFileError> {
        validate_file_attachment(&self.file, &self.path)
    }

    pub(crate) fn mark_published(&mut self) {
        self.published = true;
    }
}

impl Drop for ExclusiveCacheTemp {
    fn drop(&mut self) {
        if !self.published {
            // Removing a final-component symlink removes the link itself, not
            // its target. Identity-safe cleanup against an actively writable
            // parent directory requires descriptor-relative unlink support.
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn create_unique_cache_temp_with<F>(
    target: &Path,
    mut before_open: F,
) -> Result<ExclusiveCacheTemp, CacheFileError>
where
    F: FnMut(&Path) -> io::Result<()>,
{
    let directory = target.parent().ok_or_else(|| CacheFileError::Io {
        path: target.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "cache path has no parent"),
    })?;

    for _ in 0..MAX_UNIQUE_TEMP_ATTEMPTS {
        let path = unique_temp_path(target)?;
        before_open(&path).map_err(|source| CacheFileError::Io {
            path: path.clone(),
            source,
        })?;
        match create_new_regular_file(&path) {
            Ok(file) => {
                validate_file_attachment(&file, &path)?;
                return Ok(ExclusiveCacheTemp {
                    file,
                    path,
                    published: false,
                });
            }
            Err(CacheFileError::Io { source, .. })
                if source.kind() == io::ErrorKind::AlreadyExists =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
    }

    Err(CacheFileError::TempNameExhausted {
        directory: directory.to_path_buf(),
        attempts: MAX_UNIQUE_TEMP_ATTEMPTS,
    })
}

pub(crate) fn open_or_create_regular_file(path: &Path) -> Result<OpenedCacheFile, CacheFileError> {
    open_or_create_regular_file_with(path, || Ok(()))
}

fn open_or_create_regular_file_with<F>(
    path: &Path,
    after_create_collision: F,
) -> Result<OpenedCacheFile, CacheFileError>
where
    F: FnOnce() -> io::Result<()>,
{
    match create_new_regular_file(path) {
        Ok(file) => {
            validate_file_attachment(&file, path)?;
            Ok(OpenedCacheFile {
                file,
                created: true,
            })
        }
        Err(CacheFileError::Io { source, .. }) if source.kind() == io::ErrorKind::AlreadyExists => {
            after_create_collision().map_err(|source| CacheFileError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let file = open_existing_regular_file(path)?;
            validate_file_attachment(&file, path)?;
            Ok(OpenedCacheFile {
                file,
                created: false,
            })
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
pub(crate) fn validate_file_attachment(file: &File, path: &Path) -> Result<(), CacheFileError> {
    use std::os::unix::fs::MetadataExt;

    let expected = file_identity(file, path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Err(unsafe_path(path, CachePathViolation::Replaced));
        }
        Err(source) => return Err(CacheFileError::io(path, source)),
    };
    if metadata.file_type().is_symlink() {
        return Err(unsafe_path(path, CachePathViolation::SymbolicLink));
    }
    if !metadata.is_file() {
        return Err(unsafe_path(path, CachePathViolation::NotRegularFile));
    }
    if metadata.nlink() != 1 {
        return Err(unsafe_path(path, CachePathViolation::MultipleHardLinks));
    }
    let actual = FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    if expected != actual {
        return Err(unsafe_path(path, CachePathViolation::Replaced));
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn validate_file_attachment(file: &File, path: &Path) -> Result<(), CacheFileError> {
    let expected = file_identity(file, path)?;
    let attached = match open_existing_regular_file(path) {
        Ok(file) => file,
        Err(CacheFileError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Err(unsafe_path(path, CachePathViolation::Replaced));
        }
        Err(error) => return Err(error),
    };
    let actual = file_identity(&attached, path)?;
    if expected != actual {
        return Err(unsafe_path(path, CachePathViolation::Replaced));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn validate_file_attachment(_file: &File, path: &Path) -> Result<(), CacheFileError> {
    Err(CacheFileError::io(
        path,
        io::Error::new(
            io::ErrorKind::Unsupported,
            "cache file identity checks are unsupported on this platform",
        ),
    ))
}

fn unique_temp_path(target: &Path) -> Result<PathBuf, CacheFileError> {
    let directory = target.parent().ok_or_else(|| CacheFileError::Io {
        path: target.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "cache path has no parent"),
    })?;
    let file_name = target.file_name().ok_or_else(|| CacheFileError::Io {
        path: target.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "cache path has no file name"),
    })?;
    let nonce = process_nonce();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut temporary_name = OsString::from(".");
    temporary_name.push(file_name);
    temporary_name.push(format!(
        ".tmp-{:016x}{:016x}-{}-{counter}",
        nonce[0],
        nonce[1],
        std::process::id()
    ));
    Ok(directory.join(temporary_name))
}

fn process_nonce() -> &'static [u64; 2] {
    PROCESS_NONCE.get_or_init(|| {
        [
            RandomState::new().hash_one(0_u8),
            RandomState::new().hash_one(1_u8),
        ]
    })
}

fn create_new_regular_file(path: &Path) -> Result<File, CacheFileError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    configure_no_follow(&mut options);
    let file = options.open(path).map_err(|source| CacheFileError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    validate_regular_file(&file, path)?;
    Ok(file)
}

fn open_existing_regular_file(path: &Path) -> Result<File, CacheFileError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    configure_no_follow(&mut options);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(source) => {
            if let Some(violation) = classify_path_violation(path) {
                return Err(unsafe_path(path, violation));
            }
            return Err(CacheFileError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    validate_regular_file(&file, path)?;
    Ok(file)
}

#[cfg(unix)]
fn configure_no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
}

#[cfg(windows)]
fn configure_no_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(not(any(unix, windows)))]
fn configure_no_follow(_options: &mut OpenOptions) {}

fn validate_regular_file(file: &File, path: &Path) -> Result<(), CacheFileError> {
    let metadata = file.metadata().map_err(|source| CacheFileError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(unsafe_path(path, CachePathViolation::NotRegularFile));
    }
    validate_platform_file(&metadata, file, path)
}

#[cfg(unix)]
fn validate_platform_file(
    metadata: &fs::Metadata,
    _file: &File,
    path: &Path,
) -> Result<(), CacheFileError> {
    use std::os::unix::fs::MetadataExt;

    if metadata.nlink() != 1 {
        return Err(unsafe_path(path, CachePathViolation::MultipleHardLinks));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_platform_file(
    metadata: &fs::Metadata,
    file: &File,
    path: &Path,
) -> Result<(), CacheFileError> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(unsafe_path(path, CachePathViolation::SymbolicLink));
    }
    let information =
        winapi_util::file::information(file).map_err(|source| CacheFileError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if information.number_of_links() != 1 {
        return Err(unsafe_path(path, CachePathViolation::MultipleHardLinks));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn validate_platform_file(
    _metadata: &fs::Metadata,
    _file: &File,
    _path: &Path,
) -> Result<(), CacheFileError> {
    Ok(())
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
fn file_identity(file: &File, path: &Path) -> Result<FileIdentity, CacheFileError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata().map_err(|source| CacheFileError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    volume: u64,
    index: u64,
}

#[cfg(windows)]
fn file_identity(file: &File, path: &Path) -> Result<FileIdentity, CacheFileError> {
    let information =
        winapi_util::file::information(file).map_err(|source| CacheFileError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(FileIdentity {
        volume: information.volume_serial_number(),
        index: information.file_index(),
    })
}

fn classify_path_violation(path: &Path) -> Option<CachePathViolation> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() {
        return Some(CachePathViolation::SymbolicLink);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Some(CachePathViolation::SymbolicLink);
        }
    }
    (!metadata.is_file()).then_some(CachePathViolation::NotRegularFile)
}

fn unsafe_path(path: &Path, violation: CachePathViolation) -> CacheFileError {
    CacheFileError::Unsafe {
        path: path.to_path_buf(),
        violation,
    }
}

#[cfg(test)]
pub(crate) fn open_or_create_regular_file_for_test<F>(
    path: &Path,
    after_create_collision: F,
) -> Result<OpenedCacheFile, CacheFileError>
where
    F: FnOnce() -> io::Result<()>,
{
    open_or_create_regular_file_with(path, after_create_collision)
}
