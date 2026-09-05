//! Bounded, non-mutating filesystem measurements for retention.
//!
//! Path inspection and retained-capability revalidation share byte/count
//! aggregation and metadata fingerprints. Recursion and traversal budgets stay
//! private here; candidate selection and mutation authority belong to retention.

#[cfg(unix)]
use std::collections::BTreeSet;
#[cfg(unix)]
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

#[cfg(unix)]
use crate::capability::{CapabilityDir, CapabilityEntryKind, CapabilityEntryMetadata};
use crate::{DaemonCoreError, Result};

use super::{
    push_issue, retention_resource_limit_error, FileIdentity, TaskStoreIssue,
    MAX_RETENTION_MANAGED_ROOT_ENTRIES, MAX_RETENTION_SCAN_DEPTH,
    MAX_RETENTION_SCAN_ENTRIES_PER_TRAVERSAL,
};

#[derive(Debug, Clone, Copy)]
pub(super) struct ScanLimits {
    pub(super) max_depth: usize,
    pub(super) max_entries_per_traversal: usize,
    pub(super) max_entries_per_managed_root: usize,
}

impl ScanLimits {
    pub(super) const DEFAULT: Self = Self {
        max_depth: MAX_RETENTION_SCAN_DEPTH,
        max_entries_per_traversal: MAX_RETENTION_SCAN_ENTRIES_PER_TRAVERSAL,
        max_entries_per_managed_root: MAX_RETENTION_MANAGED_ROOT_ENTRIES,
    };
}

#[derive(Debug)]
struct ScanBudget {
    limits: ScanLimits,
    entries_seen: usize,
}

impl ScanBudget {
    const fn new(limits: ScanLimits) -> Self {
        Self {
            limits,
            entries_seen: 0,
        }
    }

    fn check_depth(&self, depth: usize, path: &Path) -> Result<()> {
        if depth <= self.limits.max_depth {
            return Ok(());
        }
        Err(retention_resource_limit_error(
            "task-store scan exceeded the supported directory-depth bound",
            path,
            format!(
                "maximum supported directory depth is {}",
                self.limits.max_depth
            ),
        ))
    }

    fn consume_entry(&mut self, path: &Path) -> Result<()> {
        if self.entries_seen < self.limits.max_entries_per_traversal {
            self.entries_seen += 1;
            return Ok(());
        }
        Err(retention_resource_limit_error(
            "task-store scan exceeded the supported entry bound",
            path,
            format!(
                "maximum supported entries per traversal is {}",
                self.limits.max_entries_per_traversal
            ),
        ))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ScanSummary {
    pub(super) logical_bytes: u64,
    pub(super) allocated_bytes: u64,
    pub(super) files: u64,
    pub(super) directories: u64,
    pub(super) symlinks: u64,
    pub(super) latest_timestamp_unix: Option<u64>,
    pub(super) metadata_fingerprint: [u8; 32],
    pub(super) safe: bool,
    pub(super) identity: Option<FileIdentity>,
    #[cfg(unix)]
    pub(super) physical_identities: BTreeSet<FileIdentity>,
}

pub(super) fn scan_path_with_limits(
    path: &Path,
    issues: &mut Vec<TaskStoreIssue>,
    issue_kind: &str,
    limits: ScanLimits,
) -> Result<ScanSummary> {
    scan_path_with_limits_from_parent(path, issues, issue_kind, limits, None)
}

pub(super) fn scan_path_with_limits_from_parent(
    path: &Path,
    issues: &mut Vec<TaskStoreIssue>,
    issue_kind: &str,
    limits: ScanLimits,
    parent_identity: Option<FileIdentity>,
) -> Result<ScanSummary> {
    let mut budget = ScanBudget::new(limits);
    scan_path_with_budget(path, issues, issue_kind, 0, parent_identity, &mut budget)
}

fn scan_path_with_budget(
    path: &Path,
    issues: &mut Vec<TaskStoreIssue>,
    issue_kind: &str,
    depth: usize,
    parent_identity: Option<FileIdentity>,
    budget: &mut ScanBudget,
) -> Result<ScanSummary> {
    budget.check_depth(depth, path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ScanSummary {
                safe: true,
                ..ScanSummary::default()
            });
        }
        Err(source) => {
            push_issue(
                issues,
                issue_kind,
                path,
                format!("failed to inspect entry: {source}"),
            );
            return Ok(ScanSummary::default());
        }
    };
    let mut summary = ScanSummary {
        allocated_bytes: filesystem_allocated_bytes(&metadata),
        latest_timestamp_unix: modified_unix(&metadata),
        safe: true,
        identity: Some(file_identity(&metadata)),
        #[cfg(unix)]
        physical_identities: BTreeSet::from([file_identity(&metadata)]),
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
    if parent_identity.is_some() && !same_device(parent_identity, summary.identity) {
        if file_type.is_symlink() {
            summary.logical_bytes = metadata.len();
            summary.symlinks = 1;
        } else if metadata.is_file() {
            summary.logical_bytes = metadata.len();
            summary.files = 1;
        } else if metadata.is_dir() {
            summary.directories = 1;
        } else {
            summary.logical_bytes = metadata.len();
        }
        summary.safe = false;
        push_issue(
            issues,
            "cross_device_entry",
            path,
            "entries on another filesystem are not traversed or eligible for retention".to_string(),
        );
        summary.metadata_fingerprint = *fingerprint.finalize().as_bytes();
        return Ok(summary);
    }
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
        return Ok(summary);
    }
    if metadata.is_file() {
        summary.logical_bytes = metadata.len();
        summary.files = 1;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;

            if metadata.nlink() > 1 {
                summary.safe = false;
                push_issue(
                    issues,
                    "hardlink_entry",
                    path,
                    "multiply-linked regular files are not eligible for retention".to_string(),
                );
            }
        }
        summary.metadata_fingerprint = *fingerprint.finalize().as_bytes();
        return Ok(summary);
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
        return Ok(summary);
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
            return Ok(summary);
        }
    };
    let mut entries = Vec::new();
    for entry in directory_entries {
        budget.consume_entry(path)?;
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
        let child = scan_path_with_budget(
            &entry.path(),
            issues,
            issue_kind,
            depth.saturating_add(1),
            summary.identity,
            budget,
        )?;
        let encoded_name = name.as_encoded_bytes();
        fingerprint.update(&(encoded_name.len() as u64).to_le_bytes());
        fingerprint.update(encoded_name);
        fingerprint.update(&child.metadata_fingerprint);
        merge_scan_summary(&mut summary, &child);
    }
    summary.metadata_fingerprint = *fingerprint.finalize().as_bytes();
    Ok(summary)
}

#[cfg(unix)]
pub(super) fn scan_capability_directory_with_limits(
    directory: &CapabilityDir,
    issues: &mut Vec<TaskStoreIssue>,
    issue_kind: &str,
    limits: ScanLimits,
) -> Result<ScanSummary> {
    let mut budget = ScanBudget::new(limits);
    scan_capability_directory_with_budget(directory, issues, issue_kind, 0, &mut budget)
}

#[cfg(unix)]
fn scan_capability_directory_with_budget(
    directory: &CapabilityDir,
    issues: &mut Vec<TaskStoreIssue>,
    issue_kind: &str,
    depth: usize,
    budget: &mut ScanBudget,
) -> Result<ScanSummary> {
    budget.check_depth(depth, directory.display_path())?;
    let metadata = directory.metadata().map_err(|source| {
        DaemonCoreError::io(
            "failed to inspect retained capability directory",
            directory.display_path(),
            source,
        )
    })?;
    let mut summary = scan_summary_from_capability_metadata(metadata);
    summary.directories = 1;
    let mut fingerprint = capability_metadata_hasher(metadata);
    let entries = directory
        .entries_bounded(budget.limits.max_entries_per_traversal)
        .map_err(|source| {
            DaemonCoreError::io(
                "failed to enumerate retained capability directory",
                directory.display_path(),
                source,
            )
        })?;
    for name in entries {
        budget.consume_entry(directory.display_path())?;
        let child = scan_capability_entry_with_budget(
            directory,
            &name,
            issues,
            issue_kind,
            depth.saturating_add(1),
            budget,
        )?;
        let encoded_name = name.as_encoded_bytes();
        fingerprint.update(&(encoded_name.len() as u64).to_le_bytes());
        fingerprint.update(encoded_name);
        fingerprint.update(&child.metadata_fingerprint);
        merge_scan_summary(&mut summary, &child);
    }
    summary.metadata_fingerprint = *fingerprint.finalize().as_bytes();
    Ok(summary)
}

#[cfg(unix)]
pub(super) fn scan_capability_entry_with_limits(
    parent: &CapabilityDir,
    name: &OsStr,
    issues: &mut Vec<TaskStoreIssue>,
    issue_kind: &str,
    limits: ScanLimits,
) -> Result<ScanSummary> {
    let mut budget = ScanBudget::new(limits);
    budget.consume_entry(parent.display_path())?;
    scan_capability_entry_with_budget(parent, name, issues, issue_kind, 0, &mut budget)
}

#[cfg(unix)]
fn scan_capability_entry_with_budget(
    parent: &CapabilityDir,
    name: &OsStr,
    issues: &mut Vec<TaskStoreIssue>,
    issue_kind: &str,
    depth: usize,
    budget: &mut ScanBudget,
) -> Result<ScanSummary> {
    let path = parent.display_path().join(name);
    budget.check_depth(depth, &path)?;
    let Some(metadata) = parent.entry_metadata(name).map_err(|source| {
        DaemonCoreError::io("failed to inspect retained capability entry", &path, source)
    })?
    else {
        return Ok(ScanSummary {
            safe: true,
            ..ScanSummary::default()
        });
    };
    let mut summary = scan_summary_from_capability_metadata(metadata);
    let fingerprint = capability_metadata_hasher(metadata);
    if metadata.identity.device != parent.identity().device {
        summary.safe = false;
        push_issue(
            issues,
            "cross_device_entry",
            &path,
            "entries on another filesystem are not traversed or eligible for retention".to_string(),
        );
        summary.metadata_fingerprint = *fingerprint.finalize().as_bytes();
        return Ok(summary);
    }
    match metadata.kind {
        CapabilityEntryKind::Symlink => {
            summary.symlinks = 1;
            summary.safe = false;
            push_issue(
                issues,
                "symlink_entry",
                &path,
                "symlinks are never followed or removed by retention".to_string(),
            );
        }
        CapabilityEntryKind::RegularFile => {
            summary.files = 1;
            if metadata.link_count > 1 {
                summary.safe = false;
                push_issue(
                    issues,
                    "hardlink_entry",
                    &path,
                    "multiply-linked regular files are not eligible for retention".to_string(),
                );
            }
            if let Err(source) = parent.authenticate_regular_file_for_scan(name, metadata.identity)
            {
                summary.safe = false;
                push_issue(
                    issues,
                    issue_kind,
                    &path,
                    format!("regular file failed descriptor authentication: {source}"),
                );
            }
        }
        CapabilityEntryKind::Directory => match parent.open_dir(name) {
            Ok(child) if child.identity() == metadata.identity => {
                return scan_capability_directory_with_budget(
                    &child, issues, issue_kind, depth, budget,
                );
            }
            Ok(_) => {
                summary.directories = 1;
                summary.safe = false;
                push_issue(
                    issues,
                    issue_kind,
                    &path,
                    "directory identity changed while it was opened".to_string(),
                );
            }
            Err(source) => {
                summary.directories = 1;
                summary.safe = false;
                push_issue(
                    issues,
                    issue_kind,
                    &path,
                    format!("directory failed descriptor authentication: {source}"),
                );
            }
        },
        CapabilityEntryKind::Other => {
            summary.safe = false;
            push_issue(
                issues,
                "special_entry",
                &path,
                "non-file, non-directory entry is not eligible for retention".to_string(),
            );
        }
    }
    summary.metadata_fingerprint = *fingerprint.finalize().as_bytes();
    Ok(summary)
}

#[cfg(unix)]
fn scan_summary_from_capability_metadata(metadata: CapabilityEntryMetadata) -> ScanSummary {
    ScanSummary {
        // Directory inode sizes are filesystem implementation details and
        // were never part of the path scanner's logical-byte accounting.
        logical_bytes: if metadata.kind == CapabilityEntryKind::Directory {
            0
        } else {
            metadata.logical_bytes
        },
        allocated_bytes: metadata.allocated_bytes,
        latest_timestamp_unix: u64::try_from(metadata.modified_unix_seconds).ok(),
        safe: true,
        identity: Some(metadata.identity),
        physical_identities: BTreeSet::from([metadata.identity]),
        ..ScanSummary::default()
    }
}

#[cfg(unix)]
fn capability_metadata_hasher(metadata: CapabilityEntryMetadata) -> blake3::Hasher {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"packet28-retention-metadata-v1");
    let kind = match metadata.kind {
        CapabilityEntryKind::Symlink => 0,
        CapabilityEntryKind::RegularFile => 1,
        CapabilityEntryKind::Directory => 2,
        CapabilityEntryKind::Other => 3,
    };
    hasher.update(&[kind]);
    hasher.update(&metadata.logical_bytes.to_le_bytes());
    if metadata.modified_unix_seconds >= 0 {
        hasher.update(&[1]);
        let seconds = metadata.modified_unix_seconds as u128;
        let nanos = seconds
            .saturating_mul(1_000_000_000)
            .saturating_add(metadata.modified_subsec_nanos as u128);
        hasher.update(&nanos.to_le_bytes());
    } else {
        hasher.update(&[0]);
    }
    hasher.update(&metadata.link_count.to_le_bytes());
    hash_file_identity(&mut hasher, metadata.identity);
    hasher
}

fn merge_scan_summary(summary: &mut ScanSummary, child: &ScanSummary) {
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
    #[cfg(unix)]
    summary
        .physical_identities
        .extend(child.physical_identities.iter().copied());
}

#[cfg(unix)]
pub(super) fn filesystem_allocated_bytes(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
pub(super) fn filesystem_allocated_bytes(metadata: &fs::Metadata) -> u64 {
    if metadata.is_dir() {
        0
    } else {
        metadata.len()
    }
}

#[cfg(unix)]
pub(super) fn same_device(parent: Option<FileIdentity>, child: Option<FileIdentity>) -> bool {
    matches!(
        (parent, child),
        (Some(parent), Some(child)) if parent.device == child.device
    )
}

#[cfg(unix)]
pub(super) fn ensure_same_filesystem(
    expected: FileIdentity,
    actual: FileIdentity,
    path: &Path,
    operation: &'static str,
) -> Result<()> {
    if same_device(Some(expected), Some(actual)) {
        return Ok(());
    }
    Err(DaemonCoreError::io(
        operation,
        path,
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "expected device {}, observed device {}",
                expected.device, actual.device
            ),
        ),
    ))
}

#[cfg(not(unix))]
pub(super) fn same_device(_parent: Option<FileIdentity>, _child: Option<FileIdentity>) -> bool {
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        hasher.update(&metadata.nlink().to_le_bytes());
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
pub(super) fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;

    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(not(unix))]
pub(super) fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    FileIdentity {
        length: metadata.len(),
        modified_unix_nanos: modified_unix_nanos(metadata).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn path_and_capability_scans_share_measurements_without_following_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::create_dir(root.path().join("empty")).unwrap();
        fs::write(root.path().join("first"), b"abc").unwrap();
        fs::write(root.path().join("nested/second"), b"defg").unwrap();
        fs::write(outside.path().join("keep"), b"outside sentinel").unwrap();
        for file in ["first", "nested/second"] {
            fs::set_permissions(root.path().join(file), fs::Permissions::from_mode(0o600)).unwrap();
        }
        let directory = CapabilityDir::open(root.path()).unwrap();
        let compare = || {
            let path =
                scan_path_with_limits(root.path(), &mut Vec::new(), "test", ScanLimits::DEFAULT)
                    .unwrap();
            let anchored = scan_capability_directory_with_limits(
                &directory,
                &mut Vec::new(),
                "test",
                ScanLimits::DEFAULT,
            )
            .unwrap();
            assert_eq!(path, anchored);
            path
        };

        let safe = compare();
        assert!(safe.safe);
        assert_eq!(
            (
                safe.logical_bytes,
                safe.files,
                safe.directories,
                safe.symlinks
            ),
            (7, 2, 3, 0)
        );

        symlink(outside.path(), root.path().join("nested/link")).unwrap();
        let unsafe_scan = compare();
        assert!(!unsafe_scan.safe);
        assert_eq!(
            (
                unsafe_scan.files,
                unsafe_scan.directories,
                unsafe_scan.symlinks
            ),
            (2, 3, 1)
        );
        assert_eq!(
            unsafe_scan.logical_bytes,
            7 + fs::symlink_metadata(root.path().join("nested/link"))
                .unwrap()
                .len()
        );
        assert!(!unsafe_scan.physical_identities.contains(&file_identity(
            &fs::symlink_metadata(outside.path().join("keep")).unwrap()
        )));
        assert_eq!(
            fs::read(outside.path().join("keep")).unwrap(),
            b"outside sentinel"
        );
    }

    #[test]
    fn summary_merge_saturates_counts_and_preserves_unsafe_descendants() {
        let mut parent = ScanSummary {
            logical_bytes: u64::MAX,
            allocated_bytes: u64::MAX,
            files: u64::MAX,
            directories: u64::MAX,
            symlinks: u64::MAX,
            latest_timestamp_unix: Some(10),
            safe: true,
            ..ScanSummary::default()
        };
        let child = ScanSummary {
            logical_bytes: 1,
            allocated_bytes: 1,
            files: 1,
            directories: 1,
            symlinks: 1,
            latest_timestamp_unix: Some(20),
            safe: false,
            ..ScanSummary::default()
        };
        merge_scan_summary(&mut parent, &child);
        assert_eq!(
            [
                parent.logical_bytes,
                parent.allocated_bytes,
                parent.files,
                parent.directories,
                parent.symlinks
            ],
            [u64::MAX; 5]
        );
        assert_eq!(parent.latest_timestamp_unix, Some(20));
        assert!(!parent.safe);
    }

    #[test]
    fn scan_depth_bound_rejects_the_first_deeper_entry_without_mutation() {
        let root = tempdir().unwrap();
        let deepest_directory = root.path().join("one").join("two");
        fs::create_dir_all(&deepest_directory).unwrap();
        let limits = ScanLimits {
            max_depth: 2,
            max_entries_per_traversal: 16,
            max_entries_per_managed_root: 16,
        };

        assert!(scan_path_with_limits(root.path(), &mut Vec::new(), "test", limits).is_ok());

        let too_deep = deepest_directory.join("three");
        fs::write(&too_deep, b"keep").unwrap();
        let error =
            scan_path_with_limits(root.path(), &mut Vec::new(), "test", limits).unwrap_err();

        assert!(error.to_string().contains("directory-depth bound"));
        assert_eq!(fs::read(too_deep).unwrap(), b"keep");
    }

    #[test]
    fn scan_entry_bound_rejects_the_first_excess_entry_without_partial_summary() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("one"), b"1").unwrap();
        fs::write(root.path().join("two"), b"2").unwrap();
        let limits = ScanLimits {
            max_depth: 2,
            max_entries_per_traversal: 2,
            max_entries_per_managed_root: 2,
        };

        assert!(scan_path_with_limits(root.path(), &mut Vec::new(), "test", limits).is_ok());

        let excess = root.path().join("three");
        fs::write(&excess, b"3").unwrap();
        let error =
            scan_path_with_limits(root.path(), &mut Vec::new(), "test", limits).unwrap_err();

        assert!(error.to_string().contains("entry bound"));
        assert_eq!(fs::read(excess).unwrap(), b"3");
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_binding_compares_devices_not_inodes() {
        let state = FileIdentity {
            device: 7,
            inode: 11,
        };
        let sibling = FileIdentity {
            device: 7,
            inode: 12,
        };
        let foreign = FileIdentity {
            device: 8,
            inode: 11,
        };

        assert!(
            ensure_same_filesystem(state, sibling, Path::new("/state/sibling"), "test").is_ok()
        );
        let error = ensure_same_filesystem(state, foreign, Path::new("/state/foreign"), "test")
            .unwrap_err();

        assert!(error.to_string().contains("expected device 7"));
        assert!(error.to_string().contains("observed device 8"));
    }

    #[cfg(unix)]
    #[test]
    fn cross_device_scan_stops_before_enumerating_the_foreign_directory() {
        let parent_identity = file_identity(&fs::symlink_metadata("/").unwrap());
        let foreign = ["/proc", "/dev", "/sys", "/tmp"]
            .into_iter()
            .map(Path::new)
            .find(|path| {
                fs::symlink_metadata(path).is_ok_and(|metadata| {
                    metadata.is_dir()
                        && !metadata.file_type().is_symlink()
                        && !same_device(Some(parent_identity), Some(file_identity(&metadata)))
                })
            });
        let Some(foreign) = foreign else {
            return;
        };
        let mut issues = Vec::new();
        let mut budget = ScanBudget::new(ScanLimits {
            max_depth: 2,
            max_entries_per_traversal: 0,
            max_entries_per_managed_root: 0,
        });

        let scan = scan_path_with_budget(
            foreign,
            &mut issues,
            "test",
            1,
            Some(parent_identity),
            &mut budget,
        )
        .unwrap();

        assert!(!scan.safe);
        assert_eq!(budget.entries_seen, 0);
        assert!(issues
            .iter()
            .any(|issue| issue.kind == "cross_device_entry"));
    }
}
