//! Error types for diff collection and analysis orchestration.

use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

pub use suite_packet_core::error::CovyError;
use thiserror::Error;

/// Errors produced by diff collection and the analysis pipeline.
///
/// The enum is non-exhaustive so new failure modes can be added without forcing
/// downstream exhaustive matches.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DiffyError {
    /// Git could not be found when starting a diff operation.
    #[error("Git is not installed or not found in PATH")]
    GitNotFound {
        /// Git operation that could not be started.
        operation: &'static str,
        /// Operating-system error returned while starting Git.
        #[source]
        source: io::Error,
    },

    /// Git was found but could not be started for another reason.
    #[error("Failed to run git {operation}: {source}")]
    GitSpawn {
        /// Git operation that could not be started.
        operation: &'static str,
        /// Operating-system error returned while starting Git.
        #[source]
        source: io::Error,
    },

    /// Git completed unsuccessfully.
    #[error("git {operation} failed with status {status}: {stderr}")]
    GitCommandFailed {
        /// Git operation that failed.
        operation: &'static str,
        /// Process exit status.
        status: ExitStatus,
        /// Standard error emitted by Git.
        stderr: String,
    },

    /// `git rev-parse` succeeded without returning both requested hashes.
    #[error("git rev-parse returned an empty ref hash")]
    EmptyGitRefHash,

    /// Mutually exclusive pipeline inputs were provided together.
    #[error("Cannot combine {first} with {second}")]
    ConflictingInputs {
        /// First conflicting input.
        first: &'static str,
        /// Second conflicting input.
        second: &'static str,
    },

    /// Standard-input ingestion was requested without an explicit format.
    #[error("--format is required when reading from --stdin (can't auto-detect)")]
    MissingStdinFormat,

    /// An explicit coverage state path does not exist.
    #[error(
        "No coverage data found at {path}. Run `covy ingest` first or provide valid coverage paths."
    )]
    CoverageStateNotFound {
        /// Missing state path.
        path: PathBuf,
    },

    /// The default coverage state path does not exist.
    #[error(
        "No coverage files specified and no cached coverage state found at {path}. Provide file paths, use --stdin, or run `covy ingest` first."
    )]
    DefaultCoverageStateNotFound {
        /// Missing default state path.
        path: PathBuf,
    },

    /// No coverage source was selected and fallback was disabled.
    #[error("{message}")]
    MissingCoverageInput {
        /// Caller-provided CLI guidance for selecting a coverage input.
        message: String,
    },

    /// Coverage glob patterns matched no files.
    #[error("No coverage files found")]
    NoCoverageFiles,

    /// Diagnostics glob patterns matched no files.
    #[error("No diagnostics files found")]
    NoDiagnosticsFiles,

    /// A coverage or diagnostics glob pattern is invalid.
    #[error("Invalid glob pattern `{pattern}`: {source}")]
    InvalidGlob {
        /// Invalid pattern.
        pattern: String,
        /// Glob parser error.
        #[source]
        source: glob::PatternError,
    },

    /// A coverage report could not be ingested.
    #[error("Failed to ingest coverage report {path}: {source}")]
    CoverageIngest {
        /// Report path.
        path: PathBuf,
        /// Typed ingestion failure.
        #[source]
        source: CovyError,
    },

    /// Coverage from standard input could not be ingested.
    #[error("Failed to ingest coverage from stdin: {source}")]
    CoverageStdinIngest {
        /// Typed ingestion failure.
        #[source]
        source: CovyError,
    },

    /// A coverage state file could not be read.
    #[error("Failed to read coverage state {path}: {source}")]
    CoverageStateRead {
        /// State path.
        path: PathBuf,
        /// File-system failure.
        #[source]
        source: io::Error,
    },

    /// A coverage state file could not be decoded.
    #[error("Failed to decode coverage state {path}: {source}")]
    CoverageStateDecode {
        /// State path.
        path: PathBuf,
        /// Typed state decoder failure.
        #[source]
        source: CovyError,
    },

    /// A cached diagnostics state could not be loaded.
    #[error("Failed to load diagnostics state {path}: {source}")]
    DiagnosticsStateLoad {
        /// State path.
        path: PathBuf,
        /// Typed state read or decoder failure.
        #[source]
        source: CovyError,
    },

    /// A binary diagnostics state file could not be read.
    #[error("Failed to read diagnostics state {path}: {source}")]
    DiagnosticsStateRead {
        /// State path.
        path: PathBuf,
        /// File-system failure.
        #[source]
        source: io::Error,
    },

    /// A binary diagnostics state file could not be decoded.
    #[error("Failed to decode diagnostics state {path}: {source}")]
    DiagnosticsStateDecode {
        /// State path.
        path: PathBuf,
        /// Typed state decoder failure.
        #[source]
        source: CovyError,
    },

    /// A diagnostics report could not be ingested.
    #[error("Failed to ingest diagnostics report {path}: {source}")]
    DiagnosticsIngest {
        /// Report path.
        path: PathBuf,
        /// Typed ingestion failure.
        #[source]
        source: CovyError,
    },
}

impl DiffyError {
    /// Return actionable user guidance for errors that have a standard remedy.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::GitNotFound { .. } => Some("Install git or ensure it is available in your PATH"),
            _ => None,
        }
    }
}

/// Result type returned by fallible `diffy-core` operations.
pub type Result<T> = std::result::Result<T, DiffyError>;

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io::ErrorKind;

    use super::DiffyError;

    #[test]
    fn git_spawn_error_has_stable_display() {
        let error = DiffyError::GitSpawn {
            operation: "diff",
            source: std::io::Error::new(ErrorKind::PermissionDenied, "blocked"),
        };

        assert_eq!(error.to_string(), "Failed to run git diff: blocked");
    }

    #[test]
    fn git_spawn_error_preserves_io_source() {
        let error = DiffyError::GitSpawn {
            operation: "rev-parse",
            source: std::io::Error::new(ErrorKind::PermissionDenied, "blocked"),
        };

        let source = error.source().expect("Git spawn error must have a source");

        assert_eq!(
            source.downcast_ref::<std::io::Error>().map(|io| io.kind()),
            Some(ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn git_not_found_error_has_actionable_hint() {
        let error = DiffyError::GitNotFound {
            operation: "diff",
            source: std::io::Error::from(ErrorKind::NotFound),
        };

        assert_eq!(
            error.hint(),
            Some("Install git or ensure it is available in your PATH")
        );
    }
}
