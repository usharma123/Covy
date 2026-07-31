//! Typed failures for the reusable test CLI adapters.

use thiserror::Error;

/// Stable failures returned by `testy-cli-common`.
///
/// Core planning and persistence failures remain available through
/// [`Self::Core`], including their nested I/O, configuration, codec, and
/// adapter sources. JSON variants retain the concrete [`serde_json::Error`]
/// instead of reducing it to display text.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TestyCliError {
    /// A reusable `testy-core` operation failed.
    #[error(transparent)]
    Core(#[from] testy_core::error::TestyError),

    /// JSON output serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// JSON input failed to decode and includes its expected schema.
    #[error("Failed to parse {type_name}: {source}\n\nExpected JSON shape:\n{example}")]
    JsonWithExample {
        /// Human-readable name of the input type.
        type_name: String,
        /// Concrete JSON decoder failure.
        #[source]
        source: serde_json::Error,
        /// Expected JSON example shown to CLI users.
        example: String,
    },

    /// CLI input violates a command invariant.
    #[error("{message}")]
    InvalidInput {
        /// Actionable validation message.
        message: String,
    },
}

impl TestyCliError {
    pub(crate) fn json_with_example(
        type_name: impl Into<String>,
        source: serde_json::Error,
        example: impl Into<String>,
    ) -> Self {
        Self::JsonWithExample {
            type_name: type_name.into(),
            source,
            example: example.into(),
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: message.into(),
        }
    }
}

/// Result returned by fallible `testy-cli-common` helpers.
pub type Result<T> = std::result::Result<T, TestyCliError>;
