//! Errors shared by packet parsing, configuration, and artifact operations.

use std::path::PathBuf;

use thiserror::Error;

/// Stable error categories returned by packet and artifact APIs.
#[derive(Debug, Error)]
pub enum CovyError {
    /// A filesystem operation failed for a known path.
    #[error("Failed to read {path}: {source}")]
    Io {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Original I/O error.
        source: std::io::Error,
    },

    /// A raw I/O failure occurred before a meaningful path was available.
    #[error("IO error: {0}")]
    IoRaw(#[from] std::io::Error),

    /// A packet or coverage representation could not be parsed.
    #[error("Failed to parse {format} coverage: {detail}")]
    Parse { format: String, detail: String },

    /// An XML document could not be decoded.
    #[error("XML parse error: {0}")]
    Xml(String),

    /// Input did not match a supported coverage format.
    #[error("Unknown coverage format for {path} (use --format to specify)")]
    UnknownFormat { path: String },

    /// Git was required but no executable was available.
    #[error("Git is not installed or not found in PATH")]
    GitNotFound,

    /// Git returned an operation-specific error.
    #[error("Git error: {0}")]
    Git(String),

    /// An input existed but contained no bytes.
    #[error("Coverage file is empty: {path}")]
    EmptyInput { path: String },

    /// Configuration was missing, invalid, or internally inconsistent.
    #[error("Config error: {0}")]
    Config(String),

    /// A packet cache operation failed.
    #[error("Cache error: {0}")]
    Cache(String),

    /// A source path could not be mapped into the repository.
    #[error("Path mapping failed: no match for {0}")]
    PathMapping(String),

    /// An error that does not fit a more stable category.
    #[error("{0}")]
    Other(String),
}

impl CovyError {
    /// Return a user-friendly hint for this error, if any.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            CovyError::UnknownFormat { .. } => {
                Some("Supported formats: lcov, cobertura, jacoco, gocov, llvm-cov")
            }
            CovyError::GitNotFound => Some("Install git or ensure it is available in your PATH"),
            CovyError::EmptyInput { .. } => {
                Some("Check that your test runner generated coverage output")
            }
            _ => None,
        }
    }
}

impl From<toml::de::Error> for CovyError {
    fn from(e: toml::de::Error) -> Self {
        CovyError::Config(e.to_string())
    }
}
