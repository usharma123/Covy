//! Typed failures returned by daemon persistence and compatibility APIs.

use std::io;
use std::path::{Path, PathBuf};

use packet28_daemon_protocol::frame::FrameError;
use thiserror::Error;

/// Failure produced by reusable `packet28-daemon-core` operations.
///
/// Each variant retains its concrete source in the standard
/// [`std::error::Error::source`] chain. The enum is non-exhaustive so future
/// storage backends can add precise failure modes without forcing downstream
/// exhaustive matches.
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
