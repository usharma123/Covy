//! Typed failures for test selection, test-map construction, and shard planning.

use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};

pub use suite_packet_core::error::CovyError;
use thiserror::Error;

/// A boxed source returned by a caller-provided test adapter.
pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// Failure returned by an injected coverage or diff adapter.
///
/// Built-in coverage adapters retain their [`CovyError`] directly. Other
/// integrations can use [`AdapterError::external`] to preserve their concrete
/// error in the standard [`Error::source`] chain.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AdapterError {
    /// Coverage ingestion failed.
    #[error(transparent)]
    Coverage(#[from] CovyError),

    /// A caller-provided adapter failed.
    #[error("{operation}: {source}")]
    External {
        /// Operation attempted by the adapter.
        operation: &'static str,
        /// Original adapter failure.
        #[source]
        source: BoxError,
    },
}

impl AdapterError {
    /// Wrap a typed external adapter failure without reducing it to a string.
    pub fn external(operation: &'static str, source: impl Error + Send + Sync + 'static) -> Self {
        Self::External {
            operation,
            source: Box::new(source),
        }
    }
}

/// Result returned by coverage and diff adapter callbacks.
pub type AdapterResult<T> = std::result::Result<T, AdapterError>;

/// Stable failures returned by reusable `testy-core` operations.
///
/// The enum is non-exhaustive so the library can add precise failure modes
/// without forcing downstream exhaustive matches.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TestyError {
    /// A file or directory operation failed.
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

    /// Configuration loading or parsing failed.
    #[error(transparent)]
    Config(#[from] suite_foundation_core::config::ConfigLoadError),

    /// A persisted test-map or timing state could not be encoded or decoded.
    #[error("{operation} {}: {source}", path.display())]
    State {
        /// State operation that failed.
        operation: &'static str,
        /// State path.
        path: PathBuf,
        /// Typed codec or schema failure.
        #[source]
        source: CovyError,
    },

    /// JSON input or output could not be decoded or encoded.
    #[error("{context}: {source}{example}")]
    Json {
        /// Description of the value being processed.
        context: String,
        /// Typed JSON failure.
        #[source]
        source: serde_json::Error,
        /// Optional schema guidance, including its leading separator.
        example: String,
    },

    /// JUnit XML could not be decoded.
    #[error("{context}: {source}")]
    Xml {
        /// XML value being processed.
        context: &'static str,
        /// Typed XML reader failure.
        #[source]
        source: quick_xml::Error,
    },

    /// A JUnit XML attribute could not be decoded.
    #[error("{context}: {source}")]
    XmlAttribute {
        /// XML value being processed.
        context: &'static str,
        /// Typed attribute failure.
        #[source]
        source: quick_xml::events::attributes::AttrError,
    },

    /// A file glob is syntactically invalid.
    #[error("Invalid glob pattern: {pattern}: {source}")]
    GlobPattern {
        /// Invalid glob.
        pattern: String,
        /// Typed glob parser failure.
        #[source]
        source: glob::PatternError,
    },

    /// An injected coverage or diff adapter failed.
    #[error("{operation}: {source}")]
    Adapter {
        /// Operation attempted through the adapter.
        operation: String,
        /// Original typed adapter failure.
        #[source]
        source: AdapterError,
    },

    /// Caller input violates a command or planning invariant.
    #[error("{message}")]
    InvalidInput {
        /// Actionable validation message.
        message: String,
    },

    /// An internal response omitted a field required by its operation.
    #[error("{operation} response missing {field}")]
    MissingResponseField {
        /// Operation that produced the response.
        operation: &'static str,
        /// Required response field.
        field: &'static str,
    },

    /// A resolved test command could not be started.
    #[error("Failed to execute test command '{program}': {source}")]
    CommandSpawn {
        /// Program that could not be started.
        program: String,
        /// Operating-system process failure.
        #[source]
        source: io::Error,
    },
}

impl TestyError {
    pub(crate) fn io(operation: &'static str, path: impl AsRef<Path>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    pub(crate) fn state(
        operation: &'static str,
        path: impl AsRef<Path>,
        source: CovyError,
    ) -> Self {
        Self::State {
            operation,
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    pub(crate) fn json(
        context: impl Into<String>,
        source: serde_json::Error,
        example: Option<&str>,
    ) -> Self {
        Self::Json {
            context: context.into(),
            source,
            example: example
                .map(|value| format!("\n\nExpected JSON shape:\n{value}"))
                .unwrap_or_default(),
        }
    }

    pub(crate) fn adapter(operation: impl Into<String>, source: AdapterError) -> Self {
        Self::Adapter {
            operation: operation.into(),
            source,
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: message.into(),
        }
    }

    /// Return actionable recovery guidance for the error category.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Io { .. } => Some("Check that the path exists and is readable and writable."),
            Self::Config(_) => Some("Fix the reported configuration path or TOML syntax."),
            Self::State { .. } => {
                Some("Regenerate the test map or timing state from source inputs.")
            }
            Self::Json { .. } => Some("Compare the input with the schema shown in the error."),
            Self::Xml { .. } | Self::XmlAttribute { .. } => {
                Some("Regenerate the JUnit XML and verify that it is well formed.")
            }
            Self::GlobPattern { .. } => Some("Correct the glob syntax and retry."),
            Self::Adapter { .. } => {
                Some("Inspect the nested adapter error for the original cause.")
            }
            Self::InvalidInput { .. } => Some("Review the supplied test-planning arguments."),
            Self::MissingResponseField { .. } => {
                Some("This is an internal response invariant; report it as a bug.")
            }
            Self::CommandSpawn { .. } => {
                Some("Verify that the test executable exists and is available on PATH.")
            }
        }
    }
}

/// Result returned by fallible `testy-core` operations.
pub type Result<T> = std::result::Result<T, TestyError>;
