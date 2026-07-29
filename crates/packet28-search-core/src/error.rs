//! Typed failures returned by the search index.

use std::error::Error;
use std::fmt;
use std::io;
use std::num::TryFromIntError;

/// Errors produced while building, loading, or querying the search index.
///
/// The enum is non-exhaustive so the crate can add more precise failure modes
/// without forcing downstream exhaustive matches.
#[derive(Debug)]
#[non_exhaustive]
pub enum SearchError {
    /// An indexed operation was requested before a ready index was loaded.
    IndexNotLoaded,

    /// A search request contained an empty or whitespace-only query.
    EmptyQuery,

    /// The query could not be parsed into the regex syntax tree used by the planner.
    InvalidRegexSyntax {
        /// Query that failed to parse.
        query: String,
        /// Typed parser failure.
        source: Box<regex_syntax::Error>,
    },

    /// The query could not be compiled by the regex verifier.
    InvalidRegex {
        /// Query that failed to compile.
        query: String,
        /// Typed regex compiler failure.
        source: Box<regex::Error>,
    },

    /// A filesystem operation failed.
    ///
    /// Higher-level operations may wrap this variant in [`SearchError::Context`]
    /// to retain the affected path and operation.
    Io {
        /// Operating-system failure.
        source: io::Error,
    },

    /// A JSON manifest or overlay state could not be encoded.
    Json {
        /// Typed JSON encoder failure.
        source: serde_json::Error,
    },

    /// A binary index document table could not be decoded.
    BinaryDecode {
        /// Typed binary decoder failure.
        source: packet28_binary_codec::ReadError,
    },

    /// A binary index document table could not be encoded.
    BinaryEncode {
        /// Typed binary encoder failure.
        source: packet28_binary_codec::WriteError,
    },

    /// An index offset or identifier could not be represented on this platform.
    IntegerConversion {
        /// Typed integer conversion failure.
        source: TryFromIntError,
    },

    /// An index artifact violated a validated structural invariant.
    CorruptIndex {
        /// Description of the rejected invariant.
        message: String,
    },

    /// A writer tried to publish from a generation that is no longer current.
    ConcurrentWriter {
        /// Generation owned by the caller.
        expected: u64,
        /// Generation currently published on disk.
        actual: u64,
    },

    /// An incremental update path did not resolve beneath the repository root.
    InvalidChangedPath {
        /// Rejected caller-supplied path.
        path: String,
    },

    /// Additional operation or path context for another typed search failure.
    Context {
        /// Operation and, when available, affected artifact.
        context: String,
        /// Typed underlying failure.
        source: Box<SearchError>,
    },

    /// An index build failed and recording that failure also failed.
    FailureProvenance {
        /// Original build failure, retained as the causal source.
        build: Box<SearchError>,
        /// Failure encountered while persisting the build provenance.
        persistence: Box<SearchError>,
    },
}

impl fmt::Display for SearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IndexNotLoaded => formatter.write_str("regex index not loaded"),
            Self::EmptyQuery => formatter.write_str("search query cannot be empty"),
            Self::InvalidRegexSyntax { query, source } => write!(
                formatter,
                "unsupported regex syntax for packet28.search: {query}: {source}"
            ),
            Self::InvalidRegex { query, source } => write!(
                formatter,
                "unsupported regex syntax for packet28.search: {query}: {source}"
            ),
            Self::Io { source } => source.fmt(formatter),
            Self::Json { source } => source.fmt(formatter),
            Self::BinaryDecode { source } => source.fmt(formatter),
            Self::BinaryEncode { source } => source.fmt(formatter),
            Self::IntegerConversion { source } => source.fmt(formatter),
            Self::CorruptIndex { message } => formatter.write_str(message),
            Self::ConcurrentWriter { expected, actual } => write!(
                formatter,
                "regex index generation conflict: caller has {expected}, published generation is {actual}"
            ),
            Self::InvalidChangedPath { path } => write!(
                formatter,
                "changed path '{path}' must resolve beneath the repository root"
            ),
            Self::Context { context, source } => write!(formatter, "{context}: {source}"),
            Self::FailureProvenance { build, persistence } => write!(
                formatter,
                "failed to persist regex index failure provenance: {persistence}: {build}"
            ),
        }
    }
}

impl Error for SearchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidRegexSyntax { source, .. } => Some(source.as_ref()),
            Self::InvalidRegex { source, .. } => Some(source.as_ref()),
            Self::Io { source } => Some(source),
            Self::Json { source } => Some(source),
            Self::BinaryDecode { source } => Some(source),
            Self::BinaryEncode { source } => Some(source),
            Self::IntegerConversion { source } => Some(source),
            Self::Context { source, .. } => Some(source.as_ref()),
            Self::FailureProvenance { build, .. } => Some(build.as_ref()),
            Self::IndexNotLoaded
            | Self::EmptyQuery
            | Self::CorruptIndex { .. }
            | Self::ConcurrentWriter { .. }
            | Self::InvalidChangedPath { .. } => None,
        }
    }
}

impl SearchError {
    pub(crate) fn corrupt(message: impl Into<String>) -> Self {
        Self::CorruptIndex {
            message: message.into(),
        }
    }

    pub(crate) fn context(self, context: impl Into<String>) -> Self {
        Self::Context {
            context: context.into(),
            source: Box::new(self),
        }
    }
}

impl From<io::Error> for SearchError {
    fn from(source: io::Error) -> Self {
        Self::Io { source }
    }
}

impl From<serde_json::Error> for SearchError {
    fn from(source: serde_json::Error) -> Self {
        Self::Json { source }
    }
}

impl From<packet28_binary_codec::ReadError> for SearchError {
    fn from(source: packet28_binary_codec::ReadError) -> Self {
        Self::BinaryDecode { source }
    }
}

impl From<packet28_binary_codec::WriteError> for SearchError {
    fn from(source: packet28_binary_codec::WriteError) -> Self {
        Self::BinaryEncode { source }
    }
}

impl From<TryFromIntError> for SearchError {
    fn from(source: TryFromIntError) -> Self {
        Self::IntegerConversion { source }
    }
}

/// Result type returned by fallible `packet28-search-core` operations.
pub type Result<T> = std::result::Result<T, SearchError>;

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io::{self, ErrorKind};

    use super::SearchError;

    #[test]
    fn context_preserves_the_typed_io_source_chain() {
        let error = SearchError::from(io::Error::from(ErrorKind::PermissionDenied))
            .context("failed to read candidate");

        let io_source = error
            .source()
            .and_then(std::error::Error::source)
            .and_then(|source| source.downcast_ref::<io::Error>());

        assert_eq!(
            io_source.map(io::Error::kind),
            Some(ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn context_display_preserves_the_existing_operation_prefix() {
        let error =
            SearchError::from(io::Error::from(ErrorKind::NotFound)).context("failed to open index");

        assert_eq!(error.to_string(), "failed to open index: entity not found");
    }
}
