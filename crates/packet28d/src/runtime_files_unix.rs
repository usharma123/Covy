//! Narrow Unix filesystem adapter for retained daemon index state.
//!
//! Every public operation is relative to a caller-owned directory descriptor,
//! rejects path separators, and uses `O_NOFOLLOW` for opens. The unsafe surface
//! is confined here so the higher-level clear-state protocol can remain safe
//! Rust and reason in terms of retained handles.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

pub(crate) fn open_directory_path(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

pub(crate) fn create_directory_at(parent: &fs::File, name: &str) -> io::Result<bool> {
    let name = component_c_string(OsStr::new(name))?;
    // SAFETY: `parent` owns a live directory descriptor, `name` is a
    // NUL-terminated single component, and no pointer escapes this call.
    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::AlreadyExists {
        Ok(false)
    } else {
        Err(error)
    }
}

pub(crate) fn open_directory_at(parent: &fs::File, name: &str) -> io::Result<fs::File> {
    open_file_at(
        parent,
        name,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
    )
}

pub(crate) fn open_file_at(parent: &fs::File, name: &str, flags: i32) -> io::Result<fs::File> {
    open_file_at_os(parent, OsStr::new(name), flags, 0o600)
}

pub(crate) fn open_lock_file_at(parent: &fs::File, name: &str) -> io::Result<fs::File> {
    open_file_at(
        parent,
        name,
        libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW,
    )
}

pub(crate) fn rename_file_at(
    directory: &fs::File,
    source: &str,
    destination: &str,
) -> io::Result<()> {
    let source = component_c_string(OsStr::new(source))?;
    let destination = component_c_string(OsStr::new(destination))?;
    // SAFETY: both names are retained NUL-terminated components and
    // `directory` owns the live descriptor used for both sides of the rename.
    let result = unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
        )
    };
    cvt_zero(result)
}

pub(crate) fn link_file_at(
    directory: &fs::File,
    source: &str,
    destination: &str,
) -> io::Result<()> {
    let source = component_c_string(OsStr::new(source))?;
    let destination = component_c_string(OsStr::new(destination))?;
    // SAFETY: both names are retained NUL-terminated components and
    // `directory` owns the live descriptor used for both sides of the link.
    let result = unsafe {
        libc::linkat(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
            0,
        )
    };
    cvt_zero(result)
}

pub(crate) fn remove_file_at(directory: &fs::File, name: &str) {
    let _ = remove_file_if_exists_at(directory, name);
}

pub(crate) fn remove_file_if_exists_at(directory: &fs::File, name: &str) -> io::Result<()> {
    match unlink_at_os(directory, OsStr::new(name), 0) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Removes `name` and its descendants without resolving through a path from
/// the workspace root.
///
/// Directories are opened with `O_NOFOLLOW`; before each directory name is
/// removed, its current binding is re-opened and compared with the retained
/// handle. A concurrent replacement therefore fails closed.
pub(crate) fn remove_directory_tree_at(parent: &fs::File, name: &str) -> io::Result<()> {
    remove_entry_at(parent, OsStr::new(name))
}

pub(crate) fn remove_retained_directory_tree_at(
    parent: &fs::File,
    name: &str,
    expected: &fs::File,
) -> io::Result<()> {
    let name = OsStr::new(name);
    let current = open_file_at_os(
        parent,
        name,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        0,
    )?;
    ensure_same_object(expected, &current)?;
    drop(current);
    remove_open_directory_tree_at(parent, name, expected)
}

pub(crate) fn read_directory_names(directory: &fs::File) -> io::Result<Vec<OsString>> {
    let mut stream = DirectoryStream::open(directory)?;
    let mut entries = Vec::new();
    while let Some(name) = stream.next_name() {
        let name = name?;
        if name.as_bytes() != b"." && name.as_bytes() != b".." {
            entries.push(name);
        }
    }
    Ok(entries)
}

fn open_file_at_os(
    parent: &fs::File,
    name: &OsStr,
    flags: i32,
    mode: libc::c_uint,
) -> io::Result<fs::File> {
    let name = component_c_string(name)?;
    // SAFETY: `parent` owns a live descriptor, `name` is a retained
    // NUL-terminated single component, and a successful `openat` returns a new
    // descriptor whose ownership is transferred exactly once into `File`.
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC,
            mode,
        )
    };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: `descriptor` is the unique successful result of `openat`.
        Ok(unsafe { fs::File::from_raw_fd(descriptor) })
    }
}

fn remove_entry_at(parent: &fs::File, name: &OsStr) -> io::Result<()> {
    let opened = open_file_at_os(
        parent,
        name,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        0,
    );
    match opened {
        Ok(directory) => remove_open_directory_tree_at(parent, name, &directory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) if is_non_directory_or_symlink(&error) => unlink_at_os(parent, name, 0),
        Err(error) => Err(error),
    }
}

fn remove_open_directory_tree_at(
    parent: &fs::File,
    name: &OsStr,
    directory: &fs::File,
) -> io::Result<()> {
    remove_directory_contents(directory)?;
    let current = open_file_at_os(
        parent,
        name,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        0,
    )
    .map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("directory binding changed before removal: {error}"),
        )
    })?;
    ensure_same_object(directory, &current)?;
    drop(current);
    unlink_at_os(parent, name, libc::AT_REMOVEDIR)
}

fn remove_directory_contents(directory: &fs::File) -> io::Result<()> {
    let mut stream = DirectoryStream::open(directory)?;
    let mut entries = Vec::new();
    while let Some(name) = stream.next_name() {
        let name = name?;
        if name.as_bytes() != b"." && name.as_bytes() != b".." {
            entries.push(name);
        }
    }
    drop(stream);
    for name in entries {
        remove_entry_at(directory, &name)?;
    }
    Ok(())
}

fn unlink_at_os(directory: &fs::File, name: &OsStr, flags: i32) -> io::Result<()> {
    let name = component_c_string(name)?;
    // SAFETY: `directory` owns a live descriptor and `name` is a retained
    // NUL-terminated component. `flags` is either zero or `AT_REMOVEDIR`.
    let result = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), flags) };
    cvt_zero(result)
}

fn ensure_same_object(expected: &fs::File, actual: &fs::File) -> io::Result<()> {
    let expected = expected.metadata()?;
    let actual = actual.metadata()?;
    if expected.dev() == actual.dev() && expected.ino() == actual.ino() {
        Ok(())
    } else {
        Err(io::Error::other(
            "directory binding changed during retained tree removal",
        ))
    }
}

fn is_non_directory_or_symlink(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotADirectory || error.raw_os_error() == Some(libc::ELOOP)
}

fn component_c_string(value: &OsStr) -> io::Result<CString> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path must be one non-special component",
        ));
    }
    CString::new(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

fn cvt_zero(result: libc::c_int) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

struct DirectoryStream(*mut libc::DIR);

impl DirectoryStream {
    fn open(directory: &fs::File) -> io::Result<Self> {
        // SAFETY: `directory` owns a live descriptor. A successful `dup`
        // returns a new descriptor that is transferred to `fdopendir`.
        let descriptor = unsafe { libc::dup(directory.as_raw_fd()) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `descriptor` is uniquely owned here. On success,
        // `fdopendir` assumes ownership; on failure we close it below.
        let stream = unsafe { libc::fdopendir(descriptor) };
        if stream.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: `fdopendir` failed and did not assume ownership.
            let _ = unsafe { libc::close(descriptor) };
            Err(error)
        } else {
            // SAFETY: `stream` is the newly owned live directory stream.
            // `dup` shares the retained descriptor's directory offset, so
            // reset it before every independent enumeration.
            unsafe {
                libc::rewinddir(stream);
            }
            Ok(Self(stream))
        }
    }

    fn next_name(&mut self) -> Option<io::Result<OsString>> {
        // POSIX uses a null return for both end-of-directory and failure.
        // Clearing errno first makes the two outcomes distinguishable.
        // SAFETY: `errno_location` returns this thread's live errno slot.
        unsafe {
            *errno_location() = 0;
        }
        // SAFETY: `self.0` remains a live, exclusively borrowed `DIR*` until
        // this guard is dropped. The entry bytes are copied before the next
        // `readdir` call.
        let entry = unsafe { libc::readdir(self.0) };
        if entry.is_null() {
            // SAFETY: errno is read immediately after the failed `readdir`
            // without an intervening libc call.
            let errno = unsafe { *errno_location() };
            return (errno != 0).then(|| Err(io::Error::from_raw_os_error(errno)));
        }
        // SAFETY: POSIX guarantees `d_name` is NUL-terminated for the entry
        // returned by `readdir`; it is copied into an owned `OsString`.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        Some(Ok(OsString::from_vec(name.to_bytes().to_vec())))
    }
}

extern "C" {
    #[cfg_attr(
        any(
            target_os = "macos",
            target_os = "ios",
            target_os = "tvos",
            target_os = "watchos",
            target_os = "visionos",
            target_os = "freebsd"
        ),
        link_name = "__error"
    )]
    #[cfg_attr(
        any(
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "android",
            target_os = "espidf",
            target_os = "vxworks",
            target_os = "cygwin",
            target_env = "newlib"
        ),
        link_name = "__errno"
    )]
    #[cfg_attr(
        any(target_os = "solaris", target_os = "illumos"),
        link_name = "___errno"
    )]
    #[cfg_attr(target_os = "haiku", link_name = "_errnop")]
    #[cfg_attr(
        any(
            target_os = "linux",
            target_os = "hurd",
            target_os = "redox",
            target_os = "dragonfly",
            target_os = "emscripten"
        ),
        link_name = "__errno_location"
    )]
    #[cfg_attr(target_os = "aix", link_name = "_Errno")]
    #[cfg_attr(target_os = "nto", link_name = "__get_errno_ptr")]
    fn errno_location() -> *mut libc::c_int;
}

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the live `DIR*` returned by
        // `fdopendir`, and closes it exactly once.
        let _ = unsafe { libc::closedir(self.0) };
    }
}
