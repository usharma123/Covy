//! Daemon index management payloads.

use super::*;

/// Runtime state of the workspace index.
///
/// Variant names serialize to the same lower-case strings used by the legacy
/// daemon manifest.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DaemonIndexState {
    /// No usable index generation exists.
    #[default]
    Missing,
    /// A rebuild has been accepted but has not started.
    Queued,
    /// A worker is building a generation.
    Building,
    /// The current generation is complete and usable.
    Ready,
    /// The generation is structurally valid but no longer matches its source.
    Stale,
    /// The generation failed validation or a build failed before publication.
    Corrupt,
}

impl DaemonIndexState {
    /// Returns the stable wire value for this state.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Queued => "queued",
            Self::Building => "building",
            Self::Ready => "ready",
            Self::Stale => "stale",
            Self::Corrupt => "corrupt",
        }
    }

    /// Applies a valid runtime state transition.
    ///
    /// Repeating a state is idempotent so progress updates can remain cheap.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonIndexTransitionError`] when the requested transition
    /// skips required queue/build phases or attempts to reuse an invalid
    /// generation without first queueing a rebuild.
    pub fn transition_to(&mut self, next: Self) -> Result<(), DaemonIndexTransitionError> {
        let allowed = *self == next
            || matches!(
                (*self, next),
                (Self::Missing, Self::Queued)
                    | (Self::Queued, Self::Building)
                    | (
                        Self::Building,
                        Self::Queued | Self::Ready | Self::Stale | Self::Corrupt
                    )
                    | (
                        Self::Ready,
                        Self::Queued | Self::Building | Self::Stale | Self::Corrupt
                    )
                    | (Self::Stale | Self::Corrupt, Self::Queued)
            );
        if !allowed {
            return Err(DaemonIndexTransitionError {
                from: *self,
                to: next,
            });
        }
        *self = next;
        Ok(())
    }
}

impl std::fmt::Display for DaemonIndexState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for DaemonIndexState {
    type Err = DaemonIndexStateParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "missing" => Ok(Self::Missing),
            "queued" => Ok(Self::Queued),
            "building" => Ok(Self::Building),
            "ready" => Ok(Self::Ready),
            "stale" => Ok(Self::Stale),
            "corrupt" => Ok(Self::Corrupt),
            value => Err(DaemonIndexStateParseError {
                value: value.to_string(),
            }),
        }
    }
}

/// Unknown daemon-index state read from an external status source.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown daemon index state '{value}'")]
pub struct DaemonIndexStateParseError {
    /// Unrecognized wire value.
    pub value: String,
}

/// Invalid workspace-index lifecycle transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid daemon index transition from {from} to {to}")]
pub struct DaemonIndexTransitionError {
    /// Current state.
    pub from: DaemonIndexState,
    /// Requested state.
    pub to: DaemonIndexState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonIndexManifest {
    pub schema_version: u32,
    pub root: String,
    pub generation: u64,
    pub include_tests: bool,
    pub status: DaemonIndexState,
    pub dirty_paths: Vec<String>,
    pub queued_paths: Vec<String>,
    pub total_files: usize,
    pub indexed_files: usize,
    pub regex_generation: Option<u64>,
    pub regex_status: Option<String>,
    pub regex_total_files: usize,
    pub regex_base_commit: Option<String>,
    pub regex_weight_table_version: Option<u32>,
    pub regex_stale_reason: Option<String>,
    pub regex_indexed_files: usize,
    pub last_build_started_at_unix: Option<u64>,
    pub last_build_completed_at_unix: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonIndexStatusRequest {
    pub root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonIndexStatusResponse {
    pub manifest: DaemonIndexManifest,
    pub ready: bool,
    pub fallback_mode: bool,
    pub loaded_generation: Option<u64>,
    pub dirty_file_count: usize,
    pub queued_file_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonIndexRebuildRequest {
    pub root: String,
    pub full: bool,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonIndexRebuildResponse {
    pub accepted: bool,
    pub full: bool,
    pub generation: Option<u64>,
    pub queued_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonIndexClearRequest {
    pub root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonIndexClearResponse {
    pub cleared: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [DaemonIndexState; 6] = [
        DaemonIndexState::Missing,
        DaemonIndexState::Queued,
        DaemonIndexState::Building,
        DaemonIndexState::Ready,
        DaemonIndexState::Stale,
        DaemonIndexState::Corrupt,
    ];

    fn transition_is_allowed(from: DaemonIndexState, to: DaemonIndexState) -> bool {
        from == to
            || matches!(
                (from, to),
                (DaemonIndexState::Missing, DaemonIndexState::Queued)
                    | (DaemonIndexState::Queued, DaemonIndexState::Building)
                    | (
                        DaemonIndexState::Building,
                        DaemonIndexState::Queued
                            | DaemonIndexState::Ready
                            | DaemonIndexState::Stale
                            | DaemonIndexState::Corrupt
                    )
                    | (
                        DaemonIndexState::Ready,
                        DaemonIndexState::Queued
                            | DaemonIndexState::Building
                            | DaemonIndexState::Stale
                            | DaemonIndexState::Corrupt
                    )
                    | (
                        DaemonIndexState::Stale | DaemonIndexState::Corrupt,
                        DaemonIndexState::Queued
                    )
            )
    }

    #[test]
    fn every_index_state_uses_the_legacy_string_wire_value() {
        for state in STATES {
            let encoded = serde_json::to_string(&state).unwrap();
            assert_eq!(encoded, format!("\"{}\"", state.as_str()));
            assert_eq!(
                serde_json::from_str::<DaemonIndexState>(&encoded).unwrap(),
                state
            );
        }
    }

    #[test]
    fn manifest_status_preserves_the_legacy_wire_shape() {
        let legacy = serde_json::json!({
            "schema_version": 2,
            "root": "/repo",
            "generation": 7,
            "status": "ready"
        });
        let manifest: DaemonIndexManifest = serde_json::from_value(legacy).unwrap();

        assert_eq!(manifest.status, DaemonIndexState::Ready);
        assert_eq!(
            serde_json::to_value(manifest).unwrap()["status"],
            serde_json::json!("ready")
        );
    }

    #[test]
    fn transition_matrix_accepts_only_declared_index_edges() {
        for from in STATES {
            for to in STATES {
                let mut state = from;
                let result = state.transition_to(to);
                assert_eq!(
                    result.is_ok(),
                    transition_is_allowed(from, to),
                    "unexpected transition result for {from} -> {to}: {result:?}"
                );
                if result.is_ok() {
                    assert_eq!(state, to);
                } else {
                    assert_eq!(state, from);
                }
            }
        }
    }

    #[test]
    fn unknown_index_state_is_rejected() {
        let error = serde_json::from_str::<DaemonIndexState>("\"publishing\"").unwrap_err();

        assert!(error.to_string().contains("unknown variant"));
    }

    #[test]
    fn string_parser_and_serde_accept_the_same_states() {
        for state in STATES {
            assert_eq!(state.as_str().parse::<DaemonIndexState>().unwrap(), state);
        }
        assert_eq!(
            "publishing".parse::<DaemonIndexState>().unwrap_err(),
            DaemonIndexStateParseError {
                value: "publishing".to_string()
            }
        );
    }
}
