//! Directory-capability filesystem operations used by retention.
//!
//! All mutation stays relative to retained directory descriptors. Symlinks are
//! never followed, and deletion first moves the exact inode to a private
//! tombstone name before unlinking it.

use std::borrow::Cow;
#[cfg(test)]
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
#[cfg(target_vendor = "apple")]
use std::os::unix::io::AsRawFd as _;
use std::os::unix::io::{AsFd, OwnedFd};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::{self as rfs, AtFlags, Dir, FileType, Mode, OFlags, RawMode, RenameFlags};

use crate::retention::FileIdentity;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static PROCESS_NONCE: OnceLock<[u8; 16]> = OnceLock::new();

const MAX_UNIQUE_NAME_ATTEMPTS: usize = 16;
const MAX_AUTHENTICATED_READ_ATTEMPTS: usize = 16;
const MAX_CAPABILITY_DIRECTORY_ENTRIES: usize = 65_536;
const MAX_CAPABILITY_RECURSION_DEPTH: usize = 64;
const MAX_CAPABILITY_RECURSIVE_ENTRIES: usize = 65_536;

pub(super) const RETENTION_JOURNAL_WRITE_TEMP_PREFIX: &str = ".retention-journal-write";
pub(super) const RETENTION_JOURNAL_WRITE_DELETION_TEMP_PREFIX: &str =
    ".retention-journal-write-deleting";
pub(super) const TASK_REGISTRY_WRITE_TEMP_PREFIX: &str = ".task-registry-write";
pub(super) const TASK_REGISTRY_WRITE_DELETION_TEMP_PREFIX: &str = ".task-registry-write-deleting";
pub(super) const ACTIVE_TASK_WRITE_TEMP_PREFIX: &str = ".active-task-write";
pub(super) const ACTIVE_TASK_WRITE_DELETION_TEMP_PREFIX: &str = ".active-task-write-deleting";
pub(super) const DELETION_TEMP_PREFIX: &str = ".deleting";
pub(super) const NOREPLACE_PROBE_SOURCE_PREFIX: &str = ".noreplace-probe-source";
pub(super) const NOREPLACE_PROBE_DESTINATION_PREFIX: &str = ".noreplace-probe-destination";
#[cfg(test)]
pub(super) const TEST_ATOMIC_WRITE_TEMP_PREFIX: &str = ".test-atomic-write";
#[cfg(test)]
const TEST_ATOMIC_WRITE_DELETION_TEMP_PREFIX: &str = ".test-atomic-write-deleting";

#[cfg(test)]
#[derive(Clone, Copy)]
enum InjectedAtomicAfterRenameFailure {
    Other,
    Unsupported,
    Io,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum DirectoryInitializerKillPoint {
    BeforeModeCorrection,
    AfterModeCorrection,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum LockInitializerKillPoint {
    BeforeModeCorrection,
    AfterModeCorrection,
}

#[cfg(test)]
struct InjectedAuthenticatedReadAfterOpen {
    name: OsString,
    action: Box<dyn FnOnce()>,
}

#[cfg(test)]
std::thread_local! {
    static INJECT_ATOMIC_AFTER_RENAME_FOR:
        std::cell::RefCell<Option<(OsString, InjectedAtomicAfterRenameFailure)>> =
        const { std::cell::RefCell::new(None) };
    static INJECT_NOREPLACE_UNSUPPORTED: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
    static INJECT_DIRECTORY_CREATE_SYNC_FAILURE_FOR: std::cell::RefCell<Option<OsString>> =
        const { std::cell::RefCell::new(None) };
    static INJECT_NEW_ENTRY_CHMOD_FAILURE_FOR: std::cell::RefCell<Option<OsString>> =
        const { std::cell::RefCell::new(None) };
    static INJECT_PREFLIGHT_FIFO_SWAP_FOR: std::cell::RefCell<Option<OsString>> =
        const { std::cell::RefCell::new(None) };
    static INJECT_AUTHENTICATED_READ_AFTER_OPEN:
        std::cell::RefCell<Option<InjectedAuthenticatedReadAfterOpen>> =
        const { std::cell::RefCell::new(None) };
    static INJECT_NAME_MAX: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
    static KILL_DIRECTORY_INITIALIZER_FOR:
        std::cell::RefCell<Option<(OsString, DirectoryInitializerKillPoint)>> =
        const { std::cell::RefCell::new(None) };
    static KILL_LOCK_INITIALIZER_FOR:
        std::cell::RefCell<Option<(OsString, LockInitializerKillPoint)>> =
        const { std::cell::RefCell::new(None) };
    static INJECT_UNIQUE_NAMES: std::cell::RefCell<VecDeque<OsString>> =
        const { std::cell::RefCell::new(VecDeque::new()) };
    static INJECT_SYNC_RENAME_AFTER_DESTINATION_FOR:
        std::cell::RefCell<Option<(PathBuf, PathBuf)>> =
        const { std::cell::RefCell::new(None) };
    static OPEN_WORKSPACE_CALLS: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
}

#[cfg(all(test, target_vendor = "apple"))]
std::thread_local! {
    static INJECT_INHERITABLE_ACL_BEFORE_CREATE_FOR: std::cell::RefCell<Option<OsString>> =
        const { std::cell::RefCell::new(None) };
}

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

fn create_directory_exact(parent: &OwnedFd, name: &OsStr, mode: RawMode) -> io::Result<()> {
    rfs::mkdirat(parent, name, Mode::from_bits_truncate(mode)).map_err(io::Error::from)
}

fn create_file_exact(
    parent: &OwnedFd,
    name: &OsStr,
    flags: OFlags,
    mode: RawMode,
) -> io::Result<OwnedFd> {
    rfs::openat(parent, name, flags, Mode::from_bits_truncate(mode)).map_err(io::Error::from)
}

#[derive(Clone, Copy)]
struct TraversalLimits {
    max_depth: usize,
    max_entries: usize,
}

impl TraversalLimits {
    const DEFAULT: Self = Self {
        max_depth: MAX_CAPABILITY_RECURSION_DEPTH,
        max_entries: MAX_CAPABILITY_RECURSIVE_ENTRIES,
    };
}

struct TraversalBudget {
    limits: TraversalLimits,
    entries_seen: usize,
}

impl TraversalBudget {
    fn new(limits: TraversalLimits) -> Self {
        Self {
            limits,
            entries_seen: 0,
        }
    }

    fn consume(&mut self, depth: usize) -> io::Result<()> {
        if depth > self.limits.max_depth {
            return Err(capability_depth_limit_error(self.limits.max_depth));
        }
        if self.entries_seen >= self.limits.max_entries {
            return Err(capability_entry_limit_error(self.limits.max_entries));
        }
        self.entries_seen += 1;
        Ok(())
    }

    fn remaining_entries(&self) -> usize {
        self.limits.max_entries.saturating_sub(self.entries_seen)
    }
}

/// A retained descriptor for a real directory.
#[derive(Debug)]
pub(super) struct CapabilityDir {
    fd: OwnedFd,
    display_path: PathBuf,
    identity: FileIdentity,
    acl_policy: AclPolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AclPolicy {
    StrictEmpty,
    NamespaceAuthorityOnly,
}

#[derive(Debug)]
pub(super) struct AtomicWriteError {
    pub(super) source: io::Error,
    /// `true` once the target name points at the new file.
    pub(super) renamed: bool,
}

#[derive(Debug)]
pub(super) struct CapabilityFileRead {
    pub(super) bytes: Vec<u8>,
    pub(super) logical_bytes: u64,
    pub(super) allocated_bytes: u64,
    pub(super) identity: FileIdentity,
    pub(super) mode: RawMode,
}

struct AuthenticatedReadFile {
    file: File,
    identity: FileIdentity,
}

#[derive(Clone, Copy)]
struct AuthenticatedReadDescriptor {
    identity: FileIdentity,
    attached: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CapabilityEntryKind {
    Symlink,
    RegularFile,
    Directory,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CapabilityEntryMetadata {
    pub(super) kind: CapabilityEntryKind,
    pub(super) identity: FileIdentity,
    pub(super) logical_bytes: u64,
    pub(super) allocated_bytes: u64,
    pub(super) modified_unix_seconds: i64,
    pub(super) modified_subsec_nanos: u32,
    pub(super) link_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RemovalProgress {
    More,
    Complete,
}

impl AtomicWriteError {
    fn before_rename(source: io::Error) -> Self {
        Self {
            source,
            renamed: false,
        }
    }

    fn after_rename(source: io::Error) -> Self {
        Self {
            source,
            renamed: true,
        }
    }
}

#[cfg(test)]
pub(super) fn inject_atomic_after_rename_failure_once(name: &OsStr) {
    INJECT_ATOMIC_AFTER_RENAME_FOR.with(|configured| {
        *configured.borrow_mut() =
            Some((name.to_os_string(), InjectedAtomicAfterRenameFailure::Other));
    });
}

#[cfg(test)]
fn inject_atomic_after_rename_barrier_failure_once(
    name: &OsStr,
    failure: InjectedAtomicAfterRenameFailure,
) {
    INJECT_ATOMIC_AFTER_RENAME_FOR.with(|configured| {
        *configured.borrow_mut() = Some((name.to_os_string(), failure));
    });
}

#[cfg(test)]
pub(super) fn inject_noreplace_unsupported_once() {
    INJECT_NOREPLACE_UNSUPPORTED.with(|configured| configured.set(true));
}

#[cfg(test)]
pub(super) fn inject_authenticated_read_after_open_once(
    name: &OsStr,
    action: impl FnOnce() + 'static,
) {
    INJECT_AUTHENTICATED_READ_AFTER_OPEN.with(|configured| {
        *configured.borrow_mut() = Some(InjectedAuthenticatedReadAfterOpen {
            name: name.to_os_string(),
            action: Box::new(action),
        });
    });
}

#[cfg(test)]
pub(super) fn inject_directory_create_sync_failure_once(name: &OsStr) {
    INJECT_DIRECTORY_CREATE_SYNC_FAILURE_FOR.with(|configured| {
        *configured.borrow_mut() = Some(name.to_os_string());
    });
}

#[cfg(test)]
fn inject_new_entry_chmod_failure_once(name: &OsStr) {
    INJECT_NEW_ENTRY_CHMOD_FAILURE_FOR.with(|configured| {
        *configured.borrow_mut() = Some(name.to_os_string());
    });
}

#[cfg(test)]
fn inject_preflight_fifo_swap_once(name: &OsStr) {
    INJECT_PREFLIGHT_FIFO_SWAP_FOR.with(|configured| {
        *configured.borrow_mut() = Some(name.to_os_string());
    });
}

#[cfg(test)]
pub(crate) fn inject_name_max_once(name_max: usize) {
    INJECT_NAME_MAX.with(|configured| configured.set(Some(name_max)));
}

#[cfg(test)]
fn maybe_swap_preflight_to_fifo(display_path: &Path, name: &OsStr) -> io::Result<()> {
    let should_swap = INJECT_PREFLIGHT_FIFO_SWAP_FOR.with(|configured| {
        let mut configured = configured.borrow_mut();
        if configured.as_deref() == Some(name) {
            configured.take();
            true
        } else {
            false
        }
    });
    if !should_swap {
        return Ok(());
    }
    let path = display_path.join(name);
    std::fs::remove_file(&path)?;
    let status = std::process::Command::new("mkfifo").arg(&path).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "mkfifo failed while injecting a preflight race for {}",
            path.display()
        )))
    }
}

#[cfg(test)]
fn maybe_inject_authenticated_read_after_open(name: &OsStr) {
    let injection = INJECT_AUTHENTICATED_READ_AFTER_OPEN.with(|configured| {
        let mut configured = configured.borrow_mut();
        if configured
            .as_ref()
            .is_some_and(|injection| injection.name == name)
        {
            configured.take()
        } else {
            None
        }
    });
    if let Some(injection) = injection {
        (injection.action)();
    }
}

#[cfg(test)]
fn kill_directory_initializer_once(name: &OsStr, point: DirectoryInitializerKillPoint) {
    KILL_DIRECTORY_INITIALIZER_FOR.with(|configured| {
        *configured.borrow_mut() = Some((name.to_os_string(), point));
    });
}

#[cfg(test)]
fn maybe_kill_directory_initializer(name: &OsStr, point: DirectoryInitializerKillPoint) {
    let should_kill = KILL_DIRECTORY_INITIALIZER_FOR.with(|configured| {
        let mut configured = configured.borrow_mut();
        if configured
            .as_ref()
            .is_some_and(|(configured_name, configured_point)| {
                configured_name == name && *configured_point == point
            })
        {
            configured.take();
            true
        } else {
            false
        }
    });
    if should_kill {
        rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::KILL)
            .unwrap();
        // Signal delivery may occur just after `kill` returns. Keep this
        // process inert until SIGKILL takes effect instead of racing into test
        // code that assumes initialization continued.
        loop {
            std::thread::park();
        }
    }
}

#[cfg(test)]
fn kill_lock_initializer_once(name: &OsStr, point: LockInitializerKillPoint) {
    KILL_LOCK_INITIALIZER_FOR.with(|configured| {
        *configured.borrow_mut() = Some((name.to_os_string(), point));
    });
}

#[cfg(test)]
fn maybe_kill_lock_initializer(name: &OsStr, point: LockInitializerKillPoint) {
    let should_kill = KILL_LOCK_INITIALIZER_FOR.with(|configured| {
        let mut configured = configured.borrow_mut();
        if configured
            .as_ref()
            .is_some_and(|(configured_name, configured_point)| {
                configured_name == name && *configured_point == point
            })
        {
            configured.take();
            true
        } else {
            false
        }
    });
    if should_kill {
        rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::KILL)
            .unwrap();
        // Signal delivery may occur just after `kill` returns.
        loop {
            std::thread::park();
        }
    }
}

#[cfg(test)]
fn inject_unique_names(names: impl IntoIterator<Item = OsString>) {
    INJECT_UNIQUE_NAMES.with(|configured| {
        configured.borrow_mut().extend(names);
    });
}

#[cfg(all(test, target_vendor = "apple"))]
fn inject_inheritable_acl_before_create_once(name: &OsStr) {
    INJECT_INHERITABLE_ACL_BEFORE_CREATE_FOR.with(|configured| {
        *configured.borrow_mut() = Some(name.to_os_string());
    });
}

#[cfg(all(test, target_vendor = "apple"))]
fn maybe_inject_inheritable_acl_before_create(
    display_path: &Path,
    final_name: &OsStr,
) -> io::Result<()> {
    let should_inject = INJECT_INHERITABLE_ACL_BEFORE_CREATE_FOR.with(|configured| {
        let mut configured = configured.borrow_mut();
        if configured.as_deref() == Some(final_name) {
            configured.take();
            true
        } else {
            false
        }
    });
    if !should_inject {
        return Ok(());
    }
    let status = std::process::Command::new("chmod")
        .arg("+a")
        .arg(
            "everyone allow read,write,execute,delete,append,readattr,writeattr,readextattr,\
             writeextattr,readsecurity,writesecurity,chown,file_inherit,directory_inherit,\
             only_inherit",
        )
        .arg(display_path)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "failed to inject inheritable ACL on {}",
            display_path.display()
        )))
    }
}

#[cfg(test)]
pub(super) fn inject_sync_rename_after_destination_once(source: &Path, destination: &Path) {
    INJECT_SYNC_RENAME_AFTER_DESTINATION_FOR.with(|configured| {
        *configured.borrow_mut() = Some((source.to_path_buf(), destination.to_path_buf()));
    });
}

#[cfg(test)]
pub(super) fn reset_open_workspace_call_count() {
    OPEN_WORKSPACE_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
pub(super) fn open_workspace_call_count() -> u64 {
    OPEN_WORKSPACE_CALLS.with(std::cell::Cell::get)
}

impl CapabilityDir {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        Self::open_with_acl_policy(path, AclPolicy::StrictEmpty)
    }

    pub(super) fn open_private(path: &Path, expected_mode: RawMode) -> io::Result<Self> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "private capability directory has no parent: {}",
                    path.display()
                ),
            )
        })?;
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "private capability directory has no file name: {}",
                    path.display()
                ),
            )
        })?;
        let canonical_parent = std::fs::canonicalize(parent)?;
        let canonical = canonical_parent.join(name);
        let fd = rfs::open(&canonical, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
        let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        validate_owned_directory(
            &stat,
            canonical.as_os_str(),
            stat.st_dev as u64,
            "private capability directory",
        )?;
        validate_workspace_namespace_ancestors(&canonical, &stat)?;
        let directory = Self::from_fd(fd, canonical)?;
        directory.validate_private(expected_mode)?;
        directory.validate_display_path_attachment()?;
        Ok(directory)
    }

    pub(super) fn open_workspace(path: &Path) -> io::Result<Self> {
        #[cfg(test)]
        OPEN_WORKSPACE_CALLS.with(|calls| calls.set(calls.get().saturating_add(1)));
        Self::open_with_acl_policy(path, AclPolicy::NamespaceAuthorityOnly)
    }

    fn open_with_acl_policy(path: &Path, acl_policy: AclPolicy) -> io::Result<Self> {
        let canonical = std::fs::canonicalize(path)?;
        let fd = rfs::open(&canonical, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
        let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        validate_owned_directory(
            &stat,
            path.as_os_str(),
            stat.st_dev as u64,
            "capability root",
        )?;
        let acl_has_authority = match acl_policy {
            AclPolicy::StrictEmpty => has_extended_acl(&fd)?,
            AclPolicy::NamespaceAuthorityOnly => has_namespace_authority_acl(&fd)?,
        };
        if ((stat.st_mode as RawMode) & 0o022) != 0 || acl_has_authority {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability root has non-owner write authority and cannot be authenticated: {}",
                    canonical.display()
                ),
            ));
        }
        validate_workspace_namespace_ancestors(&canonical, &stat)?;
        Self::from_fd_with_acl_policy(fd, canonical, acl_policy)
    }

    pub(super) fn identity(&self) -> FileIdentity {
        self.identity
    }

    pub(super) fn display_path(&self) -> &Path {
        &self.display_path
    }

    pub(super) fn validate_display_path_attachment(&self) -> io::Result<()> {
        self.validate_mutation_authority()?;
        let metadata = std::fs::symlink_metadata(&self.display_path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability display path is no longer a real directory: {}",
                    self.display_path.display()
                ),
            ));
        }
        use std::os::unix::fs::MetadataExt as _;
        let attached = FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        if attached != self.identity {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability display path no longer names the retained directory: {}",
                    self.display_path.display()
                ),
            ));
        }
        Ok(())
    }

    pub(super) fn name_max(&self) -> io::Result<usize> {
        #[cfg(test)]
        if let Some(name_max) = INJECT_NAME_MAX.with(|configured| configured.take()) {
            return Ok(name_max);
        }
        // SAFETY: `self.fd` is an owned, live directory descriptor for the
        // duration of this call. `fpathconf` neither retains nor closes it.
        let value = unsafe {
            libc::fpathconf(
                std::os::fd::AsRawFd::as_raw_fd(&self.fd),
                libc::_PC_NAME_MAX,
            )
        };
        if value < 0 {
            return Err(io::Error::last_os_error());
        }
        usize::try_from(value).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "filesystem NAME_MAX does not fit in usize",
            )
        })
    }

    pub(super) fn duplicate(&self) -> io::Result<Self> {
        let fd = rustix::io::dup(&self.fd).map_err(io::Error::from)?;
        Self::from_fd_with_acl_policy(fd, self.display_path.clone(), self.acl_policy)
    }

    pub(super) fn open_dir(&self, name: &OsStr) -> io::Result<Self> {
        validate_normal_component(name)?;
        let fd =
            rfs::openat(&self.fd, name, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
        Self::from_child_fd(fd, self.display_path.join(name), self.identity.device)
    }

    pub(super) fn open_private_dir(&self, name: &OsStr, mode: RawMode) -> io::Result<Self> {
        self.open_dir_if_exists(name, mode)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "capability directory does not exist: {}",
                    Path::new(name).display()
                ),
            )
        })
    }

    pub(super) fn open_relative_dir(&self, relative: &Path) -> io::Result<Self> {
        let mut current = self.duplicate()?;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(invalid_component(relative));
            };
            current = current.open_dir(name)?;
        }
        Ok(current)
    }

    pub(super) fn ensure_dir(&self, name: &OsStr, mode: RawMode) -> io::Result<Self> {
        let directory = self.ensure_dir_open(name, mode)?;
        directory.validate_private(mode)?;
        Ok(directory)
    }

    pub(super) fn ensure_dir_open(&self, name: &OsStr, mode: RawMode) -> io::Result<Self> {
        validate_normal_component(name)?;
        self.validate_mutation_authority()?;
        if let Some(directory) = self.open_dir_if_exists(name, mode)? {
            return Ok(directory);
        }
        self.publish_initialized_directory(name, mode, true)
    }

    pub(super) fn create_dir(&self, name: &OsStr, mode: RawMode) -> io::Result<Self> {
        validate_normal_component(name)?;
        self.validate_mutation_authority()?;
        if self.entry_identity(name)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "capability directory already exists: {}",
                    Path::new(name).display()
                ),
            ));
        }
        self.publish_initialized_directory(name, mode, false)
    }

    pub(super) fn entry_identity(&self, name: &OsStr) -> io::Result<Option<FileIdentity>> {
        validate_normal_component(name)?;
        match rfs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(Some(identity_from_stat(&stat))),
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(source) => Err(source.into()),
        }
    }

    pub(super) fn metadata(&self) -> io::Result<CapabilityEntryMetadata> {
        self.validate_mutation_authority()?;
        let stat = rfs::fstat(&self.fd).map_err(io::Error::from)?;
        capability_entry_metadata(&stat)
    }

    pub(super) fn entry_metadata(
        &self,
        name: &OsStr,
    ) -> io::Result<Option<CapabilityEntryMetadata>> {
        validate_normal_component(name)?;
        let stat = match rfs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(source) => return Err(source.into()),
        };
        capability_entry_metadata(&stat).map(Some)
    }

    pub(super) fn authenticate_regular_file_for_scan(
        &self,
        name: &OsStr,
        expected: FileIdentity,
    ) -> io::Result<()> {
        validate_normal_component(name)?;
        let fd = rfs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        validate_owned_single_link_regular(
            &stat,
            name,
            self.identity.device,
            "capability scan file",
        )?;
        if identity_from_stat(&stat) != expected
            || ((stat.st_mode as RawMode) & 0o022) != 0
            || has_extended_acl(&fd)?
            || self.entry_identity(name)? != Some(expected)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability scan file is not an authentic private regular file: {}",
                    Path::new(name).display()
                ),
            ));
        }
        Ok(())
    }

    pub(super) fn entry_storage_bytes(&self, name: &OsStr) -> io::Result<Option<(u64, u64)>> {
        validate_normal_component(name)?;
        let stat = match rfs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(source) => return Err(source.into()),
        };
        let logical_bytes = u64::try_from(stat.st_size).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "capability entry has a negative logical size",
            )
        })?;
        let allocated_bytes = u64::try_from(stat.st_blocks)
            .unwrap_or(0)
            .saturating_mul(512);
        Ok(Some((logical_bytes, allocated_bytes)))
    }

    pub(super) fn entry_is_regular_file(&self, name: &OsStr) -> io::Result<Option<bool>> {
        validate_normal_component(name)?;
        match rfs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(Some(
                FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile,
            )),
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(source) => Err(source.into()),
        }
    }

    /// Makes a completed rename durable in both affected directories.
    pub(super) fn sync_rename(&self, destination: &Self) -> io::Result<()> {
        // Persist the destination name first. If the second fsync fails or the
        // process dies, recovery may observe both names, but it must never
        // observe a durable source removal without a durable destination.
        destination.sync()?;
        if self.identity == destination.identity {
            return Ok(());
        }
        #[cfg(test)]
        if INJECT_SYNC_RENAME_AFTER_DESTINATION_FOR.with(|configured| {
            let mut configured = configured.borrow_mut();
            if configured
                .as_ref()
                .is_some_and(|(source, destination_path)| {
                    source == &self.display_path && destination_path == &destination.display_path
                })
            {
                configured.take();
                true
            } else {
                false
            }
        }) {
            return Err(io::Error::other(
                "injected source-directory sync failure after destination sync",
            ));
        }
        self.sync()
    }

    pub(super) fn rename_to_noreplace(
        &self,
        source_name: &OsStr,
        destination: &Self,
        destination_name: &OsStr,
    ) -> io::Result<()> {
        self.rename_to_noreplace_uncommitted(source_name, destination, destination_name)?;
        self.sync_rename(destination)
    }

    /// Performs only an atomic no-replace namespace move.
    ///
    /// The caller must record that the move may have happened before invoking
    /// any other fallible operation, then call [`Self::sync_rename`].
    pub(super) fn rename_to_noreplace_uncommitted(
        &self,
        source_name: &OsStr,
        destination: &Self,
        destination_name: &OsStr,
    ) -> io::Result<()> {
        validate_normal_component(source_name)?;
        validate_normal_component(destination_name)?;
        self.validate_mutation_authority()?;
        destination.validate_mutation_authority()?;
        renameat_noreplace(&self.fd, source_name, &destination.fd, destination_name)
    }

    pub(super) fn write_json_atomically(
        &self,
        name: &OsStr,
        bytes: &[u8],
        temporary_prefix: &str,
    ) -> Result<(), AtomicWriteError> {
        self.write_json_atomically_with_observer(name, bytes, temporary_prefix, || Ok(()))
    }

    pub(super) fn write_json_atomically_with_observer(
        &self,
        name: &OsStr,
        bytes: &[u8],
        temporary_prefix: &str,
        after_rename: impl FnOnce() -> io::Result<()>,
    ) -> Result<(), AtomicWriteError> {
        self.write_json_atomically_with_observers(
            name,
            bytes,
            temporary_prefix,
            |_| Ok(()),
            || Ok(()),
            after_rename,
        )
    }

    pub(super) fn write_json_atomically_with_observers(
        &self,
        name: &OsStr,
        bytes: &[u8],
        temporary_prefix: &str,
        before_temp_open: impl FnOnce(&OsStr) -> io::Result<()>,
        after_temp_sync: impl FnOnce() -> io::Result<()>,
        after_rename: impl FnOnce() -> io::Result<()>,
    ) -> Result<(), AtomicWriteError> {
        validate_normal_component(name).map_err(AtomicWriteError::before_rename)?;
        self.validate_mutation_authority()
            .map_err(AtomicWriteError::before_rename)?;
        self.remove_generated_regular_files(temporary_prefix)
            .map_err(AtomicWriteError::before_rename)?;
        #[cfg(all(test, target_vendor = "apple"))]
        maybe_inject_inheritable_acl_before_create(&self.display_path, name)
            .map_err(AtomicWriteError::before_rename)?;
        let mut before_temp_open = Some(before_temp_open);
        let (temporary, fd) = {
            let mut opened = None;
            for _ in 0..MAX_UNIQUE_NAME_ATTEMPTS {
                let temporary = unique_name(temporary_prefix);
                if let Some(observer) = before_temp_open.take() {
                    observer(&temporary).map_err(AtomicWriteError::before_rename)?;
                }
                match create_file_exact(
                    &self.fd,
                    &temporary,
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    0o600,
                ) {
                    Ok(fd) => {
                        opened = Some((temporary, fd));
                        break;
                    }
                    Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(source) => return Err(AtomicWriteError::before_rename(source)),
                }
            }
            opened.ok_or_else(|| {
                AtomicWriteError::before_rename(unique_name_attempts_exhausted(
                    "atomic write temporary",
                ))
            })?
        };
        let stat = rfs::fstat(&fd)
            .map_err(io::Error::from)
            .map_err(AtomicWriteError::before_rename)?;
        let identity = identity_from_stat(&stat);
        if let Err(source) = validate_owned_single_link_regular(
            &stat,
            &temporary,
            self.identity.device,
            "atomic write temporary",
        ) {
            let source =
                self.cleanup_new_generated_file(&temporary, identity, temporary_prefix, source);
            return Err(AtomicWriteError::before_rename(source));
        }
        if let Err(source) = set_new_entry_mode(&fd, &temporary, 0o600) {
            let source =
                self.cleanup_new_generated_file(&temporary, identity, temporary_prefix, source);
            return Err(AtomicWriteError::before_rename(source));
        }
        let mut file = File::from(fd);
        if let Err(source) = file
            .write_all(bytes)
            .and_then(|()| sync_file_barrier(&file))
        {
            let source =
                self.cleanup_new_generated_file(&temporary, identity, temporary_prefix, source);
            return Err(AtomicWriteError::before_rename(source));
        }
        if let Err(source) = after_temp_sync() {
            let source =
                self.cleanup_new_generated_file(&temporary, identity, temporary_prefix, source);
            return Err(AtomicWriteError::before_rename(source));
        }
        if let Err(source) =
            rfs::renameat(&self.fd, &temporary, &self.fd, name).map_err(io::Error::from)
        {
            let source =
                self.cleanup_new_generated_file(&temporary, identity, temporary_prefix, source);
            return Err(AtomicWriteError::before_rename(source));
        }
        after_rename().map_err(AtomicWriteError::after_rename)?;
        #[cfg(test)]
        if let Some(failure) = INJECT_ATOMIC_AFTER_RENAME_FOR.with(|configured| {
            let mut configured = configured.borrow_mut();
            if configured
                .as_ref()
                .is_some_and(|(configured_name, _)| configured_name == name)
            {
                configured.take().map(|(_, failure)| failure)
            } else {
                None
            }
        }) {
            let source = match failure {
                InjectedAtomicAfterRenameFailure::Other => {
                    io::Error::other("injected capability directory sync failure")
                }
                InjectedAtomicAfterRenameFailure::Unsupported => io::Error::new(
                    io::ErrorKind::Unsupported,
                    "injected unsupported durability barrier",
                ),
                InjectedAtomicAfterRenameFailure::Io => io::Error::from_raw_os_error(5),
            };
            return Err(AtomicWriteError::after_rename(source));
        }
        self.sync().map_err(AtomicWriteError::after_rename)?;
        self.validate_mutation_authority()
            .map_err(AtomicWriteError::after_rename)?;
        let published = self
            .read_regular_file_exact(name, identity, bytes.len())
            .map_err(AtomicWriteError::after_rename)?;
        if published != bytes {
            return Err(AtomicWriteError::after_rename(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "atomic capability publication was not readable with the expected bytes: {}",
                    Path::new(name).display()
                ),
            )));
        }
        Ok(())
    }

    pub(super) fn remove_generated_regular_files(&self, prefix: &str) -> io::Result<usize> {
        self.validate_mutation_authority()?;
        let entries = self.entries()?;
        let deletion_prefix = generated_deletion_prefix(prefix);
        let mut removed = 0_usize;
        for entry in entries {
            let is_source = generated_name_matches(&entry, prefix);
            let is_tombstone = generated_name_matches(&entry, deletion_prefix.as_ref());
            if !is_source && !is_tombstone {
                continue;
            }
            let stat = rfs::statat(&self.fd, &entry, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(io::Error::from)?;
            validate_owned_single_link_regular(
                &stat,
                &entry,
                self.identity.device,
                "generated temporary entry",
            )?;
            let identity = identity_from_stat(&stat);
            if is_tombstone {
                self.remove_tombstone_verified(&entry, identity)?;
            } else {
                self.remove_generated_file_verified(&entry, identity, prefix)?;
            }
            removed = removed.saturating_add(1);
        }
        Ok(removed)
    }

    pub(super) fn read_file_limited(&self, name: &OsStr, max_bytes: usize) -> io::Result<Vec<u8>> {
        self.read_file_limited_with_metadata(name, max_bytes)
            .map(|read| read.bytes)
    }

    pub(super) fn open_read_file(&self, name: &OsStr) -> io::Result<File> {
        validate_normal_component(name)?;
        for _ in 0..MAX_AUTHENTICATED_READ_ATTEMPTS {
            if let Some(opened) = self.open_read_file_once(name)? {
                return Ok(opened.file);
            }
        }
        Err(authenticated_read_retry_limit_error(name))
    }

    fn open_read_file_once(&self, name: &OsStr) -> io::Result<Option<AuthenticatedReadFile>> {
        self.validate_mutation_authority()?;
        let expected = self
            .authenticated_read_entry_identity(name)?
            .ok_or_else(|| authenticated_read_not_found_error(name))?;
        #[cfg(test)]
        maybe_swap_preflight_to_fifo(&self.display_path, name)?;
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let fd = match rfs::openat(&self.fd, name, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(source) => return Err(source.into()),
        };
        #[cfg(test)]
        maybe_inject_authenticated_read_after_open(name);
        let descriptor = authenticate_read_descriptor(
            &fd,
            name,
            self.identity.device,
            "capability managed file",
        )?;
        let current = self.authenticated_read_entry_identity(name)?;
        if !descriptor.attached || descriptor.identity != expected || current != Some(expected) {
            return Ok(None);
        }
        Ok(Some(AuthenticatedReadFile {
            file: File::from(fd),
            identity: expected,
        }))
    }

    fn authenticated_read_entry_identity(&self, name: &OsStr) -> io::Result<Option<FileIdentity>> {
        let stat = match rfs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(source) => return Err(source.into()),
        };
        authenticated_read_entry_identity_from_stat(&stat, name, self.identity.device)
    }

    fn read_file_is_still_attached(
        &self,
        name: &OsStr,
        file: &File,
        expected: FileIdentity,
    ) -> io::Result<bool> {
        let descriptor = authenticate_read_descriptor(
            file,
            name,
            self.identity.device,
            "capability managed file",
        )?;
        let current = self.authenticated_read_entry_identity(name)?;
        Ok(descriptor.attached && descriptor.identity == expected && current == Some(expected))
    }

    pub(super) fn authenticate_regular_file(
        &self,
        name: &OsStr,
        expected: FileIdentity,
    ) -> io::Result<()> {
        let file = self.open_read_file(name)?;
        let stat = rfs::fstat(&file).map_err(io::Error::from)?;
        if identity_from_stat(&stat) != expected || self.entry_identity(name)? != Some(expected) {
            return Err(identity_changed(name));
        }
        Ok(())
    }

    pub(super) fn authenticate_regular_file_with_link_count(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        expected_links: u64,
    ) -> io::Result<()> {
        drop(self.open_regular_file_exact_link_count(name, expected, expected_links)?);
        Ok(())
    }

    pub(super) fn open_regular_file_exact_link_count(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        expected_links: u64,
    ) -> io::Result<File> {
        validate_normal_component(name)?;
        self.validate_mutation_authority()?;
        let preflight =
            rfs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
        validate_owned_regular_link_count(
            &preflight,
            name,
            self.identity.device,
            expected_links,
            "duplicated capability managed file",
        )?;
        if identity_from_stat(&preflight) != expected
            || ((preflight.st_mode as RawMode) & 0o022) != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "duplicated capability file identity changed or gained non-owner write authority: {}",
                    Path::new(name).display()
                ),
            ));
        }
        let fd = rfs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        validate_owned_regular_link_count(
            &stat,
            name,
            self.identity.device,
            expected_links,
            "duplicated capability managed file",
        )?;
        if identity_from_stat(&stat) != expected
            || ((stat.st_mode as RawMode) & 0o022) != 0
            || has_extended_acl(&fd)?
            || self.entry_identity(name)? != Some(expected)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "duplicated capability file failed exact identity or write-authority authentication: {}",
                    Path::new(name).display()
                ),
            ));
        }
        Ok(File::from(fd))
    }

    pub(super) fn read_regular_file_exact(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        max_bytes: usize,
    ) -> io::Result<Vec<u8>> {
        let file = self.open_regular_file_exact_link_count(name, expected, 1)?;
        let stat = rfs::fstat(&file).map_err(io::Error::from)?;
        let max_len = u64::try_from(max_bytes).unwrap_or(u64::MAX);
        let logical_bytes = u64::try_from(stat.st_size).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "capability file has a negative logical size",
            )
        })?;
        if logical_bytes > max_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("capability file exceeds {max_bytes} bytes"),
            ));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(usize::try_from(logical_bytes).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "capability file length does not fit in memory address space",
                )
            })?)
            .map_err(|source| {
                io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    format!("failed to reserve exact capability read storage: {source}"),
                )
            })?;
        let mut reader = file.take(max_len.saturating_add(1));
        reader.read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes || self.entry_identity(name)? != Some(expected) {
            return Err(identity_changed(name));
        }
        Ok(bytes)
    }

    pub(super) fn read_file_limited_with_metadata(
        &self,
        name: &OsStr,
        max_bytes: usize,
    ) -> io::Result<CapabilityFileRead> {
        validate_normal_component(name)?;
        for _ in 0..MAX_AUTHENTICATED_READ_ATTEMPTS {
            let Some(opened) = self.open_read_file_once(name)? else {
                continue;
            };
            let stat = rfs::fstat(&opened.file).map_err(io::Error::from)?;
            let max_len = u64::try_from(max_bytes).unwrap_or(u64::MAX);
            let logical_bytes = u64::try_from(stat.st_size).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "capability file has a negative logical size",
                )
            })?;
            if logical_bytes > max_len {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("capability file exceeds {max_bytes} bytes"),
                ));
            }
            let initial_capacity = usize::try_from(logical_bytes).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "capability file length does not fit in memory address space",
                )
            })?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(initial_capacity)
                .map_err(|source| {
                    io::Error::new(
                        io::ErrorKind::OutOfMemory,
                        format!("failed to reserve bounded capability file storage: {source}"),
                    )
                })?;
            let mut reader = opened.file.take(max_len.saturating_add(1));
            let mut chunk = [0_u8; 8 * 1024];
            loop {
                let read = reader.read(&mut chunk)?;
                if read == 0 {
                    break;
                }
                bytes.try_reserve(read).map_err(|source| {
                    io::Error::new(
                        io::ErrorKind::OutOfMemory,
                        format!("failed to grow bounded capability file storage: {source}"),
                    )
                })?;
                bytes.extend_from_slice(&chunk[..read]);
            }
            if bytes.len() > max_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("capability file exceeds {max_bytes} bytes"),
                ));
            }
            let file = reader.into_inner();
            if !self.read_file_is_still_attached(name, &file, opened.identity)? {
                continue;
            }
            let allocated_bytes = u64::try_from(stat.st_blocks)
                .unwrap_or(0)
                .saturating_mul(512);
            return Ok(CapabilityFileRead {
                bytes,
                logical_bytes,
                allocated_bytes,
                identity: opened.identity,
                mode: (stat.st_mode as RawMode) & 0o777,
            });
        }
        Err(authenticated_read_retry_limit_error(name))
    }

    pub(super) fn open_existing_lock_file(&self, name: &OsStr) -> io::Result<Option<File>> {
        validate_normal_component(name)?;
        self.validate_mutation_authority()?;
        let preflight = match rfs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(source) => return Err(source.into()),
        };
        validate_owned_single_link_regular(
            &preflight,
            name,
            self.identity.device,
            "capability lock",
        )?;
        let expected = identity_from_stat(&preflight);
        #[cfg(test)]
        maybe_swap_preflight_to_fifo(&self.display_path, name)?;
        let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let (fd, repaired_before_open) = match rfs::openat(&self.fd, name, flags, Mode::empty()) {
            Ok(fd) => (fd, false),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(rustix::io::Errno::ACCESS | rustix::io::Errno::PERM) => {
                ensure_regular_file_owner_access(
                    &self.fd,
                    name,
                    expected,
                    self.identity.device,
                    0o600,
                )?;
                match rfs::openat(&self.fd, name, flags, Mode::empty()) {
                    Ok(fd) => (fd, true),
                    Err(rustix::io::Errno::NOENT) => return Ok(None),
                    Err(source) => return Err(source.into()),
                }
            }
            Err(source) => return Err(source.into()),
        };
        let mut stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        validate_owned_single_link_regular(&stat, name, self.identity.device, "capability lock")?;
        if identity_from_stat(&stat) != expected {
            return Err(identity_changed(name));
        }
        let acl_repaired = has_extended_acl(&fd)?;
        if acl_repaired {
            strip_extended_acl(&fd)?;
        }
        if repaired_before_open || acl_repaired || ((stat.st_mode as RawMode) & 0o777) != 0o600 {
            if ((stat.st_mode as RawMode) & 0o777) != 0o600 {
                rfs::fchmod(&fd, Mode::from_bits_truncate(0o600)).map_err(io::Error::from)?;
            }
            sync_file_barrier(&fd)?;
            stat = rfs::fstat(&fd).map_err(io::Error::from)?;
            validate_owned_single_link_regular(
                &stat,
                name,
                self.identity.device,
                "capability lock",
            )?;
            if identity_from_stat(&stat) != expected || ((stat.st_mode as RawMode) & 0o777) != 0o600
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "capability lock mode correction did not establish mode 600: {}",
                        Path::new(name).display()
                    ),
                ));
            }
        }
        self.validate_mutation_authority()?;
        stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        validate_owned_single_link_regular(&stat, name, self.identity.device, "capability lock")?;
        if identity_from_stat(&stat) != expected
            || ((stat.st_mode as RawMode) & 0o777) != 0o600
            || has_extended_acl(&fd)?
            || self.entry_identity(name)? != Some(expected)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability lock final authentication failed: {}",
                    Path::new(name).display()
                ),
            ));
        }
        Ok(Some(File::from(fd)))
    }

    pub(super) fn open_lock_file(&self, name: &OsStr) -> io::Result<File> {
        validate_normal_component(name)?;
        self.validate_mutation_authority()?;
        if let Some(file) = self.open_existing_lock_file(name)? {
            return Ok(file);
        }
        let initializer_prefix = lock_initializer_prefix(name);
        for _ in 0..MAX_UNIQUE_NAME_ATTEMPTS {
            if let Some(file) = self.open_existing_lock_file(name)? {
                return Ok(file);
            }
            #[cfg(all(test, target_vendor = "apple"))]
            maybe_inject_inheritable_acl_before_create(&self.display_path, name)?;
            let initializer = unique_name(&initializer_prefix);
            match create_file_exact(
                &self.fd,
                &initializer,
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                0o600,
            ) {
                Ok(fd) => {
                    let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
                    let identity = identity_from_stat(&stat);
                    if let Err(source) = validate_owned_single_link_regular(
                        &stat,
                        &initializer,
                        self.identity.device,
                        "lock initializer",
                    ) {
                        return Err(self.cleanup_new_generated_file(
                            &initializer,
                            identity,
                            &initializer_prefix,
                            source,
                        ));
                    }
                    #[cfg(test)]
                    maybe_kill_lock_initializer(
                        name,
                        LockInitializerKillPoint::BeforeModeCorrection,
                    );
                    if let Err(source) = set_new_entry_mode(&fd, name, 0o600) {
                        return Err(self.cleanup_new_generated_file(
                            &initializer,
                            identity,
                            &initializer_prefix,
                            source,
                        ));
                    }
                    if let Err(source) = sync_file_barrier(&fd) {
                        return Err(self.cleanup_new_generated_file(
                            &initializer,
                            identity,
                            &initializer_prefix,
                            source,
                        ));
                    }
                    #[cfg(test)]
                    maybe_kill_lock_initializer(
                        name,
                        LockInitializerKillPoint::AfterModeCorrection,
                    );
                    match renameat_noreplace(&self.fd, &initializer, &self.fd, name) {
                        Ok(()) => {
                            self.sync()?;
                            self.validate_mutation_authority()?;
                            self.authenticate_regular_file_with_link_count(name, identity, 1)?;
                            let final_stat = rfs::fstat(&fd).map_err(io::Error::from)?;
                            if identity_from_stat(&final_stat) != identity
                                || ((final_stat.st_mode as RawMode) & 0o777) != 0o600
                                || has_extended_acl(&fd)?
                                || self.entry_identity(name)? != Some(identity)
                            {
                                return Err(identity_changed(name));
                            }
                            return Ok(File::from(fd));
                        }
                        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                            self.remove_file_initializer_if_present(
                                &initializer,
                                identity,
                                &initializer_prefix,
                            )?;
                            if let Some(file) = self.open_existing_lock_file(name)? {
                                return Ok(file);
                            }
                        }
                        Err(source) if source.kind() == io::ErrorKind::NotFound => {
                            self.remove_file_initializer_if_present(
                                &initializer,
                                identity,
                                &initializer_prefix,
                            )?;
                            continue;
                        }
                        Err(source) => {
                            return Err(self.cleanup_new_generated_file(
                                &initializer,
                                identity,
                                &initializer_prefix,
                                source,
                            ));
                        }
                    }
                }
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(source),
            }
        }
        Err(unique_name_attempts_exhausted("lock initializer"))
    }

    pub(super) fn open_append_file(&self, name: &OsStr) -> io::Result<File> {
        validate_normal_component(name)?;
        self.validate_mutation_authority()?;
        // Appending uses the event file's own advisory lock, but its first
        // publication has no inode to lock yet. A fresh descriptor for the
        // retained directory supplies a namespace-scoped initialization lock
        // without adding a persistent lock entry or trusting a path lookup.
        let initialization_lock = File::from(
            rfs::openat(&self.fd, OsStr::new("."), DIRECTORY_FLAGS, Mode::empty())
                .map_err(io::Error::from)?,
        );
        fs2::FileExt::lock_exclusive(&initialization_lock)?;
        #[cfg(test)]
        if std::env::var_os("PACKET28_TEST_EXIT_AFTER_APPEND_DIRECTORY_LOCK").as_deref()
            == Some(name)
        {
            std::process::exit(86);
        }
        let result = self.open_append_file_serialized(name);
        let unlock = fs2::FileExt::unlock(&initialization_lock);
        match (result, unlock) {
            (Ok(file), Ok(())) => Ok(file),
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        }
    }

    fn open_append_file_serialized(&self, name: &OsStr) -> io::Result<File> {
        let initializer_prefix = append_initializer_prefix(name);
        self.remove_generated_regular_files(&initializer_prefix)?;
        if let Some(file) = self.open_existing_append_file(name)? {
            return Ok(file);
        }
        for _ in 0..MAX_UNIQUE_NAME_ATTEMPTS {
            if let Some(file) = self.open_existing_append_file(name)? {
                return Ok(file);
            }
            #[cfg(all(test, target_vendor = "apple"))]
            maybe_inject_inheritable_acl_before_create(&self.display_path, name)?;
            let initializer = unique_name(&initializer_prefix);
            let fd = match create_file_exact(
                &self.fd,
                &initializer,
                OFlags::RDWR
                    | OFlags::APPEND
                    | OFlags::CREATE
                    | OFlags::EXCL
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC,
                0o600,
            ) {
                Ok(fd) => fd,
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(source),
            };
            let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
            let identity = identity_from_stat(&stat);
            if let Err(source) = validate_owned_single_link_regular(
                &stat,
                &initializer,
                self.identity.device,
                "append-file initializer",
            )
            .and_then(|()| set_new_entry_mode(&fd, &initializer, 0o600))
            .and_then(|()| sync_file_barrier(&fd))
            {
                return Err(self.cleanup_new_generated_file(
                    &initializer,
                    identity,
                    &initializer_prefix,
                    source,
                ));
            }
            match renameat_noreplace(&self.fd, &initializer, &self.fd, name) {
                Ok(()) => {
                    self.sync()?;
                    self.validate_mutation_authority()?;
                    self.authenticate_regular_file_with_link_count(name, identity, 1)?;
                    return Ok(File::from(fd));
                }
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                    self.remove_file_initializer_if_present(
                        &initializer,
                        identity,
                        &initializer_prefix,
                    )?;
                    if let Some(file) = self.open_existing_append_file(name)? {
                        return Ok(file);
                    }
                }
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    self.remove_file_initializer_if_present(
                        &initializer,
                        identity,
                        &initializer_prefix,
                    )?;
                }
                Err(source) => {
                    return Err(self.cleanup_new_generated_file(
                        &initializer,
                        identity,
                        &initializer_prefix,
                        source,
                    ));
                }
            }
        }
        Err(unique_name_attempts_exhausted("append-file initializer"))
    }

    fn open_existing_append_file(&self, name: &OsStr) -> io::Result<Option<File>> {
        let preflight = match rfs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(source) => return Err(source.into()),
        };
        validate_owned_single_link_regular(
            &preflight,
            name,
            self.identity.device,
            "capability append file",
        )?;
        if ((preflight.st_mode as RawMode) & 0o022) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability append file is group/other-writable and cannot be authenticated automatically: {}",
                    Path::new(name).display()
                ),
            ));
        }
        let expected = identity_from_stat(&preflight);
        let flags =
            OFlags::RDWR | OFlags::APPEND | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let (fd, repaired_before_open) = match rfs::openat(&self.fd, name, flags, Mode::empty()) {
            Ok(fd) => (fd, false),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(rustix::io::Errno::ACCESS | rustix::io::Errno::PERM) => {
                ensure_regular_file_owner_access(
                    &self.fd,
                    name,
                    expected,
                    self.identity.device,
                    0o600,
                )?;
                match rfs::openat(&self.fd, name, flags, Mode::empty()) {
                    Ok(fd) => (fd, true),
                    Err(rustix::io::Errno::NOENT) => return Ok(None),
                    Err(source) => return Err(source.into()),
                }
            }
            Err(source) => return Err(source.into()),
        };
        let mut stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        validate_owned_single_link_regular(
            &stat,
            name,
            self.identity.device,
            "capability append file",
        )?;
        if identity_from_stat(&stat) != expected {
            return Err(identity_changed(name));
        }
        if has_extended_acl(&fd)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability append file has an extended ACL and cannot be authenticated automatically: {}",
                    Path::new(name).display()
                ),
            ));
        }
        if repaired_before_open || ((stat.st_mode as RawMode) & 0o777) != 0o600 {
            if ((stat.st_mode as RawMode) & 0o777) != 0o600 {
                rfs::fchmod(&fd, Mode::from_bits_truncate(0o600)).map_err(io::Error::from)?;
            }
            sync_file_barrier(&fd)?;
            stat = rfs::fstat(&fd).map_err(io::Error::from)?;
            validate_owned_single_link_regular(
                &stat,
                name,
                self.identity.device,
                "capability append file",
            )?;
            if identity_from_stat(&stat) != expected || ((stat.st_mode as RawMode) & 0o777) != 0o600
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "capability append-file mode correction did not establish mode 600: {}",
                        Path::new(name).display()
                    ),
                ));
            }
        }
        Ok(Some(File::from(fd)))
    }

    /// Verifies atomic no-replace rename support without moving managed data.
    pub(super) fn probe_noreplace_rename(&self) -> io::Result<()> {
        self.validate_mutation_authority()?;
        #[cfg(test)]
        if INJECT_NOREPLACE_UNSUPPORTED.with(|configured| configured.replace(false)) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "injected atomic no-replace rename unavailability",
            ));
        }
        let (source, identity) = {
            let mut opened = None;
            for _ in 0..MAX_UNIQUE_NAME_ATTEMPTS {
                let source = unique_name(NOREPLACE_PROBE_SOURCE_PREFIX);
                match create_file_exact(
                    &self.fd,
                    &source,
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    0o600,
                ) {
                    Ok(fd) => {
                        let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
                        let identity = identity_from_stat(&stat);
                        if let Err(error) = validate_owned_single_link_regular(
                            &stat,
                            &source,
                            self.identity.device,
                            "rename probe source",
                        )
                        .and_then(|()| set_new_entry_mode(&fd, &source, 0o600))
                        .and_then(|()| sync_file_barrier(&fd))
                        {
                            return Err(self.cleanup_new_generated_file(
                                &source,
                                identity,
                                NOREPLACE_PROBE_SOURCE_PREFIX,
                                error,
                            ));
                        }
                        opened = Some((source, identity));
                        break;
                    }
                    Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(source) => return Err(source),
                }
            }
            opened.ok_or_else(|| unique_name_attempts_exhausted("rename probe source"))?
        };
        for _ in 0..MAX_UNIQUE_NAME_ATTEMPTS {
            let destination = unique_name(NOREPLACE_PROBE_DESTINATION_PREFIX);
            match self.rename_to_noreplace_uncommitted(&source, self, &destination) {
                Ok(()) => {
                    if let Err(source) = self.sync() {
                        return Err(self.cleanup_new_generated_file(
                            &destination,
                            identity,
                            NOREPLACE_PROBE_DESTINATION_PREFIX,
                            source,
                        ));
                    }
                    return self.remove_generated_file_verified(
                        &destination,
                        identity,
                        NOREPLACE_PROBE_DESTINATION_PREFIX,
                    );
                }
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(self.cleanup_new_generated_file(
                        &source,
                        identity,
                        NOREPLACE_PROBE_SOURCE_PREFIX,
                        error,
                    ));
                }
            }
        }
        Err(self.cleanup_new_generated_file(
            &source,
            identity,
            NOREPLACE_PROBE_SOURCE_PREFIX,
            unique_name_attempts_exhausted("rename probe destination"),
        ))
    }

    pub(super) fn has_entries(&self) -> io::Result<bool> {
        let mut directory = Dir::read_from(&self.fd).map_err(io::Error::from)?;
        while let Some(entry) = directory.read() {
            let entry = entry.map_err(io::Error::from)?;
            let bytes = entry.file_name().to_bytes();
            if bytes != b"." && bytes != b".." {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn entries_page(&self, max_entries: usize) -> io::Result<(Vec<OsString>, bool)> {
        let mut directory = Dir::read_from(&self.fd).map_err(io::Error::from)?;
        let mut entries = Vec::new();
        while let Some(entry) = directory.read() {
            let entry = entry.map_err(io::Error::from)?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if entries.len() == max_entries {
                entries.sort();
                return Ok((entries, true));
            }
            entries.try_reserve(1).map_err(|source| {
                io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    format!("failed to reserve capability entry page storage: {source}"),
                )
            })?;
            let mut name = Vec::new();
            name.try_reserve_exact(bytes.len()).map_err(|source| {
                io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    format!("failed to reserve capability entry name storage: {source}"),
                )
            })?;
            name.extend_from_slice(bytes);
            entries.push(OsString::from_vec(name));
        }
        entries.sort();
        Ok((entries, false))
    }

    pub(super) fn entries_bounded(&self, max_entries: usize) -> io::Result<Vec<OsString>> {
        let mut directory = Dir::read_from(&self.fd).map_err(io::Error::from)?;
        let mut entries = Vec::new();
        while let Some(entry) = directory.read() {
            let entry = entry.map_err(io::Error::from)?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if entries.len() >= max_entries {
                return Err(capability_entry_limit_error(max_entries));
            }
            entries.try_reserve(1).map_err(|source| {
                io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    format!("failed to reserve bounded capability entry storage: {source}"),
                )
            })?;
            let mut name = Vec::new();
            name.try_reserve_exact(bytes.len()).map_err(|source| {
                io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    format!("failed to reserve capability entry name storage: {source}"),
                )
            })?;
            name.extend_from_slice(bytes);
            entries.push(OsString::from_vec(name));
        }
        entries.sort();
        Ok(entries)
    }

    pub(super) fn entries(&self) -> io::Result<Vec<OsString>> {
        self.entries_bounded(MAX_CAPABILITY_DIRECTORY_ENTRIES)
    }

    pub(super) fn entry_logical_bytes_verified(
        &self,
        name: &OsStr,
        expected: FileIdentity,
    ) -> io::Result<u64> {
        self.entry_logical_bytes_verified_with_limits(name, expected, TraversalLimits::DEFAULT)
    }

    fn entry_logical_bytes_verified_with_limits(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        limits: TraversalLimits,
    ) -> io::Result<u64> {
        let mut budget = TraversalBudget::new(limits);
        self.entry_logical_bytes_verified_inner(name, expected, 0, &mut budget)
    }

    fn entry_logical_bytes_verified_inner(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        depth: usize,
        budget: &mut TraversalBudget,
    ) -> io::Result<u64> {
        budget.consume(depth)?;
        let stat =
            rfs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
        if identity_from_stat(&stat) != expected {
            return Err(identity_changed(name));
        }
        let file_type = FileType::from_raw_mode(stat.st_mode);
        if file_type == FileType::RegularFile {
            return u64::try_from(stat.st_size).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "capability file has a negative logical size",
                )
            });
        }
        if file_type != FileType::Directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "capability entry is not a regular file or directory",
            ));
        }
        let child = self.open_dir(name)?;
        if child.identity != expected {
            return Err(identity_changed(name));
        }
        let mut logical_bytes = 0_u64;
        for child_name in child.entries_bounded(budget.remaining_entries())? {
            let child_identity = child
                .entry_identity(&child_name)?
                .ok_or_else(|| identity_changed(&child_name))?;
            logical_bytes =
                logical_bytes.saturating_add(child.entry_logical_bytes_verified_inner(
                    &child_name,
                    child_identity,
                    depth.saturating_add(1),
                    budget,
                )?);
        }
        Ok(logical_bytes)
    }

    /// Removes `name` only if it still resolves to `expected`.
    ///
    /// The entry is atomically moved to a fresh tombstone name before any
    /// unlink. If an attacker or concurrent writer substituted the entry, the
    /// mismatched tombstone is deliberately left in place.
    pub(super) fn remove_tree_entry_verified(
        &self,
        name: &OsStr,
        expected: FileIdentity,
    ) -> io::Result<()> {
        let tombstone = self.tombstone_entry_verified(name, expected, DELETION_TEMP_PREFIX)?;
        self.remove_tombstone_verified(&tombstone, expected)
    }

    pub(super) fn tombstone_entry_verified(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        prefix: &str,
    ) -> io::Result<OsString> {
        for _ in 0..MAX_UNIQUE_NAME_ATTEMPTS {
            let tombstone = unique_name(prefix);
            match self.tombstone_entry_to_verified(name, expected, &tombstone) {
                Ok(()) => return Ok(tombstone),
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(source),
            }
        }
        Err(unique_name_attempts_exhausted("deletion tombstone"))
    }

    pub(super) fn tombstone_entry_to_verified(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        tombstone: &OsStr,
    ) -> io::Result<()> {
        validate_normal_component(name)?;
        validate_normal_component(tombstone)?;
        let current = self
            .entry_identity(name)?
            .ok_or_else(|| identity_changed(name))?;
        if current != expected {
            return Err(identity_changed(name));
        }
        self.rename_to_noreplace(name, self, tombstone)?;
        let moved = self
            .entry_identity(tombstone)?
            .ok_or_else(|| identity_changed(tombstone))?;
        if moved != expected {
            return Err(identity_changed(tombstone));
        }
        Ok(())
    }

    pub(super) fn tombstone_dir_entry_verified(
        &self,
        name: &OsStr,
        expected: FileIdentity,
    ) -> io::Result<(OsString, Self)> {
        let tombstone = self.tombstone_entry_verified(name, expected, ".deleting-group")?;
        let directory = self.open_dir(&tombstone)?;
        if directory.identity != expected {
            return Err(identity_changed(&tombstone));
        }
        Ok((tombstone, directory))
    }

    pub(super) fn remove_empty_dir_verified(
        &self,
        name: &OsStr,
        expected: FileIdentity,
    ) -> io::Result<()> {
        self.remove_empty_dir_verified_with_observer(name, expected, || Ok(()))
    }

    pub(super) fn remove_empty_dir_verified_with_observer(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        before_final_check: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        validate_normal_component(name)?;
        self.validate_mutation_authority()?;
        before_final_check()?;
        if self.entry_identity(name)? != Some(expected) {
            return Err(identity_changed(name));
        }
        rfs::unlinkat(&self.fd, name, AtFlags::REMOVEDIR).map_err(io::Error::from)?;
        self.sync()
    }

    pub(super) fn remove_tombstone_verified(
        &self,
        name: &OsStr,
        expected: FileIdentity,
    ) -> io::Result<()> {
        match self.remove_tombstone_verified_batch(name, expected)? {
            RemovalProgress::Complete => Ok(()),
            RemovalProgress::More => Err(capability_entry_limit_error(
                MAX_CAPABILITY_RECURSIVE_ENTRIES,
            )),
        }
    }

    pub(super) fn remove_tombstone_verified_batch(
        &self,
        name: &OsStr,
        expected: FileIdentity,
    ) -> io::Result<RemovalProgress> {
        self.remove_tombstone_verified_batch_with_observer(
            name,
            expected,
            MAX_CAPABILITY_RECURSIVE_ENTRIES,
            || Ok(()),
        )
    }

    #[cfg(test)]
    pub(super) fn remove_tombstone_verified_with_observer(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        before_final_check: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        match self.remove_tombstone_verified_batch_with_observer(
            name,
            expected,
            MAX_CAPABILITY_RECURSIVE_ENTRIES,
            before_final_check,
        )? {
            RemovalProgress::Complete => Ok(()),
            RemovalProgress::More => Err(capability_entry_limit_error(
                MAX_CAPABILITY_RECURSIVE_ENTRIES,
            )),
        }
    }

    #[cfg(test)]
    pub(super) fn remove_tombstone_verified_batch_with_limit(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        max_entries: usize,
    ) -> io::Result<RemovalProgress> {
        self.remove_tombstone_verified_batch_with_observer(name, expected, max_entries, || Ok(()))
    }

    fn remove_tombstone_verified_batch_with_observer(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        max_entries: usize,
        before_final_check: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<RemovalProgress> {
        if max_entries == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "capability deletion batch must allow at least one entry",
            ));
        }
        let mut before_final_check = Some(before_final_check);
        validate_normal_component(name)?;
        let stat =
            rfs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
        if identity_from_stat(&stat) != expected {
            return Err(identity_changed(name));
        }
        let file_type = FileType::from_raw_mode(stat.st_mode);
        if file_type != FileType::Directory {
            run_final_deletion_observer(&mut before_final_check)?;
            self.unlink_tombstone_verified(name, expected, file_type)?;
            return Ok(RemovalProgress::Complete);
        }

        let tombstone = self.open_dir(name)?;
        if tombstone.identity != expected {
            return Err(identity_changed(name));
        }
        for _ in 0..max_entries {
            let (entries, _) = tombstone.entries_page(1)?;
            let Some(entry_name) = entries.into_iter().next() else {
                run_final_deletion_observer(&mut before_final_check)?;
                self.unlink_tombstone_verified(name, expected, file_type)?;
                return Ok(RemovalProgress::Complete);
            };
            let entry_stat = rfs::statat(&tombstone.fd, &entry_name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(io::Error::from)?;
            let entry_identity = identity_from_stat(&entry_stat);
            if FileType::from_raw_mode(entry_stat.st_mode) == FileType::Directory {
                let child = tombstone.open_dir(&entry_name)?;
                if child.identity != entry_identity {
                    return Err(identity_changed(&entry_name));
                }
                let (child_entries, _) = child.entries_page(1)?;
                if let Some(child_name) = child_entries.into_iter().next() {
                    let child_identity = child
                        .entry_identity(&child_name)?
                        .ok_or_else(|| identity_changed(&child_name))?;
                    child.move_entry_to_generated_verified(
                        &child_name,
                        child_identity,
                        &tombstone,
                        DELETION_TEMP_PREFIX,
                    )?;
                } else {
                    tombstone.remove_empty_dir_verified(&entry_name, entry_identity)?;
                }
            } else {
                let entry_tombstone = tombstone.tombstone_entry_verified(
                    &entry_name,
                    entry_identity,
                    DELETION_TEMP_PREFIX,
                )?;
                tombstone.unlink_tombstone_verified(
                    &entry_tombstone,
                    entry_identity,
                    FileType::from_raw_mode(entry_stat.st_mode),
                )?;
            }
        }
        if tombstone.has_entries()? {
            Ok(RemovalProgress::More)
        } else {
            run_final_deletion_observer(&mut before_final_check)?;
            self.unlink_tombstone_verified(name, expected, file_type)?;
            Ok(RemovalProgress::Complete)
        }
    }

    fn move_entry_to_generated_verified(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        destination: &Self,
        prefix: &str,
    ) -> io::Result<OsString> {
        validate_normal_component(name)?;
        if self.entry_identity(name)? != Some(expected) {
            return Err(identity_changed(name));
        }
        for _ in 0..MAX_UNIQUE_NAME_ATTEMPTS {
            let destination_name = unique_name(prefix);
            match self.rename_to_noreplace(name, destination, &destination_name) {
                Ok(()) => {
                    if destination.entry_identity(&destination_name)? != Some(expected) {
                        return Err(identity_changed(&destination_name));
                    }
                    return Ok(destination_name);
                }
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(source),
            }
        }
        Err(unique_name_attempts_exhausted(
            "flattened deletion tombstone",
        ))
    }

    fn unlink_tombstone_verified(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        file_type: FileType,
    ) -> io::Result<()> {
        self.validate_mutation_authority()?;
        let final_identity = self
            .entry_identity(name)?
            .ok_or_else(|| identity_changed(name))?;
        if final_identity != expected {
            return Err(identity_changed(name));
        }
        if file_type == FileType::Directory {
            rfs::unlinkat(&self.fd, name, AtFlags::REMOVEDIR).map_err(io::Error::from)?;
        } else {
            rfs::unlinkat(&self.fd, name, AtFlags::empty()).map_err(io::Error::from)?;
        }
        self.sync()
    }

    fn open_dir_if_exists(&self, name: &OsStr, mode: RawMode) -> io::Result<Option<Self>> {
        validate_normal_component(name)?;
        let preflight = match rfs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(source) => return Err(source.into()),
        };
        validate_owned_directory(
            &preflight,
            name,
            self.identity.device,
            "capability directory",
        )?;
        let mut authenticity_lost = ((preflight.st_mode as RawMode) & 0o022) != 0;
        let expected = identity_from_stat(&preflight);
        let fd = match rfs::openat(&self.fd, name, DIRECTORY_FLAGS, Mode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(rustix::io::Errno::ACCESS | rustix::io::Errno::PERM) => {
                ensure_directory_owner_access(
                    &self.fd,
                    name,
                    expected,
                    self.identity.device,
                    false,
                )?;
                match rfs::openat(&self.fd, name, DIRECTORY_FLAGS, Mode::empty()) {
                    Ok(fd) => fd,
                    Err(rustix::io::Errno::NOENT) => return Ok(None),
                    Err(source) => return Err(source.into()),
                }
            }
            Err(source) => return Err(source.into()),
        };
        let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        validate_owned_directory(&stat, name, self.identity.device, "capability directory")?;
        if identity_from_stat(&stat) != expected {
            return Err(identity_changed(name));
        }
        authenticity_lost |= ((stat.st_mode as RawMode) & 0o022) != 0;
        if has_extended_acl(&fd)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability authority directory has an extended ACL and cannot be authenticated automatically: {}",
                    Path::new(name).display()
                ),
            ));
        }
        if authenticity_lost {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability authority directory is group/other-writable and cannot be authenticated automatically: {}",
                    Path::new(name).display()
                ),
            ));
        }
        let actual_mode = (stat.st_mode as RawMode) & 0o777;
        let expected_mode = mode & 0o777;
        let corrected = if actual_mode == expected_mode {
            stat
        } else {
            rfs::fchmod(&fd, Mode::from_bits_truncate(mode)).map_err(io::Error::from)?;
            sync_directory_barrier(&fd)?;
            rfs::fstat(&fd).map_err(io::Error::from)?
        };
        validate_owned_directory(
            &corrected,
            name,
            self.identity.device,
            "capability directory",
        )?;
        if identity_from_stat(&corrected) != expected
            || ((corrected.st_mode as RawMode) & 0o777) != expected_mode
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability directory mode correction did not establish mode {:o}: {}",
                    expected_mode,
                    Path::new(name).display()
                ),
            ));
        }
        let directory =
            Self::from_child_fd(fd, self.display_path.join(name), self.identity.device)?;
        if directory.identity != expected {
            return Err(identity_changed(name));
        }
        Ok(Some(directory))
    }

    fn publish_initialized_directory(
        &self,
        name: &OsStr,
        mode: RawMode,
        ensure: bool,
    ) -> io::Result<Self> {
        let initializer_prefix = directory_initializer_prefix(name);
        #[cfg(all(test, target_vendor = "apple"))]
        maybe_inject_inheritable_acl_before_create(&self.display_path, name)?;
        for _ in 0..MAX_UNIQUE_NAME_ATTEMPTS {
            if ensure {
                if let Some(directory) = self.open_dir_if_exists(name, mode)? {
                    return Ok(directory);
                }
            }
            let initializer = unique_name(&initializer_prefix);
            match create_directory_exact(&self.fd, &initializer, 0o700) {
                Ok(()) => {}
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(source),
            }
            let (directory, expected) =
                match self.finish_directory_initializer(&initializer, name, mode) {
                    Ok(initialized) => initialized,
                    Err(source) if ensure && source.kind() == io::ErrorKind::NotFound => {
                        continue;
                    }
                    Err(source) => return Err(source),
                };
            match renameat_noreplace(&self.fd, &initializer, &self.fd, name) {
                Ok(()) => {
                    self.sync()?;
                    self.validate_mutation_authority()?;
                    let published = self.open_dir(name)?;
                    if published.identity != expected
                        || directory.identity != expected
                        || self.entry_identity(name)? != Some(expected)
                        || has_extended_acl(&published.fd)?
                    {
                        return Err(identity_changed(name));
                    }
                    published.validate_private(mode)?;
                    return Ok(published);
                }
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                    self.remove_initializer_if_present(&initializer, expected)?;
                    if ensure {
                        if let Some(directory) = self.open_dir_if_exists(name, mode)? {
                            return Ok(directory);
                        }
                        continue;
                    }
                    return Err(source);
                }
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    self.remove_initializer_if_present(&initializer, expected)?;
                    continue;
                }
                Err(source) => {
                    return Err(self.cleanup_new_directory(&initializer, Some(expected), source));
                }
            }
        }
        Err(unique_name_attempts_exhausted("directory initializer"))
    }

    fn finish_directory_initializer(
        &self,
        initializer: &OsStr,
        final_name: &OsStr,
        mode: RawMode,
    ) -> io::Result<(Self, FileIdentity)> {
        let expected = match self.entry_identity(initializer) {
            Ok(Some(identity)) => identity,
            Ok(None) => {
                let source = io::Error::new(
                    io::ErrorKind::NotFound,
                    "capability directory initializer disappeared before it could be opened",
                );
                return Err(self.cleanup_new_directory(initializer, None, source));
            }
            Err(source) => {
                return Err(self.cleanup_new_directory(initializer, None, source));
            }
        };
        #[cfg(test)]
        maybe_kill_directory_initializer(
            final_name,
            DirectoryInitializerKillPoint::BeforeModeCorrection,
        );
        let result = (|| {
            ensure_directory_owner_access(
                &self.fd,
                initializer,
                expected,
                self.identity.device,
                true,
            )?;
            let initializer_fd = self.open_new_directory_initializer(initializer, expected)?;
            set_new_entry_mode(&initializer_fd, final_name, mode)?;
            let directory = Self::from_child_fd(
                initializer_fd,
                self.display_path.join(initializer),
                self.identity.device,
            )?;
            if directory.identity != expected {
                return Err(identity_changed(initializer));
            }
            if directory.has_entries()? {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "new capability directory initializer was populated before publication",
                ));
            }
            directory.validate_private(mode)?;
            directory.sync()?;
            #[cfg(test)]
            if INJECT_DIRECTORY_CREATE_SYNC_FAILURE_FOR.with(|configured| {
                let mut configured = configured.borrow_mut();
                if configured.as_deref() == Some(final_name) {
                    configured.take();
                    true
                } else {
                    false
                }
            }) {
                return Err(io::Error::other(
                    "injected capability directory creation sync failure",
                ));
            }
            #[cfg(test)]
            maybe_kill_directory_initializer(
                final_name,
                DirectoryInitializerKillPoint::AfterModeCorrection,
            );
            Ok((directory, expected))
        })();
        result.map_err(|source| self.cleanup_new_directory(initializer, Some(expected), source))
    }

    fn remove_initializer_if_present(
        &self,
        initializer: &OsStr,
        expected: FileIdentity,
    ) -> io::Result<()> {
        match self.entry_identity(initializer)? {
            None => Ok(()),
            Some(actual) if actual == expected => {
                self.remove_empty_dir_verified(initializer, expected)
            }
            Some(_) => Err(identity_changed(initializer)),
        }
    }

    fn open_new_directory_initializer(
        &self,
        name: &OsStr,
        expected: FileIdentity,
    ) -> io::Result<OwnedFd> {
        validate_normal_component(name)?;
        self.validate_mutation_authority()?;
        let fd =
            rfs::openat(&self.fd, name, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
        let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        validate_owned_directory(
            &stat,
            name,
            self.identity.device,
            "capability directory initializer",
        )?;
        if identity_from_stat(&stat) != expected {
            return Err(identity_changed(name));
        }

        // A fresh high-entropy initializer may inherit an ACL from a
        // protective workspace ACL. It is not yet published at `final_name`,
        // and the retained parent itself grants no namespace authority, so
        // keep the exact raw descriptor only long enough for
        // `set_new_entry_mode` to strip that inherited ACL and establish the
        // requested mode. It becomes a strict `CapabilityDir` only after that
        // normalization succeeds.
        Ok(fd)
    }

    fn remove_file_initializer_if_present(
        &self,
        initializer: &OsStr,
        expected: FileIdentity,
        prefix: &str,
    ) -> io::Result<()> {
        match self.entry_identity(initializer)? {
            None => Ok(()),
            Some(actual) if actual == expected => {
                self.remove_generated_file_verified(initializer, expected, prefix)
            }
            Some(_) => Err(identity_changed(initializer)),
        }
    }

    fn remove_generated_file_verified(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        prefix: &str,
    ) -> io::Result<()> {
        let deletion_prefix = generated_deletion_prefix(prefix);
        let tombstone = self.tombstone_entry_verified(name, expected, deletion_prefix.as_ref())?;
        self.remove_tombstone_verified(&tombstone, expected)
    }

    fn cleanup_new_directory(
        &self,
        name: &OsStr,
        expected: Option<FileIdentity>,
        source: io::Error,
    ) -> io::Error {
        let remove_result = match expected {
            Some(expected) => self.remove_empty_dir_verified(name, expected),
            None => rfs::unlinkat(&self.fd, name, AtFlags::REMOVEDIR).map_err(io::Error::from),
        };
        error_after_cleanup(source, remove_result, self.sync())
    }

    fn cleanup_new_generated_file(
        &self,
        name: &OsStr,
        expected: FileIdentity,
        prefix: &str,
        source: io::Error,
    ) -> io::Error {
        let remove_result = self.remove_generated_file_verified(name, expected, prefix);
        error_after_cleanup(source, remove_result, self.sync())
    }

    fn validate_mutation_authority(&self) -> io::Result<()> {
        let stat = rfs::fstat(&self.fd).map_err(io::Error::from)?;
        validate_owned_directory(
            &stat,
            self.display_path.as_os_str(),
            self.identity.device,
            "capability mutation parent",
        )?;
        if identity_from_stat(&stat) != self.identity {
            return Err(identity_changed(self.display_path.as_os_str()));
        }
        if ((stat.st_mode as RawMode) & 0o022) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability mutation parent is group/other-writable: {}",
                    self.display_path.display()
                ),
            ));
        }
        let acl_has_authority = match self.acl_policy {
            AclPolicy::StrictEmpty => has_extended_acl(&self.fd)?,
            AclPolicy::NamespaceAuthorityOnly => has_namespace_authority_acl(&self.fd)?,
        };
        if acl_has_authority {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability mutation parent has extended ACL namespace authority: {}",
                    self.display_path.display()
                ),
            ));
        }
        Ok(())
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        sync_directory_barrier(&self.fd)
    }

    pub(super) fn validate_private(&self, expected_mode: RawMode) -> io::Result<()> {
        let stat = rfs::fstat(&self.fd).map_err(io::Error::from)?;
        let actual_mode = (stat.st_mode as u32) & 0o777;
        let expected_mode = (expected_mode as u32) & 0o777;
        if actual_mode != expected_mode || stat.st_uid as u32 != rustix::process::geteuid().as_raw()
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "private capability directory has mode {actual_mode:o} and uid {}; expected mode {expected_mode:o} and uid {}",
                    stat.st_uid,
                    rustix::process::geteuid().as_raw()
                ),
            ));
        }
        Ok(())
    }

    fn from_fd(fd: OwnedFd, display_path: PathBuf) -> io::Result<Self> {
        Self::from_fd_with_acl_policy(fd, display_path, AclPolicy::StrictEmpty)
    }

    fn from_child_fd(fd: OwnedFd, display_path: PathBuf, expected_device: u64) -> io::Result<Self> {
        let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        validate_owned_directory(
            &stat,
            display_path.as_os_str(),
            expected_device,
            "capability child",
        )?;
        if ((stat.st_mode as RawMode) & 0o022) != 0 || has_extended_acl(&fd)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "capability child has non-owner write authority or an extended ACL: {}",
                    display_path.display()
                ),
            ));
        }
        Self::from_fd(fd, display_path)
    }

    fn from_fd_with_acl_policy(
        fd: OwnedFd,
        display_path: PathBuf,
        acl_policy: AclPolicy,
    ) -> io::Result<Self> {
        let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "capability path is not a directory",
            ));
        }
        Ok(Self {
            fd,
            display_path,
            identity: identity_from_stat(&stat),
            acl_policy,
        })
    }
}

fn run_final_deletion_observer(
    observer: &mut Option<impl FnOnce() -> io::Result<()>>,
) -> io::Result<()> {
    let observer = observer
        .take()
        .ok_or_else(|| io::Error::other("final deletion observer was already consumed"))?;
    observer()
}

#[cfg(target_vendor = "apple")]
fn ensure_directory_owner_access(
    parent: &OwnedFd,
    name: &OsStr,
    expected: FileIdentity,
    expected_device: u64,
    allow_new_initializer_repair: bool,
) -> io::Result<()> {
    let stat = rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    validate_owned_directory(&stat, name, expected_device, "capability directory")?;
    if identity_from_stat(&stat) != expected {
        return Err(identity_changed(name));
    }
    let anchored_path = apple_anchored_entry_path(parent, name);
    let acl_present = apple_acl::path_has_any_extended_acl(&anchored_path)?;
    if rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .map(|stat| identity_from_stat(&stat))
        .map_err(io::Error::from)?
        != expected
    {
        return Err(identity_changed(name));
    }
    if !allow_new_initializer_repair || acl_present {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "mode-inaccessible capability directory requires manual repair: {}",
                Path::new(name).display()
            ),
        ));
    }
    // This repair is restricted to a freshly created, high-entropy
    // initializer under the retained parent. Existing authority never reaches
    // this branch. Identity is checked immediately before and after the
    // no-follow chmod, then the caller acquires and authenticates a descriptor.
    rfs::chmodat(parent, name, Mode::RWXU, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    let corrected =
        rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    if identity_from_stat(&corrected) != expected
        || ((corrected.st_mode as RawMode) & 0o777) != 0o700
        || apple_acl::path_has_any_extended_acl(&anchored_path)?
    {
        return Err(identity_changed(name));
    }
    Ok(())
}

#[cfg(target_vendor = "apple")]
fn apple_anchored_entry_path(parent: &OwnedFd, name: &OsStr) -> PathBuf {
    PathBuf::from(format!("/dev/fd/{}", parent.as_raw_fd())).join(name)
}

#[cfg(not(target_vendor = "apple"))]
fn ensure_directory_owner_access(
    parent: &OwnedFd,
    name: &OsStr,
    expected: FileIdentity,
    expected_device: u64,
    _allow_acl_normalization: bool,
) -> io::Result<()> {
    let fd = open_new_directory_permission_handle(parent, name)?;
    let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
    validate_owned_directory(&stat, name, expected_device, "capability directory")?;
    if identity_from_stat(&stat) != expected {
        return Err(identity_changed(name));
    }
    set_new_directory_owner_access(&fd)
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
fn open_new_directory_permission_handle(parent: &OwnedFd, name: &OsStr) -> io::Result<OwnedFd> {
    rfs::openat(
        parent,
        name,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd"
)))]
fn open_new_directory_permission_handle(parent: &OwnedFd, name: &OsStr) -> io::Result<OwnedFd> {
    rfs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
fn set_new_directory_owner_access(fd: &OwnedFd) -> io::Result<()> {
    rfs::chmodat(fd, OsStr::new("."), Mode::RWXU, AtFlags::empty()).map_err(io::Error::from)
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd"
)))]
fn set_new_directory_owner_access(fd: &OwnedFd) -> io::Result<()> {
    rfs::fchmod(fd, Mode::RWXU).map_err(io::Error::from)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn ensure_regular_file_owner_access(
    parent: &OwnedFd,
    name: &OsStr,
    expected: FileIdentity,
    expected_device: u64,
    _mode: RawMode,
) -> io::Result<()> {
    let fd = rfs::openat(
        parent,
        name,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
    validate_owned_single_link_regular(&stat, name, expected_device, "capability lock")?;
    if identity_from_stat(&stat) != expected {
        return Err(identity_changed(name));
    }
    let proc_path = PathBuf::from(format!(
        "/proc/self/fd/{}",
        std::os::fd::AsRawFd::as_raw_fd(&fd)
    ));
    let acl_present = posix_acl::path_has_extended_acl(&proc_path)?;
    let retained = rfs::fstat(&fd).map_err(io::Error::from)?;
    let attached = rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    validate_owned_single_link_regular(&retained, name, expected_device, "capability lock")?;
    validate_owned_single_link_regular(&attached, name, expected_device, "capability lock")?;
    if identity_from_stat(&retained) != expected || identity_from_stat(&attached) != expected {
        return Err(identity_changed(name));
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        if acl_present {
            format!(
                "mode-inaccessible capability file has an ACL and requires manual repair: {}",
                Path::new(name).display()
            )
        } else {
            format!(
                "mode-inaccessible capability file requires manual repair: {}",
                Path::new(name).display()
            )
        },
    ))
}

#[cfg(target_vendor = "apple")]
fn ensure_regular_file_owner_access(
    parent: &OwnedFd,
    name: &OsStr,
    expected: FileIdentity,
    expected_device: u64,
    _mode: RawMode,
) -> io::Result<()> {
    let stat = rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    validate_owned_single_link_regular(&stat, name, expected_device, "capability lock")?;
    if identity_from_stat(&stat) != expected {
        return Err(identity_changed(name));
    }
    let anchored_path = apple_anchored_entry_path(parent, name);
    if apple_acl::path_has_any_extended_acl(&anchored_path)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "mode-inaccessible capability file has an ACL and cannot be normalized",
        ));
    }
    if rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .map(|stat| identity_from_stat(&stat))
        .map_err(io::Error::from)?
        != expected
    {
        return Err(identity_changed(name));
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "mode-inaccessible capability file requires manual repair: {}",
            Path::new(name).display()
        ),
    ))
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
fn ensure_regular_file_owner_access(
    _parent: &OwnedFd,
    name: &OsStr,
    _expected: FileIdentity,
    _expected_device: u64,
    _mode: RawMode,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "safe mode-inaccessible file authentication is unsupported on this platform: {}",
            Path::new(name).display()
        ),
    ))
}

#[cfg(target_vendor = "apple")]
mod apple_acl {
    use std::ffi::{c_int, c_void, CString};
    use std::io;
    use std::os::fd::{AsFd, AsRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::Path;
    use std::ptr;

    const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;
    const ACL_FIRST_ENTRY: c_int = 0;
    const ACL_NEXT_ENTRY: c_int = -1;
    const ACL_EXTENDED_ALLOW: c_int = 1;
    const ACL_ENTRY_ONLY_INHERIT: c_int = 1 << 8;
    const DANGEROUS_PERMISSIONS: [c_int; 8] = [
        1 << 2,  // write data / add file
        1 << 4,  // delete
        1 << 5,  // append data / add subdirectory
        1 << 6,  // delete child
        1 << 8,  // write attributes
        1 << 10, // write extended attributes
        1 << 12, // write security
        1 << 13, // change owner
    ];

    #[repr(C)]
    struct AclOpaque {
        _private: [u8; 0],
    }

    #[repr(C)]
    struct AclEntryOpaque {
        _private: [u8; 0],
    }

    #[repr(C)]
    struct AclPermsetOpaque {
        _private: [u8; 0],
    }

    #[repr(C)]
    struct AclFlagsetOpaque {
        _private: [u8; 0],
    }

    unsafe extern "C" {
        fn acl_init(count: c_int) -> *mut AclOpaque;
        fn acl_free(object: *mut c_void) -> c_int;
        fn acl_get_fd_np(fd: c_int, acl_type: c_int) -> *mut AclOpaque;
        fn acl_get_link_np(path: *const libc::c_char, acl_type: c_int) -> *mut AclOpaque;
        fn acl_get_entry(
            acl: *mut AclOpaque,
            entry_id: c_int,
            entry: *mut *mut AclEntryOpaque,
        ) -> c_int;
        fn acl_get_perm_np(permset: *mut AclPermsetOpaque, permission: c_int) -> c_int;
        fn acl_get_permset(
            entry: *mut AclEntryOpaque,
            permset: *mut *mut AclPermsetOpaque,
        ) -> c_int;
        fn acl_get_flag_np(flagset: *mut AclFlagsetOpaque, flag: c_int) -> c_int;
        fn acl_get_flagset_np(object: *mut c_void, flagset: *mut *mut AclFlagsetOpaque) -> c_int;
        fn acl_get_tag_type(entry: *mut AclEntryOpaque, tag: *mut c_int) -> c_int;
        fn acl_set_fd_np(fd: c_int, acl: *mut AclOpaque, acl_type: c_int) -> c_int;
    }

    struct OwnedAcl(*mut AclOpaque);

    impl Drop for OwnedAcl {
        fn drop(&mut self) {
            // SAFETY: every `OwnedAcl` is constructed from a non-null pointer
            // returned by an ACL allocation/getter and is freed exactly once.
            let _ = unsafe { acl_free(self.0.cast()) };
        }
    }

    pub(super) fn has_any_extended_acl(fd: impl AsFd) -> io::Result<bool> {
        // SAFETY: the borrowed descriptor is live for this call; the returned
        // ACL is independently allocated and immediately wrapped for one free.
        let acl = unsafe { acl_get_fd_np(fd.as_fd().as_raw_fd(), ACL_TYPE_EXTENDED) };
        if acl.is_null() {
            let source = io::Error::last_os_error();
            if source.raw_os_error() == Some(libc::ENOENT) {
                return Ok(false);
            }
            return Err(source);
        }
        let acl = OwnedAcl(acl);
        let mut entry = ptr::null_mut();
        // SAFETY: `acl` remains live and `entry` points to writable storage for
        // the borrowed entry pointer.
        match unsafe { acl_get_entry(acl.0, ACL_FIRST_ENTRY, &mut entry) } {
            0 => Ok(true),
            _ => {
                let source = io::Error::last_os_error();
                if source.raw_os_error() == Some(libc::EINVAL) {
                    Ok(false)
                } else {
                    Err(source)
                }
            }
        }
    }

    pub(super) fn path_has_any_extended_acl(path: &Path) -> io::Result<bool> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "ACL path contains an interior NUL",
            )
        })?;
        // SAFETY: `path` is NUL-terminated and live for the call; the returned
        // ACL is independently allocated.
        let acl = unsafe { acl_get_link_np(path.as_ptr(), ACL_TYPE_EXTENDED) };
        if acl.is_null() {
            let source = io::Error::last_os_error();
            if source.raw_os_error() == Some(libc::ENOENT) {
                return Ok(false);
            }
            return Err(source);
        }
        let acl = OwnedAcl(acl);
        let mut entry = ptr::null_mut();
        // SAFETY: the ACL and output storage remain live for the call.
        match unsafe { acl_get_entry(acl.0, ACL_FIRST_ENTRY, &mut entry) } {
            0 => Ok(true),
            _ => {
                let source = io::Error::last_os_error();
                if source.raw_os_error() == Some(libc::EINVAL) {
                    Ok(false)
                } else {
                    Err(source)
                }
            }
        }
    }

    pub(super) fn has_namespace_authority_acl(fd: impl AsFd) -> io::Result<bool> {
        // SAFETY: the borrowed descriptor is live for this call; the returned
        // ACL is independently allocated and immediately wrapped for one free.
        let acl = unsafe { acl_get_fd_np(fd.as_fd().as_raw_fd(), ACL_TYPE_EXTENDED) };
        if acl.is_null() {
            let source = io::Error::last_os_error();
            if source.raw_os_error() == Some(libc::ENOENT) {
                return Ok(false);
            }
            return Err(source);
        }
        let acl = OwnedAcl(acl);
        let mut entry_id = ACL_FIRST_ENTRY;
        loop {
            let mut entry = ptr::null_mut();
            // SAFETY: `acl` remains live, and `entry` points to writable
            // storage for the borrowed entry pointer. The entry is owned by
            // `acl`.
            match unsafe { acl_get_entry(acl.0, entry_id, &mut entry) } {
                0 => {}
                _ => {
                    let source = io::Error::last_os_error();
                    if source.raw_os_error() == Some(libc::EINVAL) {
                        return Ok(false);
                    }
                    return Err(source);
                }
            }
            entry_id = ACL_NEXT_ENTRY;
            let mut tag = 0;
            // SAFETY: `entry` is a live entry borrowed from `acl`, and `tag`
            // points to writable storage for the returned tag.
            if unsafe { acl_get_tag_type(entry, &mut tag) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // Protective deny entries reduce authority. They are intentionally
            // accepted; only an allow entry that grants namespace or metadata
            // mutation makes the descriptor unauthentic.
            if tag != ACL_EXTENDED_ALLOW {
                continue;
            }
            let mut flags = ptr::null_mut();
            // SAFETY: `entry` is live and may be passed as the generic ACL
            // object pointer; `flags` receives a flagset borrowed from it.
            if unsafe { acl_get_flagset_np(entry.cast(), &mut flags) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // An inherit-only allow ACE does not grant authority over the
            // current directory. Children are still normalized to an empty
            // ACL before their names are published.
            // SAFETY: `flags` is borrowed from the live ACL entry above and
            // remains valid for this query.
            match unsafe { acl_get_flag_np(flags, ACL_ENTRY_ONLY_INHERIT) } {
                1 => continue,
                0 => {}
                _ => return Err(io::Error::last_os_error()),
            }
            let mut permissions = ptr::null_mut();
            // SAFETY: `entry` remains live and `permissions` points to writable
            // storage for a permset borrowed from the same ACL.
            if unsafe { acl_get_permset(entry, &mut permissions) } != 0 {
                return Err(io::Error::last_os_error());
            }
            for permission in DANGEROUS_PERMISSIONS {
                // SAFETY: `permissions` is borrowed from the live ACL entry.
                match unsafe { acl_get_perm_np(permissions, permission) } {
                    0 => {}
                    1 => return Ok(true),
                    _ => return Err(io::Error::last_os_error()),
                }
            }
        }
    }

    pub(super) fn strip_extended_acl(fd: impl AsFd) -> io::Result<()> {
        // SAFETY: `acl_init(0)` allocates an empty ACL owned by the caller.
        let empty = unsafe { acl_init(0) };
        if empty.is_null() {
            return Err(io::Error::last_os_error());
        }
        let empty = OwnedAcl(empty);
        // SAFETY: the descriptor is live and `empty` is a valid ACL_TYPE_EXTENDED
        // object for the duration of the call; neither is retained by libc.
        if unsafe { acl_set_fd_np(fd.as_fd().as_raw_fd(), empty.0, ACL_TYPE_EXTENDED) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if has_any_extended_acl(fd)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "extended ACL removal did not establish an empty descriptor ACL",
            ));
        }
        Ok(())
    }
}

#[cfg(target_vendor = "apple")]
fn has_extended_acl(fd: impl AsFd) -> io::Result<bool> {
    apple_acl::has_any_extended_acl(fd)
}

#[cfg(target_vendor = "apple")]
fn has_namespace_authority_acl(fd: impl AsFd) -> io::Result<bool> {
    apple_acl::has_namespace_authority_acl(fd)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod posix_acl {
    use std::ffi::{c_char, c_void, CString};
    use std::io;
    use std::os::fd::{AsFd, AsRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::Path;

    const ACCESS_ACL: &[u8] = b"system.posix_acl_access\0";
    const DEFAULT_ACL: &[u8] = b"system.posix_acl_default\0";

    fn acl_present(fd: impl AsFd, name: &'static [u8]) -> io::Result<bool> {
        // SAFETY: `name` is static and NUL-terminated, the borrowed descriptor
        // is live, and a null value buffer requests only the xattr length.
        let size = unsafe {
            libc::fgetxattr(
                fd.as_fd().as_raw_fd(),
                name.as_ptr().cast::<c_char>(),
                std::ptr::null_mut::<c_void>(),
                0,
            )
        };
        if size >= 0 {
            return Ok(true);
        }
        let source = io::Error::last_os_error();
        if source.raw_os_error() == Some(libc::ENODATA) {
            return Ok(false);
        }
        Err(source)
    }

    pub(super) fn has_extended_acl(fd: impl AsFd) -> io::Result<bool> {
        Ok(acl_present(&fd, ACCESS_ACL)? || acl_present(fd, DEFAULT_ACL)?)
    }

    pub(super) fn path_has_extended_acl(path: &Path) -> io::Result<bool> {
        Ok(path_acl_present(path, ACCESS_ACL)? || path_acl_present(path, DEFAULT_ACL)?)
    }

    fn path_acl_present(path: &Path, name: &'static [u8]) -> io::Result<bool> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "ACL inspection path contains an interior NUL",
            )
        })?;
        // SAFETY: `path` and `name` are NUL-terminated and live for the call;
        // a null value buffer requests only the xattr length.
        let size = unsafe {
            libc::getxattr(
                path.as_ptr(),
                name.as_ptr().cast::<c_char>(),
                std::ptr::null_mut::<c_void>(),
                0,
            )
        };
        if size >= 0 {
            return Ok(true);
        }
        let source = io::Error::last_os_error();
        if source.raw_os_error() == Some(libc::ENODATA) {
            return Ok(false);
        }
        Err(source)
    }

    fn remove_acl(fd: impl AsFd, name: &'static [u8]) -> io::Result<()> {
        // SAFETY: `name` is static and NUL-terminated and the borrowed
        // descriptor remains live for the call.
        if unsafe { libc::fremovexattr(fd.as_fd().as_raw_fd(), name.as_ptr().cast::<c_char>()) }
            == 0
        {
            return Ok(());
        }
        let source = io::Error::last_os_error();
        if source.raw_os_error() == Some(libc::ENODATA) {
            Ok(())
        } else {
            Err(source)
        }
    }

    pub(super) fn strip_extended_acl(fd: impl AsFd) -> io::Result<()> {
        remove_acl(&fd, ACCESS_ACL)?;
        remove_acl(&fd, DEFAULT_ACL)?;
        if has_extended_acl(fd)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "POSIX ACL removal did not establish an empty descriptor ACL",
            ));
        }
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn has_extended_acl(fd: impl AsFd) -> io::Result<bool> {
    posix_acl::has_extended_acl(fd)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn has_namespace_authority_acl(fd: impl AsFd) -> io::Result<bool> {
    posix_acl::has_extended_acl(fd)
}

#[cfg(all(
    not(target_vendor = "apple"),
    not(any(target_os = "linux", target_os = "android"))
))]
fn has_extended_acl(_fd: impl AsFd) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "descriptor ACL verification is unavailable on this Unix platform",
    ))
}

#[cfg(all(
    not(target_vendor = "apple"),
    not(any(target_os = "linux", target_os = "android"))
))]
fn has_namespace_authority_acl(_fd: impl AsFd) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "descriptor namespace ACL verification is unavailable on this Unix platform",
    ))
}

#[cfg(target_vendor = "apple")]
fn strip_extended_acl(fd: impl AsFd) -> io::Result<()> {
    apple_acl::strip_extended_acl(fd)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn strip_extended_acl(fd: impl AsFd) -> io::Result<()> {
    posix_acl::strip_extended_acl(fd)
}

#[cfg(all(
    not(target_vendor = "apple"),
    not(any(target_os = "linux", target_os = "android"))
))]
fn strip_extended_acl(_fd: impl AsFd) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "descriptor ACL removal is unavailable on this Unix platform",
    ))
}

fn sync_directory_barrier(fd: impl AsFd) -> io::Result<()> {
    sync_fsync(fd)
}

pub(super) fn sync_file_barrier(fd: impl AsFd) -> io::Result<()> {
    sync_fsync(&fd)?;
    // F_FULLFSYNC provides the stronger data-file power-loss guarantee on
    // Apple platforms. Directory namespace durability uses `fsync` above:
    // F_FULLFSYNC is not a portable directory operation across macOS
    // filesystems, while any data-file failure here remains explicit.
    #[cfg(target_vendor = "apple")]
    loop {
        match rfs::fcntl_fullfsync(&fd) {
            Ok(()) => break,
            Err(rustix::io::Errno::INTR) => continue,
            Err(source) => {
                let source = io::Error::from(source);
                return Err(io::Error::new(
                    source.kind(),
                    format!(
                        "macOS F_FULLFSYNC barrier failed; power-loss durability was not established: {source}"
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn sync_fsync(fd: impl AsFd) -> io::Result<()> {
    loop {
        match rfs::fsync(&fd) {
            Ok(()) => return Ok(()),
            Err(rustix::io::Errno::INTR) => continue,
            Err(source) => return Err(source.into()),
        }
    }
}

fn set_new_entry_mode(fd: &OwnedFd, _name: &OsStr, mode: RawMode) -> io::Result<()> {
    let before = rfs::fstat(fd).map_err(io::Error::from)?;
    let before_type = FileType::from_raw_mode(before.st_mode);
    if !matches!(before_type, FileType::RegularFile | FileType::Directory) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "new capability entry is not a regular file or directory: {}",
                Path::new(_name).display()
            ),
        ));
    }
    let effective_uid = rustix::process::geteuid().as_raw();
    if before.st_uid != effective_uid
        || (before_type == FileType::RegularFile && before.st_nlink != 1)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "new capability entry ownership or link count is unauthentic: {}",
                Path::new(_name).display()
            ),
        ));
    }
    // Inspection must succeed before the first permission mutation. Newly
    // created entries may inherit an ACL; it is removed through the retained
    // descriptor before the exact mode is established.
    let _inherited_acl = has_extended_acl(fd)?;
    strip_extended_acl(fd)?;
    if has_extended_acl(fd)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "new capability entry retained an ACL after normalization",
        ));
    }
    #[cfg(test)]
    if INJECT_NEW_ENTRY_CHMOD_FAILURE_FOR.with(|configured| {
        let mut configured = configured.borrow_mut();
        if configured.as_deref() == Some(_name) {
            configured.take();
            true
        } else {
            false
        }
    }) {
        return Err(io::Error::other(
            "injected capability entry permission correction failure",
        ));
    }
    rfs::fchmod(fd, Mode::from_bits_truncate(mode)).map_err(io::Error::from)?;
    let after = rfs::fstat(fd).map_err(io::Error::from)?;
    if identity_from_stat(&after) != identity_from_stat(&before)
        || FileType::from_raw_mode(after.st_mode) != before_type
        || after.st_uid != effective_uid
        || (before_type == FileType::RegularFile && after.st_nlink != 1)
        || ((after.st_mode as RawMode) & 0o777) != (mode & 0o777)
        || has_extended_acl(fd)?
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "new capability entry normalization did not establish an exact mode and empty ACL: {}",
                Path::new(_name).display()
            ),
        ));
    }
    Ok(())
}

fn error_after_cleanup(
    source: io::Error,
    remove_result: io::Result<()>,
    sync_result: io::Result<()>,
) -> io::Error {
    match (remove_result, sync_result) {
        (Ok(()), Ok(())) => source,
        (remove_result, sync_result) => {
            let remove_message = remove_result
                .err()
                .map_or_else(|| "removed".to_string(), |error| error.to_string());
            let sync_message = sync_result
                .err()
                .map_or_else(|| "synchronized".to_string(), |error| error.to_string());
            io::Error::new(
                source.kind(),
                format!(
                    "{source}; cleanup after creation failed (remove: {remove_message}; parent sync: {sync_message})"
                ),
            )
        }
    }
}

fn identity_from_stat(stat: &rfs::Stat) -> FileIdentity {
    FileIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
    }
}

fn capability_entry_metadata(stat: &rfs::Stat) -> io::Result<CapabilityEntryMetadata> {
    let kind = match FileType::from_raw_mode(stat.st_mode) {
        FileType::Symlink => CapabilityEntryKind::Symlink,
        FileType::RegularFile => CapabilityEntryKind::RegularFile,
        FileType::Directory => CapabilityEntryKind::Directory,
        _ => CapabilityEntryKind::Other,
    };
    let logical_bytes = u64::try_from(stat.st_size).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "capability entry has a negative logical size",
        )
    })?;
    let allocated_bytes = u64::try_from(stat.st_blocks)
        .unwrap_or(0)
        .saturating_mul(512);
    Ok(CapabilityEntryMetadata {
        kind,
        identity: identity_from_stat(stat),
        logical_bytes,
        allocated_bytes,
        modified_unix_seconds: stat.st_mtime,
        modified_subsec_nanos: u32::try_from(stat.st_mtime_nsec).unwrap_or(0),
        link_count: u64::from(stat.st_nlink),
    })
}

fn validate_workspace_namespace_ancestors(path: &Path, root_stat: &rfs::Stat) -> io::Result<()> {
    let effective_uid = rustix::process::geteuid().as_raw();
    let mut child_uid = root_stat.st_uid;
    for ancestor in path.ancestors().skip(1) {
        let ancestor_fd =
            rfs::open(ancestor, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
        let ancestor_stat = rfs::fstat(&ancestor_fd).map_err(io::Error::from)?;
        if FileType::from_raw_mode(ancestor_stat.st_mode) != FileType::Directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "workspace namespace ancestor is not a directory: {}",
                    ancestor.display()
                ),
            ));
        }
        if has_namespace_authority_acl(&ancestor_fd)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "workspace namespace ancestor has extended ACL authority: {}",
                    ancestor.display()
                ),
            ));
        }
        let mode = ancestor_stat.st_mode as RawMode;
        let non_owner_writable = (mode & 0o022) != 0;
        let sticky = (mode & 0o1000) != 0;
        if non_owner_writable && !(sticky && child_uid == effective_uid) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "workspace namespace ancestor permits replacement without safe sticky ownership semantics: {}",
                    ancestor.display()
                ),
            ));
        }
        child_uid = ancestor_stat.st_uid;
    }
    Ok(())
}

fn validate_owned_directory(
    stat: &rfs::Stat,
    name: &OsStr,
    expected_device: u64,
    kind: &str,
) -> io::Result<()> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{kind} is not a directory: {}", Path::new(name).display()),
        ));
    }
    let effective_uid = rustix::process::geteuid().as_raw();
    if stat.st_uid != effective_uid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} is owned by uid {}; expected uid {effective_uid}: {}",
                stat.st_uid,
                Path::new(name).display()
            ),
        ));
    }
    if stat.st_dev as u64 != expected_device {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} is on device {}; expected device {expected_device}: {}",
                stat.st_dev,
                Path::new(name).display()
            ),
        ));
    }
    Ok(())
}

fn validate_owned_single_link_regular(
    stat: &rfs::Stat,
    name: &OsStr,
    expected_device: u64,
    kind: &str,
) -> io::Result<()> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} is not a regular file: {}",
                Path::new(name).display()
            ),
        ));
    }
    let effective_uid = rustix::process::geteuid().as_raw();
    if stat.st_uid != effective_uid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} is owned by uid {}; expected uid {effective_uid}: {}",
                stat.st_uid,
                Path::new(name).display()
            ),
        ));
    }
    if stat.st_nlink != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} has {} links; expected exactly one: {}",
                stat.st_nlink,
                Path::new(name).display()
            ),
        ));
    }
    if stat.st_dev as u64 != expected_device {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} is on device {}; expected device {expected_device}: {}",
                stat.st_dev,
                Path::new(name).display()
            ),
        ));
    }
    Ok(())
}

fn validate_owned_regular_link_count(
    stat: &rfs::Stat,
    name: &OsStr,
    expected_device: u64,
    expected_links: u64,
    kind: &str,
) -> io::Result<()> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} is not a regular file: {}",
                Path::new(name).display()
            ),
        ));
    }
    let effective_uid = rustix::process::geteuid().as_raw();
    if stat.st_uid != effective_uid
        || stat.st_dev as u64 != expected_device
        || stat.st_nlink as u64 != expected_links
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} has uid {}, device {}, and {} links; expected uid {effective_uid}, device {expected_device}, and {expected_links} links: {}",
                stat.st_uid,
                stat.st_dev,
                stat.st_nlink,
                Path::new(name).display()
            ),
        ));
    }
    Ok(())
}

fn validate_owned_regular_read_snapshot(
    stat: &rfs::Stat,
    name: &OsStr,
    expected_device: u64,
    kind: &str,
) -> io::Result<()> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} is not a regular file: {}",
                Path::new(name).display()
            ),
        ));
    }
    let effective_uid = rustix::process::geteuid().as_raw();
    if stat.st_uid != effective_uid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} is owned by uid {}; expected uid {effective_uid}: {}",
                stat.st_uid,
                Path::new(name).display()
            ),
        ));
    }
    if stat.st_nlink > 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} has {} links; expected an anchored snapshot with at most one: {}",
                stat.st_nlink,
                Path::new(name).display()
            ),
        ));
    }
    if stat.st_dev as u64 != expected_device {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{kind} is on device {}; expected device {expected_device}: {}",
                stat.st_dev,
                Path::new(name).display()
            ),
        ));
    }
    Ok(())
}

fn validate_authenticated_read_mode(stat: &rfs::Stat, name: &OsStr) -> io::Result<RawMode> {
    let mode = (stat.st_mode as RawMode) & 0o777;
    if (mode & 0o400) == 0 || (mode & 0o022) != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "capability managed file is not owner-readable with no non-owner write authority: {}",
                Path::new(name).display()
            ),
        ));
    }
    Ok(mode)
}

fn authenticated_read_entry_identity_from_stat(
    stat: &rfs::Stat,
    name: &OsStr,
    expected_device: u64,
) -> io::Result<Option<FileIdentity>> {
    validate_owned_regular_read_snapshot(stat, name, expected_device, "capability managed file")?;
    // Darwin may report the old inode with zero links when `fstatat` races an
    // atomic replacement. This is the path-side equivalent of an already-open
    // descriptor becoming detached: retry it, but continue to reject hard
    // links and every other failed authentication above.
    if stat.st_nlink == 0 {
        return Ok(None);
    }
    validate_authenticated_read_mode(stat, name)?;
    Ok(Some(identity_from_stat(stat)))
}

fn authenticate_read_descriptor(
    fd: impl AsFd,
    name: &OsStr,
    expected_device: u64,
    kind: &str,
) -> io::Result<AuthenticatedReadDescriptor> {
    let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
    validate_owned_regular_read_snapshot(&stat, name, expected_device, kind)?;
    validate_authenticated_read_mode(&stat, name)?;
    if has_extended_acl(&fd)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "capability managed file has an extended ACL and cannot be authenticated automatically: {}",
                Path::new(name).display()
            ),
        ));
    }
    // Atomic replacement may detach this already-open, authenticated inode.
    // It remains safe to inspect, but the caller must reopen before accepting it.
    Ok(AuthenticatedReadDescriptor {
        identity: identity_from_stat(&stat),
        attached: stat.st_nlink == 1,
    })
}

fn authenticated_read_not_found_error(name: &OsStr) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "capability managed file is absent: {}",
            Path::new(name).display()
        ),
    )
}

fn authenticated_read_retry_limit_error(name: &OsStr) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "capability managed file changed during {MAX_AUTHENTICATED_READ_ATTEMPTS} consecutive authenticated read attempts: {}",
            Path::new(name).display()
        ),
    )
}

fn unique_name(prefix: &str) -> OsString {
    #[cfg(test)]
    if let Some(name) = INJECT_UNIQUE_NAMES.with(|configured| configured.borrow_mut().pop_front()) {
        return name;
    }
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hasher = blake3::Hasher::new();
    hasher.update(process_nonce());
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(&counter.to_le_bytes());
    let entropy = hasher.finalize().to_hex();
    OsString::from(format!(
        "{prefix}-{}-{counter}-{}",
        std::process::id(),
        &entropy.as_str()[..32]
    ))
}

fn process_nonce() -> &'static [u8; 16] {
    PROCESS_NONCE.get_or_init(|| {
        let mut nonce = [0_u8; 16];
        if File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut nonce))
            .is_ok()
        {
            return nonce;
        }
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let mut hasher = blake3::Hasher::new();
        hasher.update(&std::process::id().to_le_bytes());
        hasher.update(&elapsed.as_nanos().to_le_bytes());
        hasher.update(&(std::ptr::addr_of!(PROCESS_NONCE) as usize).to_le_bytes());
        nonce.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
        nonce
    })
}

fn directory_initializer_prefix(name: &OsStr) -> String {
    let digest = blake3::hash(name.as_bytes()).to_hex();
    format!(".directory-init-{}", &digest.as_str()[..16])
}

fn lock_initializer_prefix(name: &OsStr) -> String {
    let digest = blake3::hash(name.as_bytes()).to_hex();
    format!(".lock-init-{}", &digest.as_str()[..16])
}

/// Returns whether `candidate` is an exact capability-generated initializer
/// for the named lock file.
///
/// Callers must still hold the synchronization authority that proves no live
/// publisher can own the initializer before reclaiming a matching entry.
#[cfg(test)]
fn lock_initializer_name_matches(candidate: &OsStr, lock_name: &OsStr) -> bool {
    generated_name_matches(candidate, &lock_initializer_prefix(lock_name))
}

fn append_initializer_prefix(name: &OsStr) -> String {
    let digest = blake3::hash(name.as_bytes()).to_hex();
    format!(".append-init-{}", &digest.as_str()[..16])
}

#[cfg(test)]
fn directory_initializer_name_matches(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(suffix) = name.strip_prefix(".directory-init-") else {
        return false;
    };
    let Some((digest, _)) = suffix.split_once('-') else {
        return false;
    };
    digest.len() == 16
        && digest.bytes().all(is_lower_hex_digit)
        && generated_name_matches(OsStr::new(name), &format!(".directory-init-{digest}"))
}

pub(super) fn generated_deletion_prefix(prefix: &str) -> Cow<'static, str> {
    match prefix {
        RETENTION_JOURNAL_WRITE_TEMP_PREFIX => {
            Cow::Borrowed(RETENTION_JOURNAL_WRITE_DELETION_TEMP_PREFIX)
        }
        TASK_REGISTRY_WRITE_TEMP_PREFIX => Cow::Borrowed(TASK_REGISTRY_WRITE_DELETION_TEMP_PREFIX),
        ACTIVE_TASK_WRITE_TEMP_PREFIX => Cow::Borrowed(ACTIVE_TASK_WRITE_DELETION_TEMP_PREFIX),
        #[cfg(test)]
        TEST_ATOMIC_WRITE_TEMP_PREFIX => Cow::Borrowed(TEST_ATOMIC_WRITE_DELETION_TEMP_PREFIX),
        _ => Cow::Owned(format!("{prefix}-deleting")),
    }
}

pub(super) fn generated_name_matches(name: &OsStr, prefix: &str) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(suffix) = name
        .strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_prefix('-'))
    else {
        return false;
    };
    let mut parts = suffix.split('-');
    let (Some(process_id), Some(counter)) = (parts.next(), parts.next()) else {
        return false;
    };
    let entropy_is_valid = match (parts.next(), parts.next()) {
        (None, None) => true,
        (Some(entropy), None) => entropy.len() == 32 && entropy.bytes().all(is_lower_hex_digit),
        _ => false,
    };
    !process_id.is_empty()
        && !counter.is_empty()
        && process_id.bytes().all(|byte| byte.is_ascii_digit())
        && counter.bytes().all(|byte| byte.is_ascii_digit())
        && process_id.parse::<u32>().is_ok()
        && counter.parse::<u64>().is_ok()
        && entropy_is_valid
}

fn is_lower_hex_digit(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

fn validate_normal_component(name: &OsStr) -> io::Result<()> {
    let mut components = Path::new(name).components();
    if matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none() {
        Ok(())
    } else {
        Err(invalid_component(Path::new(name)))
    }
}

fn invalid_component(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("path is not one normalized component: {}", path.display()),
    )
}

fn capability_entry_limit_error(max_entries: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("capability traversal exceeds the {max_entries}-entry limit"),
    )
}

fn capability_depth_limit_error(max_depth: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("capability traversal exceeds the maximum depth of {max_depth}"),
    )
}

fn unique_name_attempts_exhausted(kind: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "could not allocate a collision-free {kind} after {MAX_UNIQUE_NAME_ATTEMPTS} attempts"
        ),
    )
}

fn identity_changed(name: &OsStr) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "capability entry identity changed before deletion: {}",
            Path::new(name).display()
        ),
    )
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos"
))]
fn renameat_noreplace(
    source_dir: &OwnedFd,
    source_name: &OsStr,
    destination_dir: &OwnedFd,
    destination_name: &OsStr,
) -> io::Result<()> {
    rfs::renameat_with(
        source_dir,
        source_name,
        destination_dir,
        destination_name,
        RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos"
)))]
fn renameat_noreplace(
    _source_dir: &OwnedFd,
    _source_name: &OsStr,
    _destination_dir: &OwnedFd,
    _destination_name: &OsStr,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::process::ExitStatusExt as _;
    use std::process::Command;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};

    use tempfile::tempdir;

    use super::*;

    const UMASK_CASE_ENV: &str = "PACKET28_CAPABILITY_UMASK_CASE";
    const FIFO_CASE_ENV: &str = "PACKET28_CAPABILITY_FIFO_CASE";
    const INITIALIZER_KILL_CASE_ENV: &str = "PACKET28_CAPABILITY_INIT_KILL_CASE";
    const INITIALIZER_KILL_ROOT_ENV: &str = "PACKET28_CAPABILITY_INIT_KILL_ROOT";
    const NAMESPACE_CASE_ENV: &str = "PACKET28_CAPABILITY_NAMESPACE_CASE";
    const NAMESPACE_ROOT_ENV: &str = "PACKET28_CAPABILITY_NAMESPACE_ROOT";

    #[cfg(target_os = "linux")]
    fn set_test_access_acl(file: &fs::File) {
        use std::os::fd::AsRawFd as _;

        const ACCESS_ACL: &[u8] = b"system.posix_acl_access\0";
        const ACL_UNDEFINED_ID: u32 = u32::MAX;
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&2_u32.to_le_bytes());
        let mut push_entry = |tag: u16, permissions: u16, id: u32| {
            encoded.extend_from_slice(&tag.to_le_bytes());
            encoded.extend_from_slice(&permissions.to_le_bytes());
            encoded.extend_from_slice(&id.to_le_bytes());
        };
        // SAFETY: `geteuid` accepts no pointer arguments and has no
        // preconditions.
        let named_uid = unsafe { libc::geteuid() }.wrapping_add(1);
        push_entry(0x01, 0o6, ACL_UNDEFINED_ID);
        push_entry(0x02, 0o4, named_uid);
        push_entry(0x04, 0o0, ACL_UNDEFINED_ID);
        push_entry(0x10, 0o4, ACL_UNDEFINED_ID);
        push_entry(0x20, 0o0, ACL_UNDEFINED_ID);
        // SAFETY: the descriptor and NUL-terminated name are live, and the
        // buffer contains the kernel's fixed POSIX ACL xattr representation.
        let result = unsafe {
            libc::fsetxattr(
                file.as_raw_fd(),
                ACCESS_ACL.as_ptr().cast(),
                encoded.as_ptr().cast(),
                encoded.len(),
                0,
            )
        };
        assert_eq!(
            result,
            0,
            "failed to seed POSIX ACL: {}",
            io::Error::last_os_error()
        );
    }

    #[cfg(target_os = "linux")]
    fn test_access_acl_bytes(file: &fs::File) -> Vec<u8> {
        use std::os::fd::AsRawFd as _;

        const ACCESS_ACL: &[u8] = b"system.posix_acl_access\0";
        // SAFETY: a null value buffer requests only the live xattr length.
        let len = unsafe {
            libc::fgetxattr(
                file.as_raw_fd(),
                ACCESS_ACL.as_ptr().cast(),
                std::ptr::null_mut(),
                0,
            )
        };
        assert!(
            len > 0,
            "POSIX ACL xattr is absent: {}",
            io::Error::last_os_error()
        );
        let mut bytes = vec![0_u8; len as usize];
        // SAFETY: `bytes` has exactly the queried length and the descriptor
        // remains open for the duration of the call.
        let read = unsafe {
            libc::fgetxattr(
                file.as_raw_fd(),
                ACCESS_ACL.as_ptr().cast(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        };
        assert_eq!(
            read,
            len,
            "failed to read POSIX ACL: {}",
            io::Error::last_os_error()
        );
        bytes
    }

    struct UmaskGuard(Mode);

    impl UmaskGuard {
        fn set(mask: RawMode) -> Self {
            Self(rustix::process::umask(Mode::from_bits_truncate(mask)))
        }
    }

    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            rustix::process::umask(self.0);
        }
    }

    fn enter_isolated_umask_case(case: &str, test_name: &str) -> bool {
        if std::env::var_os(UMASK_CASE_ENV).as_deref() == Some(OsStr::new(case)) {
            return true;
        }
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(test_name)
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(UMASK_CASE_ENV, case)
            .status()
            .unwrap();
        assert!(status.success(), "isolated umask test process failed");
        false
    }

    fn enter_isolated_fifo_case(test_name: &str) -> bool {
        if std::env::var_os(FIFO_CASE_ENV).is_some() {
            return true;
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(test_name)
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(FIFO_CASE_ENV, "1")
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "isolated FIFO test process failed");
                return false;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("FIFO capability operation blocked past its deadline");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn run_initializer_kill_child(root: &Path, case: &str) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("capability::tests::initializer_sigkill_residue_is_isolated_from_publication")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(INITIALIZER_KILL_CASE_ENV, case)
            .env(INITIALIZER_KILL_ROOT_ENV, root)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("initializer killpoint process missed its deadline");
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.signal(), Some(9));
    }

    fn capability_mode(directory: &CapabilityDir) -> RawMode {
        (rfs::fstat(&directory.fd).unwrap().st_mode as RawMode) & 0o777
    }

    fn path_mode(path: &Path) -> RawMode {
        (fs::metadata(path).unwrap().permissions().mode() & 0o777) as RawMode
    }

    #[test]
    fn directory_creation_restores_requested_mode_under_restrictive_umask() {
        if !enter_isolated_umask_case(
            "directories",
            "capability::tests::directory_creation_restores_requested_mode_under_restrictive_umask",
        ) {
            return;
        }
        let new_workspace = tempdir().unwrap();
        let new_parent = CapabilityDir::open(new_workspace.path()).unwrap();
        let existing_workspace = tempdir().unwrap();
        let existing_parent = CapabilityDir::open(existing_workspace.path()).unwrap();
        let preexisting_state = existing_parent
            .create_dir(OsStr::new(".packet28"), 0o700)
            .unwrap();
        let preexisting_daemon = preexisting_state
            .create_dir(OsStr::new("daemon"), 0o700)
            .unwrap();
        drop(preexisting_daemon);
        drop(preexisting_state);
        let _umask = UmaskGuard::set(0o777);

        let state = new_parent
            .ensure_dir_open(OsStr::new(".packet28"), 0o755)
            .unwrap();
        let daemon = state.ensure_dir_open(OsStr::new("daemon"), 0o755).unwrap();
        let quarantine = state
            .ensure_dir(OsStr::new(".retention-trash"), 0o700)
            .unwrap();
        let group = quarantine.create_dir(OsStr::new("group-1"), 0o700).unwrap();

        let reopened_state = existing_parent
            .ensure_dir_open(OsStr::new(".packet28"), 0o755)
            .unwrap();
        let reopened_daemon = reopened_state
            .ensure_dir_open(OsStr::new("daemon"), 0o755)
            .unwrap();

        assert_eq!(capability_mode(&state), 0o755);
        assert_eq!(capability_mode(&daemon), 0o755);
        assert_eq!(capability_mode(&quarantine), 0o700);
        assert_eq!(capability_mode(&group), 0o700);
        assert_eq!(capability_mode(&reopened_state), 0o755);
        assert_eq!(capability_mode(&reopened_daemon), 0o755);
    }

    #[test]
    fn files_restore_requested_mode_under_owner_filtering_umask() {
        if !enter_isolated_umask_case(
            "files",
            "capability::tests::files_restore_requested_mode_under_owner_filtering_umask",
        ) {
            return;
        }
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let preexisting_lock = root.path().join("preexisting.lock");
        fs::write(&preexisting_lock, []).unwrap();
        fs::set_permissions(&preexisting_lock, fs::Permissions::from_mode(0o000)).unwrap();
        let preexisting_identity = parent
            .entry_identity(OsStr::new("preexisting.lock"))
            .unwrap()
            .unwrap();
        let _umask = UmaskGuard::set(0o777);

        parent
            .write_json_atomically(
                OsStr::new("state.json"),
                br#"{"ok":true}"#,
                TEST_ATOMIC_WRITE_TEMP_PREFIX,
            )
            .unwrap();
        assert_eq!(
            parent
                .read_file_limited(OsStr::new("state.json"), 1024)
                .unwrap(),
            br#"{"ok":true}"#
        );
        assert_eq!(path_mode(&root.path().join("state.json")), 0o600);

        parent
            .write_json_atomically(
                OsStr::new("active-task"),
                br#"{"task_id":"first"}"#,
                ACTIVE_TASK_WRITE_TEMP_PREFIX,
            )
            .unwrap();
        assert_eq!(path_mode(&root.path().join("active-task")), 0o600);
        parent
            .write_json_atomically(
                OsStr::new("active-task"),
                br#"{"task_id":"second"}"#,
                ACTIVE_TASK_WRITE_TEMP_PREFIX,
            )
            .unwrap();
        assert_eq!(
            parent
                .read_file_limited(OsStr::new("active-task"), 1024)
                .unwrap(),
            br#"{"task_id":"second"}"#
        );
        assert_eq!(path_mode(&root.path().join("active-task")), 0o600);

        let lock = parent.open_lock_file(OsStr::new("new.lock")).unwrap();
        assert_eq!(path_mode(&root.path().join("new.lock")), 0o600);
        drop(lock);
        drop(parent.open_lock_file(OsStr::new("new.lock")).unwrap());

        assert_eq!(
            parent
                .open_lock_file(OsStr::new("preexisting.lock"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(path_mode(&preexisting_lock), 0o000);
        assert_eq!(
            parent
                .entry_identity(OsStr::new("preexisting.lock"))
                .unwrap(),
            Some(preexisting_identity)
        );
    }

    #[test]
    fn concurrent_lock_initialization_publishes_one_inode() {
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let barrier = Arc::new(Barrier::new(8));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let directory = parent.duplicate().unwrap();
            let barrier = Arc::clone(&barrier);
            workers.push(thread::spawn(move || {
                barrier.wait();
                let file = directory
                    .open_lock_file(OsStr::new("concurrent.lock"))
                    .unwrap();
                identity_from_stat(&rfs::fstat(&file).unwrap())
            }));
        }
        let identities: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(identities.iter().all(|identity| *identity == identities[0]));
        assert_eq!(path_mode(&root.path().join("concurrent.lock")), 0o600);
        let prefix = lock_initializer_prefix(OsStr::new("concurrent.lock"));
        let deletion_prefix = generated_deletion_prefix(&prefix);
        assert!(!parent.entries().unwrap().iter().any(|entry| {
            generated_name_matches(entry, &prefix)
                || generated_name_matches(entry, deletion_prefix.as_ref())
        }));

        let existing_target = OsStr::new("existing.events.jsonl");
        let mut existing = parent.open_append_file(existing_target).unwrap();
        existing.write_all(b"existing-event\n").unwrap();
        existing.sync_all().unwrap();
        drop(existing);
        let existing_identity = parent.entry_identity(existing_target).unwrap().unwrap();
        let existing_prefix = append_initializer_prefix(existing_target);
        let existing_deletion_prefix = generated_deletion_prefix(&existing_prefix);
        let existing_stale = OsString::from(format!("{existing_prefix}-4242-10"));
        let existing_tombstone = OsString::from(format!("{existing_deletion_prefix}-4242-11"));
        let existing_lookalike = format!("{existing_prefix}-user-data");
        fs::write(root.path().join(&existing_stale), b"stale-source").unwrap();
        fs::write(root.path().join(&existing_tombstone), b"stale-tombstone").unwrap();
        fs::write(root.path().join(&existing_lookalike), b"keep").unwrap();

        let reopened = parent.open_append_file(existing_target).unwrap();
        assert_eq!(
            identity_from_stat(&rfs::fstat(&reopened).unwrap()),
            existing_identity
        );
        drop(reopened);

        assert_eq!(
            parent.entry_identity(existing_target).unwrap(),
            Some(existing_identity)
        );
        assert_eq!(
            fs::read(root.path().join(existing_target)).unwrap(),
            b"existing-event\n"
        );
        assert!(!root.path().join(existing_stale).exists());
        assert!(!root.path().join(existing_tombstone).exists());
        assert_eq!(
            fs::read(root.path().join(existing_lookalike)).unwrap(),
            b"keep"
        );
        assert!(!parent.entries().unwrap().iter().any(|entry| {
            generated_name_matches(entry, &existing_prefix)
                || generated_name_matches(entry, existing_deletion_prefix.as_ref())
        }));
    }

    #[test]
    fn append_file_rejects_symlinks_hardlinks_and_fifos_without_external_mutation() {
        use std::os::unix::fs::{symlink, MetadataExt as _};

        if !enter_isolated_fifo_case(
            "capability::tests::append_file_rejects_symlinks_hardlinks_and_fifos_without_external_mutation",
        ) {
            return;
        }
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();

        let symlink_target = outside.path().join("symlink-target");
        fs::write(&symlink_target, b"outside-symlink").unwrap();
        fs::set_permissions(&symlink_target, fs::Permissions::from_mode(0o666)).unwrap();
        symlink(&symlink_target, root.path().join("symlink.events.jsonl")).unwrap();
        let symlink_mode = path_mode(&symlink_target);

        assert_eq!(
            parent
                .open_append_file(OsStr::new("symlink.events.jsonl"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read(&symlink_target).unwrap(), b"outside-symlink");
        assert_eq!(path_mode(&symlink_target), symlink_mode);

        let hardlink_target = outside.path().join("hardlink-target");
        fs::write(&hardlink_target, b"outside-hardlink").unwrap();
        fs::set_permissions(&hardlink_target, fs::Permissions::from_mode(0o666)).unwrap();
        let hardlink_entry = root.path().join("hardlink.events.jsonl");
        fs::hard_link(&hardlink_target, &hardlink_entry).unwrap();
        let hardlink_mode = path_mode(&hardlink_target);

        assert_eq!(
            parent
                .open_append_file(OsStr::new("hardlink.events.jsonl"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read(&hardlink_target).unwrap(), b"outside-hardlink");
        assert_eq!(path_mode(&hardlink_target), hardlink_mode);
        assert_eq!(fs::metadata(&hardlink_target).unwrap().nlink(), 2);
        assert_eq!(fs::metadata(&hardlink_entry).unwrap().nlink(), 2);

        let fifo = root.path().join("fifo.events.jsonl");
        assert!(Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        assert_eq!(
            parent
                .open_append_file(OsStr::new("fifo.events.jsonl"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            parent
                .entry_is_regular_file(OsStr::new("fifo.events.jsonl"))
                .unwrap(),
            Some(false)
        );
    }

    #[test]
    fn append_file_cleans_strict_stale_initializers_and_preserves_lookalikes() {
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let target = OsStr::new("task.events.jsonl");
        let prefix = append_initializer_prefix(target);
        let deletion_prefix = generated_deletion_prefix(&prefix);
        let stale_source = OsString::from(format!("{prefix}-4242-7"));
        let stale_tombstone = OsString::from(format!("{deletion_prefix}-4242-8"));
        let lookalikes = [
            format!("{prefix}-not-a-pid-9"),
            format!("{prefix}-4242"),
            format!("{deletion_prefix}-4242-9-extra"),
            format!("{prefix}-user-data"),
        ];
        fs::write(root.path().join(&stale_source), b"stale-source").unwrap();
        fs::write(root.path().join(&stale_tombstone), b"stale-tombstone").unwrap();
        for lookalike in &lookalikes {
            fs::write(root.path().join(lookalike), b"keep").unwrap();
        }

        let mut file = parent.open_append_file(target).unwrap();
        file.write_all(b"event\n").unwrap();
        file.sync_all().unwrap();
        drop(file);

        assert!(!root.path().join(stale_source).exists());
        assert!(!root.path().join(stale_tombstone).exists());
        for lookalike in lookalikes {
            assert_eq!(
                fs::read(root.path().join(lookalike)).unwrap(),
                b"keep",
                "initializer lookalike must not be removed"
            );
        }
        assert_eq!(fs::read(root.path().join(target)).unwrap(), b"event\n");
        assert_eq!(path_mode(&root.path().join(target)), 0o600);
        assert!(!parent.entries().unwrap().iter().any(|entry| {
            generated_name_matches(entry, &prefix)
                || generated_name_matches(entry, deletion_prefix.as_ref())
        }));
    }

    #[test]
    fn append_file_initializer_collision_exhaustion_preserves_colliders() {
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let collider = root.path().join("append-collider");
        fs::write(&collider, b"keep").unwrap();
        fs::set_permissions(&collider, fs::Permissions::from_mode(0o640)).unwrap();
        let collider_mode = path_mode(&collider);
        inject_unique_names(
            (0..MAX_UNIQUE_NAME_ATTEMPTS).map(|_| OsString::from("append-collider")),
        );

        assert_eq!(
            parent
                .open_append_file(OsStr::new("never-created.events.jsonl"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&collider).unwrap(), b"keep");
        assert_eq!(path_mode(&collider), collider_mode);
        assert!(!root.path().join("never-created.events.jsonl").exists());
        let prefix = append_initializer_prefix(OsStr::new("never-created.events.jsonl"));
        let deletion_prefix = generated_deletion_prefix(&prefix);
        assert!(!parent.entries().unwrap().iter().any(|entry| {
            generated_name_matches(entry, &prefix)
                || generated_name_matches(entry, deletion_prefix.as_ref())
        }));
    }

    #[test]
    fn concurrent_append_file_initialization_publishes_one_authenticated_inode() {
        use std::os::unix::fs::MetadataExt as _;

        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let barrier = Arc::new(Barrier::new(8));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let directory = parent.duplicate().unwrap();
            let barrier = Arc::clone(&barrier);
            workers.push(thread::spawn(move || {
                barrier.wait();
                let file = directory
                    .open_append_file(OsStr::new("concurrent.events.jsonl"))
                    .unwrap();
                let stat = rfs::fstat(&file).unwrap();
                (
                    identity_from_stat(&stat),
                    stat.st_nlink,
                    (stat.st_mode as RawMode) & 0o777,
                )
            }));
        }
        let snapshots: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(snapshots
            .iter()
            .all(|(identity, links, mode)| *identity == snapshots[0].0
                && *links == 1
                && *mode == 0o600));
        let published = fs::metadata(root.path().join("concurrent.events.jsonl")).unwrap();
        assert!(published.is_file());
        assert_eq!(published.nlink(), 1);
        assert_eq!(published.permissions().mode() & 0o777, 0o600);
        let prefix = append_initializer_prefix(OsStr::new("concurrent.events.jsonl"));
        let deletion_prefix = generated_deletion_prefix(&prefix);
        assert!(!parent.entries().unwrap().iter().any(|entry| {
            generated_name_matches(entry, &prefix)
                || generated_name_matches(entry, deletion_prefix.as_ref())
        }));
    }

    #[test]
    fn existing_targets_repair_only_when_authenticity_is_preserved() {
        if !enter_isolated_umask_case(
            "existing-targets",
            "capability::tests::existing_targets_repair_only_when_authenticity_is_preserved",
        ) {
            return;
        }
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let _umask = UmaskGuard::set(0o777);

        let owner_only_dir = root.path().join("owner-only-dir");
        fs::create_dir(&owner_only_dir).unwrap();
        fs::set_permissions(&owner_only_dir, fs::Permissions::from_mode(0o000)).unwrap();
        let owner_only_identity = parent
            .entry_identity(OsStr::new("owner-only-dir"))
            .unwrap()
            .unwrap();
        #[cfg(target_vendor = "apple")]
        {
            assert_eq!(
                parent
                    .ensure_dir_open(OsStr::new("owner-only-dir"), 0o755)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
            assert_eq!(path_mode(&owner_only_dir), 0o000);
        }
        #[cfg(not(target_vendor = "apple"))]
        {
            let repaired = parent
                .ensure_dir_open(OsStr::new("owner-only-dir"), 0o755)
                .unwrap();
            assert_eq!(capability_mode(&repaired), 0o755);
            assert_eq!(repaired.identity(), owner_only_identity);
        }
        assert_eq!(
            parent.entry_identity(OsStr::new("owner-only-dir")).unwrap(),
            Some(owner_only_identity)
        );

        let tainted_dir = root.path().join("tainted-dir");
        fs::create_dir(&tainted_dir).unwrap();
        fs::set_permissions(&tainted_dir, fs::Permissions::from_mode(0o777)).unwrap();
        let tainted_identity = parent
            .entry_identity(OsStr::new("tainted-dir"))
            .unwrap()
            .unwrap();
        assert_eq!(
            parent
                .ensure_dir_open(OsStr::new("tainted-dir"), 0o700)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(path_mode(&tainted_dir), 0o777);
        assert_eq!(
            parent.entry_identity(OsStr::new("tainted-dir")).unwrap(),
            Some(tainted_identity)
        );

        let owner_only_file = root.path().join("owner-only.json");
        fs::write(&owner_only_file, b"trusted").unwrap();
        fs::set_permissions(&owner_only_file, fs::Permissions::from_mode(0o000)).unwrap();
        let owner_only_file_identity = parent
            .entry_identity(OsStr::new("owner-only.json"))
            .unwrap()
            .unwrap();
        assert_eq!(
            parent
                .read_file_limited(OsStr::new("owner-only.json"), 1024)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(path_mode(&owner_only_file), 0o000);
        assert_eq!(
            parent
                .entry_identity(OsStr::new("owner-only.json"))
                .unwrap(),
            Some(owner_only_file_identity)
        );

        let readable_file = root.path().join("readable.json");
        fs::write(&readable_file, b"read-only").unwrap();
        fs::set_permissions(&readable_file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            parent
                .read_file_limited(OsStr::new("readable.json"), 1024)
                .unwrap(),
            b"read-only"
        );
        assert_eq!(fs::read(&readable_file).unwrap(), b"read-only");
        assert_eq!(path_mode(&readable_file), 0o644);

        let tainted_file = root.path().join("tainted.json");
        fs::write(&tainted_file, b"untrusted").unwrap();
        fs::set_permissions(&tainted_file, fs::Permissions::from_mode(0o777)).unwrap();
        let tainted_file_identity = parent
            .entry_identity(OsStr::new("tainted.json"))
            .unwrap()
            .unwrap();
        assert_eq!(
            parent
                .read_file_limited(OsStr::new("tainted.json"), 1024)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(path_mode(&tainted_file), 0o777);
        assert_eq!(
            parent.entry_identity(OsStr::new("tainted.json")).unwrap(),
            Some(tainted_file_identity)
        );

        let wide_lock = root.path().join("wide.lock");
        fs::write(&wide_lock, []).unwrap();
        fs::set_permissions(&wide_lock, fs::Permissions::from_mode(0o777)).unwrap();
        let wide_lock_identity = parent
            .entry_identity(OsStr::new("wide.lock"))
            .unwrap()
            .unwrap();
        drop(parent.open_lock_file(OsStr::new("wide.lock")).unwrap());
        assert_eq!(path_mode(&wide_lock), 0o600);
        assert_eq!(
            parent.entry_identity(OsStr::new("wide.lock")).unwrap(),
            Some(wide_lock_identity)
        );

        let symlink_target = root.path().join("symlink-target");
        fs::create_dir(&symlink_target).unwrap();
        fs::set_permissions(&symlink_target, fs::Permissions::from_mode(0o777)).unwrap();
        std::os::unix::fs::symlink(&symlink_target, root.path().join("symlink-dir")).unwrap();
        assert!(parent
            .ensure_dir_open(OsStr::new("symlink-dir"), 0o700)
            .is_err());
        assert_eq!(path_mode(&symlink_target), 0o777);

        assert!(Command::new("mkfifo")
            .arg(root.path().join("special-dir"))
            .status()
            .unwrap()
            .success());
        assert_eq!(
            parent
                .ensure_dir_open(OsStr::new("special-dir"), 0o700)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        let wrong_device_dir = root.path().join("wrong-device-dir");
        fs::create_dir(&wrong_device_dir).unwrap();
        fs::set_permissions(&wrong_device_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let wrong_device_parent = CapabilityDir {
            fd: rustix::io::dup(&parent.fd).unwrap(),
            display_path: parent.display_path.clone(),
            identity: FileIdentity {
                device: parent.identity.device.wrapping_add(1),
                inode: parent.identity.inode,
            },
            acl_policy: parent.acl_policy,
        };
        assert_eq!(
            wrong_device_parent
                .ensure_dir_open(OsStr::new("wrong-device-dir"), 0o755)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(path_mode(&wrong_device_dir), 0o700);
    }

    #[test]
    fn read_only_admission_preserves_safe_and_inaccessible_file_modes() {
        use std::io::{Read as _, Seek as _, SeekFrom};

        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();

        let readable = root.path().join("readable.json");
        fs::write(&readable, b"readable").unwrap();
        fs::set_permissions(&readable, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            parent
                .read_file_limited(OsStr::new("readable.json"), 1024)
                .unwrap(),
            b"readable"
        );
        assert_eq!(path_mode(&readable), 0o644);

        let inaccessible = root.path().join("inaccessible.json");
        fs::write(&inaccessible, b"inaccessible").unwrap();
        let mut retained = fs::File::open(&inaccessible).unwrap();
        fs::set_permissions(&inaccessible, fs::Permissions::from_mode(0o000)).unwrap();

        let error = parent
            .read_file_limited(OsStr::new("inaccessible.json"), 1024)
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(path_mode(&inaccessible), 0o000);
        retained.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        retained.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"inaccessible");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn read_only_admission_preserves_linux_acl_mode_and_bytes_on_failure() {
        use std::io::{Read as _, Seek as _, SeekFrom};

        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let path = root.path().join("acl-inaccessible.json");
        fs::write(&path, b"acl-protected").unwrap();
        let mut retained = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        set_test_access_acl(&retained);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let acl_before = test_access_acl_bytes(&retained);
        assert!(has_extended_acl(&retained).unwrap());

        let error = parent
            .read_file_limited(OsStr::new("acl-inaccessible.json"), 1024)
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(path_mode(&path), 0o000);
        assert_eq!(test_access_acl_bytes(&retained), acl_before);
        retained.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        retained.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"acl-protected");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn append_and_lock_admission_preserve_linux_acl_mode_and_bytes_on_failure() {
        use std::io::{Read as _, Seek as _, SeekFrom};

        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        for (name, append) in [("acl-append.log", true), ("acl-lock.lock", false)] {
            let path = root.path().join(name);
            fs::write(&path, b"acl-protected").unwrap();
            let mut retained = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            set_test_access_acl(&retained);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
            let acl_before = test_access_acl_bytes(&retained);

            let error = if append {
                parent.open_append_file(OsStr::new(name)).unwrap_err()
            } else {
                parent.open_lock_file(OsStr::new(name)).unwrap_err()
            };

            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(path_mode(&path), 0o000);
            assert_eq!(test_access_acl_bytes(&retained), acl_before);
            retained.seek(SeekFrom::Start(0)).unwrap();
            let mut bytes = Vec::new();
            retained.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"acl-protected");
        }
    }

    #[test]
    fn namespace_ancestor_validation_precedes_state_and_lock_mutation() {
        if let Some(case) = std::env::var_os(NAMESPACE_CASE_ENV) {
            let workspace = PathBuf::from(std::env::var_os(NAMESPACE_ROOT_ENV).unwrap());
            match case.to_str().unwrap() {
                "insecure-grandparent" => {
                    assert_eq!(
                        CapabilityDir::open(&workspace).unwrap_err().kind(),
                        io::ErrorKind::PermissionDenied
                    );
                    assert!(!workspace.join(".packet28").exists());
                }
                "secure" | "sticky-grandparent" => {
                    let root = CapabilityDir::open(&workspace).unwrap();
                    let state = root
                        .ensure_dir_open(OsStr::new(".packet28"), 0o755)
                        .unwrap();
                    let daemon = state.ensure_dir_open(OsStr::new("daemon"), 0o755).unwrap();
                    drop(daemon.open_lock_file(OsStr::new("instance.lock")).unwrap());
                }
                other => panic!("unknown namespace validation case: {other}"),
            }
            return;
        }

        let outer = tempdir().unwrap();
        for (case, grandparent_mode, expect_state) in [
            ("insecure-grandparent", 0o777, false),
            ("sticky-grandparent", 0o1777, true),
        ] {
            let grandparent = outer.path().join(case);
            let secure_parent = grandparent.join("secure-parent");
            let workspace = secure_parent.join("workspace");
            fs::create_dir_all(&workspace).unwrap();
            fs::set_permissions(&grandparent, fs::Permissions::from_mode(grandparent_mode))
                .unwrap();
            fs::set_permissions(&secure_parent, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700)).unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg(
                    "capability::tests::namespace_ancestor_validation_precedes_state_and_lock_mutation",
                )
                .arg("--nocapture")
                .arg("--test-threads=1")
                .env(NAMESPACE_CASE_ENV, case)
                .env(NAMESPACE_ROOT_ENV, &workspace)
                .status()
                .unwrap();
            assert!(status.success());
            assert_eq!(workspace.join(".packet28").exists(), expect_state);
        }

        let secure_parent = outer.path().join("secure");
        let workspace = secure_parent.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::set_permissions(&secure_parent, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700)).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(
                "capability::tests::namespace_ancestor_validation_precedes_state_and_lock_mutation",
            )
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(NAMESPACE_CASE_ENV, "secure")
            .env(NAMESPACE_ROOT_ENV, &workspace)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(workspace.join(".packet28/daemon/instance.lock").is_file());
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn inherited_extended_acls_are_stripped_before_publication() {
        for (name, mode) in [
            (".packet28", 0o755),
            (".retention-trash", 0o700),
            ("quarantine-group", 0o700),
        ] {
            let root = tempdir().unwrap();
            let parent = CapabilityDir::open_workspace(root.path()).unwrap();
            inject_inheritable_acl_before_create_once(OsStr::new(name));
            let directory = parent.ensure_dir_open(OsStr::new(name), mode).unwrap();
            assert!(!has_extended_acl(&directory.fd).unwrap());
            assert_eq!(capability_mode(&directory), mode);
        }

        for (name, prefix) in [
            ("active-task", ACTIVE_TASK_WRITE_TEMP_PREFIX),
            ("tasks.json", TASK_REGISTRY_WRITE_TEMP_PREFIX),
            ("journal-v1.json", RETENTION_JOURNAL_WRITE_TEMP_PREFIX),
        ] {
            let root = tempdir().unwrap();
            let parent = CapabilityDir::open_workspace(root.path()).unwrap();
            inject_inheritable_acl_before_create_once(OsStr::new(name));
            parent
                .write_json_atomically(OsStr::new(name), b"{}", prefix)
                .unwrap();
            let fd = rfs::openat(
                &parent.fd,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .unwrap();
            assert!(!has_extended_acl(&fd).unwrap());
            assert_eq!((rfs::fstat(&fd).unwrap().st_mode as RawMode) & 0o777, 0o600);
        }

        for name in ["instance.lock", "lifecycle.lock", "registry.lock"] {
            let root = tempdir().unwrap();
            let parent = CapabilityDir::open_workspace(root.path()).unwrap();
            inject_inheritable_acl_before_create_once(OsStr::new(name));
            let lock = parent.open_lock_file(OsStr::new(name)).unwrap();
            assert!(!has_extended_acl(&lock).unwrap());
            assert_eq!(
                (rfs::fstat(&lock).unwrap().st_mode as RawMode) & 0o777,
                0o600
            );
        }
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn protective_deny_acl_does_not_add_namespace_authority() {
        let root = tempdir().unwrap();
        assert!(Command::new("chmod")
            .arg("+a")
            .arg("everyone deny delete")
            .arg(root.path())
            .status()
            .unwrap()
            .success());

        assert_eq!(
            CapabilityDir::open(root.path()).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let capability = CapabilityDir::open_workspace(root.path()).unwrap();

        assert!(!has_namespace_authority_acl(&capability.fd).unwrap());
        assert!(has_extended_acl(&capability.fd).unwrap());
        capability
            .ensure_dir_open(OsStr::new("managed"), 0o700)
            .unwrap();
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn existing_acl_taint_is_persistent_for_authority_but_repairable_for_locks() {
        let add_acl = |path: &Path| {
            assert!(Command::new("chmod")
                .arg("+a")
                .arg("everyone allow read,write,delete,append,readattr,writeattr")
                .arg(path)
                .status()
                .unwrap()
                .success());
        };
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();

        let authority_dir = root.path().join("authority-dir");
        fs::create_dir(&authority_dir).unwrap();
        fs::set_permissions(&authority_dir, fs::Permissions::from_mode(0o700)).unwrap();
        add_acl(&authority_dir);
        assert_eq!(
            parent
                .ensure_dir_open(OsStr::new("authority-dir"), 0o700)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        let authority_fd = rfs::open(&authority_dir, DIRECTORY_FLAGS, Mode::empty()).unwrap();
        assert!(has_extended_acl(&authority_fd).unwrap());

        let managed_file = root.path().join("managed.json");
        fs::write(&managed_file, b"untrusted").unwrap();
        fs::set_permissions(&managed_file, fs::Permissions::from_mode(0o600)).unwrap();
        add_acl(&managed_file);
        assert_eq!(
            parent
                .read_file_limited(OsStr::new("managed.json"), 1024)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        let managed_fd = rfs::open(
            &managed_file,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .unwrap();
        assert!(has_extended_acl(&managed_fd).unwrap());

        let lock_path = root.path().join("repairable.lock");
        fs::write(&lock_path, []).unwrap();
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).unwrap();
        let lock_identity = parent
            .entry_identity(OsStr::new("repairable.lock"))
            .unwrap()
            .unwrap();
        add_acl(&lock_path);
        let lock = parent
            .open_lock_file(OsStr::new("repairable.lock"))
            .unwrap();
        assert!(!has_extended_acl(&lock).unwrap());
        assert_eq!(
            parent
                .entry_identity(OsStr::new("repairable.lock"))
                .unwrap(),
            Some(lock_identity)
        );
    }

    #[test]
    fn initializer_sigkill_residue_is_isolated_from_publication() {
        if let Some(case) = std::env::var_os(INITIALIZER_KILL_CASE_ENV) {
            let root = PathBuf::from(std::env::var_os(INITIALIZER_KILL_ROOT_ENV).unwrap());
            let parent = CapabilityDir::open(&root).unwrap();
            let _umask = UmaskGuard::set(0o777);
            match case.to_str().unwrap() {
                "directory-before-mode" => {
                    kill_directory_initializer_once(
                        OsStr::new(".packet28"),
                        DirectoryInitializerKillPoint::BeforeModeCorrection,
                    );
                    parent
                        .ensure_dir_open(OsStr::new(".packet28"), 0o755)
                        .unwrap();
                }
                "directory-after-mode" => {
                    kill_directory_initializer_once(
                        OsStr::new(".packet28"),
                        DirectoryInitializerKillPoint::AfterModeCorrection,
                    );
                    parent
                        .ensure_dir_open(OsStr::new(".packet28"), 0o755)
                        .unwrap();
                }
                "lock-before-mode" => {
                    kill_lock_initializer_once(
                        OsStr::new("lifecycle.lock"),
                        LockInitializerKillPoint::BeforeModeCorrection,
                    );
                    parent.open_lock_file(OsStr::new("lifecycle.lock")).unwrap();
                }
                "lock-after-mode" => {
                    kill_lock_initializer_once(
                        OsStr::new("lifecycle.lock"),
                        LockInitializerKillPoint::AfterModeCorrection,
                    );
                    parent.open_lock_file(OsStr::new("lifecycle.lock")).unwrap();
                }
                other => panic!("unknown initializer kill case: {other}"),
            }
            panic!("initializer killpoint did not terminate its process");
        }

        for case in ["directory-before-mode", "directory-after-mode"] {
            let root = tempdir().unwrap();
            run_initializer_kill_child(root.path(), case);
            let parent = CapabilityDir::open(root.path()).unwrap();
            assert_eq!(
                parent.entry_identity(OsStr::new(".packet28")).unwrap(),
                None
            );
            let residue = parent
                .entries()
                .unwrap()
                .into_iter()
                .find(|entry| directory_initializer_name_matches(entry))
                .expect("killed directory initializer must remain strictly identifiable");
            let residue_identity = parent.entry_identity(&residue).unwrap().unwrap();

            let state = parent
                .ensure_dir_open(OsStr::new(".packet28"), 0o755)
                .unwrap();

            assert_eq!(capability_mode(&state), 0o755);
            assert_eq!(
                parent.entry_identity(&residue).unwrap(),
                Some(residue_identity),
                "creation before lifecycle exclusion must not sweep a potentially live initializer"
            );
        }

        for case in ["lock-before-mode", "lock-after-mode"] {
            let root = tempdir().unwrap();
            run_initializer_kill_child(root.path(), case);
            let parent = CapabilityDir::open(root.path()).unwrap();
            assert_eq!(
                parent.entry_identity(OsStr::new("lifecycle.lock")).unwrap(),
                None
            );
            let prefix = lock_initializer_prefix(OsStr::new("lifecycle.lock"));
            let deletion_prefix = generated_deletion_prefix(&prefix);
            let residue = parent
                .entries()
                .unwrap()
                .into_iter()
                .find(|entry| {
                    generated_name_matches(entry, &prefix)
                        || generated_name_matches(entry, deletion_prefix.as_ref())
                })
                .expect("killed lock initializer must remain strictly identifiable");
            let residue_identity = parent.entry_identity(&residue).unwrap().unwrap();

            drop(parent.open_lock_file(OsStr::new("lifecycle.lock")).unwrap());

            assert_eq!(path_mode(&root.path().join("lifecycle.lock")), 0o600);
            assert_eq!(
                parent.entry_identity(&residue).unwrap(),
                Some(residue_identity),
                "opening before lock exclusion must not sweep a potentially live initializer"
            );
        }
    }

    #[test]
    fn directory_initializers_are_not_swept_without_exclusion() {
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let stale = root.path().join(".directory-init-0123456789abcdef-12-34");
        let lookalike = root
            .path()
            .join(".directory-init-0123456789abcdef-12-34-extra");
        fs::create_dir(&stale).unwrap();
        fs::create_dir(&lookalike).unwrap();
        fs::set_permissions(&stale, fs::Permissions::from_mode(0o000)).unwrap();

        drop(
            parent
                .ensure_dir_open(OsStr::new("published"), 0o755)
                .unwrap(),
        );

        assert!(stale.is_dir());
        assert!(lookalike.is_dir());
    }

    #[test]
    fn permission_or_sync_failure_cleans_new_entries() {
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();

        inject_new_entry_chmod_failure_once(OsStr::new("chmod-directory"));
        assert!(parent
            .create_dir(OsStr::new("chmod-directory"), 0o700)
            .is_err());
        assert_eq!(
            parent
                .entry_identity(OsStr::new("chmod-directory"))
                .unwrap(),
            None
        );

        inject_directory_create_sync_failure_once(OsStr::new("sync-directory"));
        assert!(parent
            .create_dir(OsStr::new("sync-directory"), 0o700)
            .is_err());
        assert_eq!(
            parent.entry_identity(OsStr::new("sync-directory")).unwrap(),
            None
        );

        let atomic_result = parent.write_json_atomically_with_observers(
            OsStr::new("state.json"),
            b"{}",
            TEST_ATOMIC_WRITE_TEMP_PREFIX,
            |temporary| {
                inject_new_entry_chmod_failure_once(temporary);
                Ok(())
            },
            || Ok(()),
            || Ok(()),
        );
        assert!(atomic_result.is_err());
        assert_eq!(
            parent
                .entries()
                .unwrap()
                .into_iter()
                .filter(|name| generated_name_matches(name, TEST_ATOMIC_WRITE_TEMP_PREFIX))
                .count(),
            0
        );

        inject_new_entry_chmod_failure_once(OsStr::new("new.lock"));
        assert!(parent.open_lock_file(OsStr::new("new.lock")).is_err());
        assert_eq!(parent.entry_identity(OsStr::new("new.lock")).unwrap(), None);
    }

    #[test]
    fn generated_temp_cleanup_recovers_only_strict_target_specific_tombstones() {
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let deletion_prefix = generated_deletion_prefix(TEST_ATOMIC_WRITE_TEMP_PREFIX);
        let first_source = OsString::from(format!("{TEST_ATOMIC_WRITE_TEMP_PREFIX}-4242-7"));
        fs::write(root.path().join(&first_source), b"first").unwrap();
        let first_identity = parent.entry_identity(&first_source).unwrap().unwrap();
        let stale_tombstone = parent
            .tombstone_entry_verified(&first_source, first_identity, deletion_prefix.as_ref())
            .unwrap();
        assert!(generated_name_matches(
            &stale_tombstone,
            TEST_ATOMIC_WRITE_DELETION_TEMP_PREFIX
        ));
        assert!(!generated_name_matches(
            &stale_tombstone,
            DELETION_TEMP_PREFIX
        ));

        let second_source = OsString::from(format!("{TEST_ATOMIC_WRITE_TEMP_PREFIX}-4242-8"));
        fs::write(root.path().join(&second_source), b"second").unwrap();
        let lookalikes = [
            format!("{TEST_ATOMIC_WRITE_DELETION_TEMP_PREFIX}-not-a-pid-1"),
            format!("{TEST_ATOMIC_WRITE_DELETION_TEMP_PREFIX}-123-1-extra"),
            format!("{TEST_ATOMIC_WRITE_DELETION_TEMP_PREFIX}-123"),
            format!("{DELETION_TEMP_PREFIX}-123-1"),
        ];
        for lookalike in &lookalikes {
            fs::write(root.path().join(lookalike), b"keep").unwrap();
        }

        let removed = parent
            .remove_generated_regular_files(TEST_ATOMIC_WRITE_TEMP_PREFIX)
            .unwrap();

        assert_eq!(removed, 2);
        assert_eq!(parent.entry_identity(&second_source).unwrap(), None);
        assert_eq!(parent.entry_identity(&stale_tombstone).unwrap(), None);
        for lookalike in lookalikes {
            assert_eq!(
                fs::read(root.path().join(lookalike)).unwrap(),
                b"keep",
                "lookalike must not be treated as an owned temporary"
            );
        }
    }

    #[test]
    fn generated_temp_cleanup_rejects_hardlinks_and_special_files() {
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let original = root.path().join("original");
        fs::write(&original, b"keep").unwrap();
        let hardlink = OsString::from(format!("{TEST_ATOMIC_WRITE_TEMP_PREFIX}-4242-11"));
        fs::hard_link(&original, root.path().join(&hardlink)).unwrap();

        assert_eq!(
            parent
                .remove_generated_regular_files(TEST_ATOMIC_WRITE_TEMP_PREFIX)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read(&original).unwrap(), b"keep");
        assert_eq!(fs::read(root.path().join(&hardlink)).unwrap(), b"keep");

        fs::remove_file(root.path().join(&hardlink)).unwrap();
        let fifo = OsString::from(format!("{TEST_ATOMIC_WRITE_TEMP_PREFIX}-4242-12"));
        assert!(Command::new("mkfifo")
            .arg(root.path().join(&fifo))
            .status()
            .unwrap()
            .success());
        assert_eq!(
            parent
                .remove_generated_regular_files(TEST_ATOMIC_WRITE_TEMP_PREFIX)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            parent.entry_is_regular_file(&fifo).unwrap(),
            Some(false),
            "cleanup must leave a strict-name special file untouched"
        );
    }

    #[test]
    fn generated_name_grammar_accepts_legacy_and_nonce_forms_only() {
        let prefix = ".generated";
        assert_eq!(
            generated_deletion_prefix(ACTIVE_TASK_WRITE_TEMP_PREFIX),
            ACTIVE_TASK_WRITE_DELETION_TEMP_PREFIX
        );
        assert!(generated_name_matches(
            OsStr::new(".generated-12-34"),
            prefix
        ));
        assert!(generated_name_matches(
            OsStr::new(".generated-12-34-0123456789abcdef0123456789abcdef"),
            prefix
        ));
        for lookalike in [
            ".generated-12-34-short",
            ".generated-12-34-0123456789ABCDEF0123456789ABCDEF",
            ".generated-12-34-0123456789abcdef0123456789abcdef-extra",
            ".generated-12-not-a-counter",
            ".generated--34",
        ] {
            assert!(!generated_name_matches(OsStr::new(lookalike), prefix));
        }
        assert!(directory_initializer_name_matches(OsStr::new(
            ".directory-init-0123456789abcdef-12-34"
        )));
        assert!(directory_initializer_name_matches(OsStr::new(
            ".directory-init-0123456789abcdef-12-34-0123456789abcdef0123456789abcdef"
        )));
        for lookalike in [
            ".directory-init-0123456789abcde-12-34",
            ".directory-init-0123456789ABCDEF-12-34",
            ".directory-init-0123456789abcdef-12-34-short",
            ".directory-init-0123456789abcdef-not-a-pid-34",
        ] {
            assert!(!directory_initializer_name_matches(OsStr::new(lookalike)));
        }
        let lock_name = OsStr::new(".hook-spool-recovery-v2.lock");
        let initializer = OsString::from(format!(
            "{}-12-34-0123456789abcdef0123456789abcdef",
            lock_initializer_prefix(lock_name)
        ));
        assert!(lock_initializer_name_matches(&initializer, lock_name));
        assert!(!lock_initializer_name_matches(
            &initializer,
            OsStr::new(".hook-spool-quota-v2.lock")
        ));
        assert!(!lock_initializer_name_matches(
            OsStr::new(".lock-init-0123456789abcdef-12-34-lookalike"),
            lock_name
        ));
    }

    #[test]
    fn generated_name_collisions_retry_without_deleting_colliders() {
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();

        fs::write(root.path().join("source"), b"source").unwrap();
        fs::write(root.path().join("tombstone-collision"), b"keep").unwrap();
        let source_identity = parent
            .entry_identity(OsStr::new("source"))
            .unwrap()
            .unwrap();
        inject_unique_names([
            OsString::from("tombstone-collision"),
            OsString::from(".deleting-1-2-0123456789abcdef0123456789abcdef"),
        ]);
        parent
            .remove_tree_entry_verified(OsStr::new("source"), source_identity)
            .unwrap();
        assert_eq!(
            fs::read(root.path().join("tombstone-collision")).unwrap(),
            b"keep"
        );

        let legitimate_directory = root.path().join("directory-collision");
        fs::create_dir(&legitimate_directory).unwrap();
        fs::write(legitimate_directory.join("keep"), b"keep").unwrap();
        let directory_prefix = directory_initializer_prefix(OsStr::new("group"));
        inject_unique_names([
            OsString::from("directory-collision"),
            OsString::from(format!(
                "{directory_prefix}-1-3-0123456789abcdef0123456789abcdef"
            )),
        ]);
        drop(parent.create_dir(OsStr::new("group"), 0o700).unwrap());
        assert_eq!(
            fs::read(legitimate_directory.join("keep")).unwrap(),
            b"keep"
        );

        fs::write(root.path().join("atomic-collision"), b"keep").unwrap();
        inject_unique_names([
            OsString::from("atomic-collision"),
            OsString::from(format!(
                "{TEST_ATOMIC_WRITE_TEMP_PREFIX}-1-4-0123456789abcdef0123456789abcdef"
            )),
        ]);
        parent
            .write_json_atomically(
                OsStr::new("state.json"),
                b"new",
                TEST_ATOMIC_WRITE_TEMP_PREFIX,
            )
            .unwrap();
        assert_eq!(
            fs::read(root.path().join("atomic-collision")).unwrap(),
            b"keep"
        );
        assert_eq!(fs::read(root.path().join("state.json")).unwrap(), b"new");

        fs::write(root.path().join("probe-source-collision"), b"source").unwrap();
        fs::write(
            root.path().join("probe-destination-collision"),
            b"destination",
        )
        .unwrap();
        inject_unique_names([
            OsString::from("probe-source-collision"),
            OsString::from(".noreplace-probe-source-1-5-0123456789abcdef0123456789abcdef"),
            OsString::from("probe-destination-collision"),
            OsString::from(".noreplace-probe-destination-1-6-0123456789abcdef0123456789abcdef"),
            OsString::from(".deleting-1-7-0123456789abcdef0123456789abcdef"),
        ]);
        parent.probe_noreplace_rename().unwrap();
        assert_eq!(
            fs::read(root.path().join("probe-source-collision")).unwrap(),
            b"source"
        );
        assert_eq!(
            fs::read(root.path().join("probe-destination-collision")).unwrap(),
            b"destination"
        );
    }

    #[test]
    fn generated_name_retry_exhaustion_is_bounded_and_preserves_colliders() {
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let repeated =
            |name: &'static str| (0..MAX_UNIQUE_NAME_ATTEMPTS).map(move |_| OsString::from(name));

        fs::write(root.path().join("victim"), b"victim").unwrap();
        fs::write(root.path().join("tombstone-collider"), b"keep").unwrap();
        let victim_identity = parent
            .entry_identity(OsStr::new("victim"))
            .unwrap()
            .unwrap();
        inject_unique_names(repeated("tombstone-collider"));
        assert_eq!(
            parent
                .tombstone_entry_verified(
                    OsStr::new("victim"),
                    victim_identity,
                    DELETION_TEMP_PREFIX,
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(root.path().join("victim")).unwrap(), b"victim");
        assert_eq!(
            fs::read(root.path().join("tombstone-collider")).unwrap(),
            b"keep"
        );

        let directory_collider = root.path().join("directory-collider");
        fs::create_dir(&directory_collider).unwrap();
        fs::write(directory_collider.join("keep"), b"keep").unwrap();
        inject_unique_names(repeated("directory-collider"));
        assert_eq!(
            parent
                .create_dir(OsStr::new("never-published"), 0o700)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(directory_collider.join("keep")).unwrap(), b"keep");
        assert_eq!(
            parent
                .entry_identity(OsStr::new("never-published"))
                .unwrap(),
            None
        );

        fs::write(root.path().join("atomic-collider"), b"keep").unwrap();
        inject_unique_names(repeated("atomic-collider"));
        let atomic_error = parent
            .write_json_atomically(
                OsStr::new("never-written.json"),
                b"new",
                TEST_ATOMIC_WRITE_TEMP_PREFIX,
            )
            .unwrap_err();
        assert!(!atomic_error.renamed);
        assert_eq!(atomic_error.source.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(root.path().join("atomic-collider")).unwrap(),
            b"keep"
        );

        fs::write(root.path().join("lock-collider"), b"keep").unwrap();
        inject_unique_names(repeated("lock-collider"));
        assert_eq!(
            parent
                .open_lock_file(OsStr::new("never-created.lock"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            fs::read(root.path().join("lock-collider")).unwrap(),
            b"keep"
        );

        fs::write(root.path().join("probe-source-collider"), b"keep").unwrap();
        inject_unique_names(repeated("probe-source-collider"));
        assert_eq!(
            parent.probe_noreplace_rename().unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            fs::read(root.path().join("probe-source-collider")).unwrap(),
            b"keep"
        );

        fs::write(root.path().join("probe-destination-collider"), b"keep").unwrap();
        let source =
            OsString::from(".noreplace-probe-source-1-99-0123456789abcdef0123456789abcdef");
        let cleanup = OsString::from(
            ".noreplace-probe-source-deleting-1-100-0123456789abcdef0123456789abcdef",
        );
        inject_unique_names(
            std::iter::once(source.clone())
                .chain(repeated("probe-destination-collider"))
                .chain(std::iter::once(cleanup)),
        );
        assert_eq!(
            parent.probe_noreplace_rename().unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            fs::read(root.path().join("probe-destination-collider")).unwrap(),
            b"keep"
        );
        assert_eq!(parent.entry_identity(&source).unwrap(), None);
    }

    #[test]
    fn bounded_enumeration_accepts_exact_limit_and_rejects_one_over() {
        let empty_root = tempdir().unwrap();
        let empty = CapabilityDir::open(empty_root.path()).unwrap();
        assert!(
            empty.name_max().unwrap() >= 255,
            "test filesystem must support Packet28's 255-byte component contract"
        );
        assert!(!empty.has_entries().unwrap());
        assert!(empty.entries_bounded(0).unwrap().is_empty());

        let root = tempdir().unwrap();
        let directory = CapabilityDir::open(root.path()).unwrap();
        fs::write(root.path().join("a"), b"a").unwrap();
        fs::write(root.path().join("b"), b"b").unwrap();

        assert!(directory.has_entries().unwrap());
        assert_eq!(directory.entries_bounded(2).unwrap().len(), 2);
        assert_eq!(
            directory.entries_bounded(1).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn final_deletion_observer_reuse_returns_an_error() {
        let mut observer = Some(|| Ok(()));
        run_final_deletion_observer(&mut observer).unwrap();

        assert_eq!(
            run_final_deletion_observer(&mut observer)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn recursive_measurement_is_bounded_and_deletion_resumes_in_batches() {
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let measured = parent.create_dir(OsStr::new("measured"), 0o700).unwrap();
        fs::write(measured.display_path().join("a"), b"a").unwrap();
        let nested = measured.create_dir(OsStr::new("nested"), 0o700).unwrap();
        fs::write(nested.display_path().join("b"), b"bb").unwrap();
        let measured_identity = measured.identity();
        drop(nested);
        drop(measured);

        assert_eq!(
            parent
                .entry_logical_bytes_verified_with_limits(
                    OsStr::new("measured"),
                    measured_identity,
                    TraversalLimits {
                        max_depth: 2,
                        max_entries: 4,
                    },
                )
                .unwrap(),
            3
        );
        assert_eq!(
            parent
                .entry_logical_bytes_verified_with_limits(
                    OsStr::new("measured"),
                    measured_identity,
                    TraversalLimits {
                        max_depth: 2,
                        max_entries: 3,
                    },
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            parent
                .entry_logical_bytes_verified_with_limits(
                    OsStr::new("measured"),
                    measured_identity,
                    TraversalLimits {
                        max_depth: 1,
                        max_entries: 4,
                    },
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        let exact = parent
            .create_dir(OsStr::new("remove-exact"), 0o700)
            .unwrap();
        fs::write(exact.display_path().join("a"), b"a").unwrap();
        fs::write(exact.display_path().join("b"), b"b").unwrap();
        let exact_identity = exact.identity();
        drop(exact);
        assert_eq!(
            parent
                .remove_tombstone_verified_batch_with_limit(
                    OsStr::new("remove-exact"),
                    exact_identity,
                    2,
                )
                .unwrap(),
            RemovalProgress::Complete
        );
        assert_eq!(
            parent.entry_identity(OsStr::new("remove-exact")).unwrap(),
            None
        );

        let over = parent.create_dir(OsStr::new("remove-over"), 0o700).unwrap();
        fs::write(over.display_path().join("a"), b"a").unwrap();
        fs::write(over.display_path().join("b"), b"b").unwrap();
        let over_identity = over.identity();
        drop(over);
        assert_eq!(
            parent
                .remove_tombstone_verified_batch_with_limit(
                    OsStr::new("remove-over"),
                    over_identity,
                    1,
                )
                .unwrap(),
            RemovalProgress::More
        );
        assert_eq!(
            parent.entry_identity(OsStr::new("remove-over")).unwrap(),
            Some(over_identity)
        );
        assert_eq!(
            parent
                .remove_tombstone_verified_batch_with_limit(
                    OsStr::new("remove-over"),
                    over_identity,
                    1,
                )
                .unwrap(),
            RemovalProgress::Complete
        );

        let wide = parent.create_dir(OsStr::new("remove-wide"), 0o700).unwrap();
        for index in 0..11 {
            fs::write(wide.display_path().join(format!("file-{index:02}")), b"x").unwrap();
        }
        let wide_identity = wide.identity();
        drop(wide);
        let mut wide_more = 0;
        while let RemovalProgress::More = parent
            .remove_tombstone_verified_batch_with_limit(OsStr::new("remove-wide"), wide_identity, 2)
            .unwrap()
        {
            wide_more += 1;
        }
        assert!(wide_more >= 5);
        assert_eq!(
            parent.entry_identity(OsStr::new("remove-wide")).unwrap(),
            None
        );

        let deep = parent.create_dir(OsStr::new("remove-deep"), 0o700).unwrap();
        let mut deepest = deep.display_path().to_path_buf();
        for index in 0..96 {
            deepest.push(format!("level-{index:02}"));
            fs::create_dir(&deepest).unwrap();
        }
        fs::write(deepest.join("leaf"), b"x").unwrap();
        let deep_identity = deep.identity();
        drop(deep);
        let mut deep_more = 0;
        while let RemovalProgress::More = parent
            .remove_tombstone_verified_batch_with_limit(OsStr::new("remove-deep"), deep_identity, 1)
            .unwrap()
        {
            deep_more += 1;
            assert!(deep_more < 512, "deep deletion did not make progress");
        }
        assert!(deep_more > MAX_CAPABILITY_RECURSION_DEPTH);
        assert_eq!(
            parent.entry_identity(OsStr::new("remove-deep")).unwrap(),
            None
        );
    }

    #[test]
    fn fifo_entries_are_rejected_without_blocking() {
        if !enter_isolated_fifo_case(
            "capability::tests::fifo_entries_are_rejected_without_blocking",
        ) {
            return;
        }
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        for name in ["registry.json", "retention-journal.json", "registry.lock"] {
            assert!(Command::new("mkfifo")
                .arg(root.path().join(name))
                .status()
                .unwrap()
                .success());
        }

        for name in ["registry.json", "retention-journal.json"] {
            assert_eq!(
                parent
                    .read_file_limited(OsStr::new(name), 1024)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        assert_eq!(
            parent
                .open_existing_lock_file(OsStr::new("registry.lock"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        fs::write(root.path().join("racy-registry.json"), b"{}").unwrap();
        inject_preflight_fifo_swap_once(OsStr::new("racy-registry.json"));
        assert_eq!(
            parent
                .read_file_limited(OsStr::new("racy-registry.json"), 1024)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        fs::write(root.path().join("racy.lock"), []).unwrap();
        inject_preflight_fifo_swap_once(OsStr::new("racy.lock"));
        assert_eq!(
            parent
                .open_existing_lock_file(OsStr::new("racy.lock"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn authenticated_reader_rejects_symlink_substitution_after_open_without_following() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let name = OsStr::new("authority.json");
        let authority = root.path().join(name);
        fs::write(&authority, b"trusted").unwrap();
        fs::set_permissions(&authority, fs::Permissions::from_mode(0o600)).unwrap();
        let outside_target = outside.path().join("outside.json");
        fs::write(&outside_target, b"outside").unwrap();
        let staged_symlink = root.path().join("staged-symlink");
        std::os::unix::fs::symlink(&outside_target, &staged_symlink).unwrap();
        let authority_for_hook = authority;
        inject_authenticated_read_after_open_once(name, move || {
            fs::rename(staged_symlink, authority_for_hook).unwrap();
        });

        let error = parent.read_file_limited(name, 1024).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(outside_target).unwrap(), b"outside");
    }

    #[test]
    fn authenticated_reader_rejects_hard_link_substitution_after_open() {
        use std::os::unix::fs::MetadataExt as _;

        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        let name = OsStr::new("authority.json");
        let authority = root.path().join(name);
        fs::write(&authority, b"trusted").unwrap();
        fs::set_permissions(&authority, fs::Permissions::from_mode(0o600)).unwrap();
        let linked_source = root.path().join("linked-source.json");
        let staged_link = root.path().join("staged-link.json");
        fs::write(&linked_source, b"linked").unwrap();
        fs::set_permissions(&linked_source, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&linked_source, &staged_link).unwrap();
        let authority_for_hook = authority;
        inject_authenticated_read_after_open_once(name, move || {
            fs::rename(staged_link, authority_for_hook).unwrap();
        });

        let error = parent.read_file_limited(name, 1024).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::metadata(linked_source).unwrap().nlink(), 2);
    }

    #[test]
    fn authenticated_reader_retries_a_zero_link_entry_snapshot() {
        let root = tempdir().unwrap();
        let name = OsStr::new("authority.json");
        let authority = root.path().join(name);
        fs::write(&authority, b"trusted").unwrap();
        fs::set_permissions(&authority, fs::Permissions::from_mode(0o600)).unwrap();
        let file = fs::File::open(&authority).unwrap();
        fs::remove_file(&authority).unwrap();
        let stat = rfs::fstat(&file).unwrap();
        assert_eq!(stat.st_nlink, 0);

        let identity =
            authenticated_read_entry_identity_from_stat(&stat, name, stat.st_dev as u64).unwrap();

        assert_eq!(identity, None);
    }

    #[test]
    fn multiply_linked_authority_and_lock_files_are_rejected_before_mode_repair() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        for (name, alias) in [
            ("registry.json", "registry-alias.json"),
            ("registry.lock", "lock-alias"),
        ] {
            let path = root.path().join(name);
            let alias_path = root.path().join(alias);
            fs::write(&path, b"authority").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
            fs::hard_link(&path, &alias_path).unwrap();
            let mode_before = fs::metadata(&path).unwrap().mode() & 0o777;

            let error = if name.ends_with(".lock") {
                parent
                    .open_existing_lock_file(OsStr::new(name))
                    .unwrap_err()
            } else {
                parent
                    .read_file_limited(OsStr::new(name), 1024)
                    .unwrap_err()
            };

            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(fs::metadata(&path).unwrap().nlink(), 2);
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, mode_before);
            assert_eq!(
                fs::metadata(&alias_path).unwrap().mode() & 0o777,
                mode_before
            );
        }
    }

    #[test]
    fn post_rename_barrier_failures_are_classified_as_committed() {
        let root = tempdir().unwrap();
        let parent = CapabilityDir::open(root.path()).unwrap();
        for (name, failure) in [
            (
                "unsupported.json",
                InjectedAtomicAfterRenameFailure::Unsupported,
            ),
            ("io.json", InjectedAtomicAfterRenameFailure::Io),
        ] {
            inject_atomic_after_rename_barrier_failure_once(OsStr::new(name), failure);
            let error = parent
                .write_json_atomically(OsStr::new(name), b"new", TEST_ATOMIC_WRITE_TEMP_PREFIX)
                .unwrap_err();

            assert!(error.renamed);
            match failure {
                InjectedAtomicAfterRenameFailure::Unsupported => {
                    assert_eq!(error.source.kind(), io::ErrorKind::Unsupported);
                }
                InjectedAtomicAfterRenameFailure::Io => {
                    assert_eq!(error.source.raw_os_error(), Some(5));
                }
                InjectedAtomicAfterRenameFailure::Other => unreachable!(),
            }
            assert_eq!(fs::read(root.path().join(name)).unwrap(), b"new");
        }
    }
}
