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
        }
    }
}

/// Result returned by fallible `packet28-daemon-core` operations.
pub type Result<T> = std::result::Result<T, DaemonCoreError>;
