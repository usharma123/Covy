//! Typed failures returned by daemon persistence and compatibility APIs.

use std::io;
use std::path::{Path, PathBuf};

use packet28_daemon_protocol::frame::FrameError;
use thiserror::Error;

/// Failure produced by reusable `packet28-daemon-core` operations.
///
/// Filesystem, codec, and framing variants retain their concrete source in the
/// standard [`std::error::Error::source`] chain. Policy and validation
/// variants expose their rejected values directly. The enum is non-exhaustive
/// so future storage backends can add precise failure modes without forcing
/// downstream exhaustive matches.
///
/// # Examples
///
/// ```
/// use std::error::Error as _;
/// use std::io::Cursor;
///
/// use packet28_daemon_core::{read_socket_message, DaemonCoreError, DaemonRequest};
///
/// let mut empty_frame = Cursor::new(0_u64.to_be_bytes());
/// let error = read_socket_message::<_, DaemonRequest>(&mut empty_frame).unwrap_err();
///
/// assert!(matches!(error, DaemonCoreError::Frame { .. }));
/// assert!(error.source().is_some());
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DaemonCoreError {
    /// A filesystem or file-lock operation failed.
    #[error("{operation} {}: {source}", path.display())]
    Io {
        /// Operation that failed.
        operation: &'static str,
        /// Path being accessed.
        path: PathBuf,
        /// Operating-system failure.
        #[source]
        source: io::Error,
    },

    /// Persisted daemon JSON could not be encoded or decoded.
    #[error("{operation} {}: {source}", path.display())]
    Json {
        /// Encoding or decoding operation that failed.
        operation: &'static str,
        /// Persisted JSON path.
        path: PathBuf,
        /// Typed JSON codec failure.
        #[source]
        source: serde_json::Error,
    },

    /// A task registry would exceed the supported serialized-size bound.
    #[error(
        "task registry {} is {encoded_bytes} bytes; maximum supported size is {max_bytes} bytes",
        path.display()
    )]
    TaskRegistryTooLarge {
        /// Registry path that was not replaced.
        path: PathBuf,
        /// Encoded size of the rejected registry.
        encoded_bytes: u64,
        /// Maximum supported encoded size.
        max_bytes: u64,
    },

    /// A registry record cannot fit the bounded crash-recovery journal.
    #[error(
        "task registry {} requires a {journal_bytes}-byte retention journal; maximum supported size is {max_bytes} bytes",
        path.display()
    )]
    TaskRegistryRetentionEnvelopeTooLarge {
        /// Registry path that was not replaced.
        path: PathBuf,
        /// Encoded size of the rejected maximum candidate journal.
        journal_bytes: u64,
        /// Maximum supported encoded journal size.
        max_bytes: u64,
    },

    /// An active-task record would exceed the supported serialized-size bound.
    #[error(
        "active-task record {} is {encoded_bytes} bytes; maximum supported size is {max_bytes} bytes",
        path.display()
    )]
    ActiveTaskRecordTooLarge {
        /// Active-task path that was not replaced or decoded.
        path: PathBuf,
        /// Encoded size of the rejected record.
        encoded_bytes: u64,
        /// Maximum supported encoded size.
        max_bytes: u64,
    },

    /// An active-task record violates an invariant required by all writers.
    #[error("invalid active-task record {}: {message}", path.display())]
    InvalidActiveTaskRecord {
        /// Active-task path that was rejected.
        path: PathBuf,
        /// Stable explanation of the invalid record.
        message: String,
    },

    /// A task registry violates an invariant required by all supported writers.
    #[error("invalid task registry {}: {message}", path.display())]
    InvalidTaskRegistry {
        /// Registry path that was rejected.
        path: PathBuf,
        /// Stable explanation of the invalid registry shape.
        message: String,
    },

    /// A task identifier cannot be represented safely by task storage paths.
    #[error("invalid task storage identifier for {}: {message}", path.display())]
    InvalidTaskStorageIdentifier {
        /// Storage path that was not created or changed.
        path: PathBuf,
        /// Stable explanation of the invalid identifier.
        message: String,
    },

    /// A task-event frame is not bound to the log selected by its path.
    #[error("invalid task event frame {}: {message}", path.display())]
    InvalidTaskEventFrame {
        /// Event-log path whose frame was rejected.
        path: PathBuf,
        /// Stable explanation of the identity mismatch.
        message: String,
    },

    /// A durable mutation completed before its storage authority was lost.
    ///
    /// The mutation must not be retried blindly: its bytes may already be
    /// durable even though Packet28 could not prove that the canonical
    /// filename and lock still named the authenticated objects at return.
    #[error(
        "{operation} may have committed at {} before storage authority was lost: {source}",
        path.display()
    )]
    StorageMutationAuthorityLost {
        /// Durable mutation whose completion could not be authenticated.
        operation: &'static str,
        /// Canonical path whose final binding became uncertain.
        path: PathBuf,
        /// Attachment or descriptor/name authentication failure.
        #[source]
        source: io::Error,
    },

    /// Authority JSON exceeded a pre-materialization resource budget.
    #[error(
        "{authority} JSON {} exceeds the {resource} budget: observed {observed}, maximum {max}"
        ,
        path.display()
    )]
    AuthorityJsonLimitExceeded {
        /// Authority file that was rejected without materializing it.
        path: PathBuf,
        /// Stable authority kind, such as `task registry`.
        authority: &'static str,
        /// Stable resource name, such as `value nodes`.
        resource: &'static str,
        /// First count that exceeded the budget.
        observed: u64,
        /// Maximum accepted count.
        max: u64,
    },

    /// A legacy daemon-core socket frame could not be encoded or decoded.
    #[error("{operation} daemon socket frame: {source}")]
    Frame {
        /// Framing operation that failed.
        operation: &'static str,
        /// Typed protocol framing failure.
        #[source]
        source: FrameError,
    },

    /// A retention request did not specify a usable bound.
    #[error("invalid task-retention policy: {message}")]
    InvalidRetentionPolicy {
        /// Explanation of the rejected policy.
        message: &'static str,
    },

    /// The Packet28 state root was not a real directory contained by the workspace.
    #[error(
        "unsafe Packet28 state root {} for workspace {}: {reason}",
        state_root.display(),
        workspace_root.display()
    )]
    UnsafeStateRoot {
        /// Canonical workspace root.
        workspace_root: PathBuf,
        /// State path that failed validation.
        state_root: PathBuf,
        /// Failed containment or file-type invariant.
        reason: &'static str,
    },

    /// Cleanup was requested while the daemon may still own task state.
    #[error("task retention cannot apply while daemon owns task storage at {}", path.display())]
    RetentionBlockedByDaemon {
        /// Lifecycle lock or readiness marker requiring the daemon to stop.
        path: PathBuf,
    },

    /// Another daemon process already owns this workspace.
    #[error("another Packet28 daemon already owns {}", path.display())]
    DaemonInstanceAlreadyRunning {
        /// Persistent instance-lock path owned by the running daemon.
        path: PathBuf,
    },

    /// A candidate changed after inspection and was not removed.
    #[error("task-retention candidate changed during cleanup: {}", path.display())]
    RetentionCandidateChanged {
        /// Candidate path whose identity changed.
        path: PathBuf,
    },

    /// Applying retention is unsupported on this platform.
    #[error("task-retention deletion is unsupported on this platform; use dry-run inspection")]
    RetentionApplyUnsupported,
}

impl DaemonCoreError {
    pub(crate) fn io(operation: &'static str, path: impl AsRef<Path>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    pub(crate) fn json(
        operation: &'static str,
        path: impl AsRef<Path>,
        source: serde_json::Error,
    ) -> Self {
        Self::Json {
            operation,
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    pub(crate) fn frame(operation: &'static str, source: FrameError) -> Self {
        Self::Frame { operation, source }
    }

    /// Returns recovery guidance for this category of failure.
    pub fn hint(&self) -> &'static str {
        match self {
            Self::Io { .. } => "Check that the reported path exists and is readable and writable.",
            Self::Json { .. } => {
                "Repair or regenerate the reported daemon state file before retrying."
            }
            Self::TaskRegistryTooLarge { .. } => {
                "Reduce completed task history before saving the task registry."
            }
            Self::TaskRegistryRetentionEnvelopeTooLarge { .. } => {
                "Reduce the largest task record before saving the task registry."
            }
            Self::ActiveTaskRecordTooLarge { .. } => {
                "Reduce active-task metadata before saving the record."
            }
            Self::InvalidActiveTaskRecord { .. } => {
                "Provide a non-empty active task identifier."
            }
            Self::InvalidTaskRegistry { .. } => {
                "Make each non-empty task map key match its embedded task identifier."
            }
            Self::InvalidTaskStorageIdentifier { .. } => {
                "Use a non-empty task identifier whose derived storage key is portable and unambiguous."
            }
            Self::InvalidTaskEventFrame { .. } => {
                "Repair the event log so every frame carries the exact task identifier selected by its path."
            }
            Self::StorageMutationAuthorityLost { .. } => {
                "Do not retry blindly; inspect the canonical file and registry under an authenticated lock first."
            }
            Self::AuthorityJsonLimitExceeded { .. } => {
                "Reduce the authority document's nesting, entry count, or decoded string content."
            }
            Self::Frame { .. } => {
                "Verify that the peer uses the same Packet28 daemon protocol version."
            }
            Self::InvalidRetentionPolicy { .. } => {
                "Specify --max-age-seconds, --max-bytes, or both."
            }
            Self::UnsafeStateRoot { .. } => {
                "Replace symlinked or non-directory Packet28 state with a real workspace-local directory."
            }
            Self::RetentionBlockedByDaemon { .. } => {
                "Stop packet28d before applying task retention."
            }
            Self::DaemonInstanceAlreadyRunning { .. } => {
                "Use the running daemon or stop it before starting another instance."
            }
            Self::RetentionCandidateChanged { .. } => {
                "Re-run the dry-run inspection before retrying cleanup."
            }
            Self::RetentionApplyUnsupported => {
                "Run retention in dry-run mode and remove data manually on this platform."
            }
        }
    }
}

/// Result returned by fallible `packet28-daemon-core` operations.
pub type Result<T> = std::result::Result<T, DaemonCoreError>;
