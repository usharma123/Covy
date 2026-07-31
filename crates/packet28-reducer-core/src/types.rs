use serde::{Deserialize, Serialize};
pub use suite_packet_core::search::{
    SearchEngineStats, SearchGroup, SearchMatch, SearchRequest, SearchResult,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct ReadRegionsRequest {
    pub path: String,
    pub regions: Vec<String>,
    pub line_start: Option<usize>,
    pub line_end: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct ReadLine {
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct ReadRegionsResult {
    pub path: String,
    pub regions: Vec<String>,
    pub symbols: Vec<String>,
    pub lines: Vec<ReadLine>,
    pub compact_preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandReducerFamily {
    Git,
    Fs,
    Rust,
    Github,
    Python,
    Javascript,
    Go,
    Infra,
    Ruby,
    Dotnet,
    Jvm,
}

impl CommandReducerFamily {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Fs => "fs",
            Self::Rust => "rust",
            Self::Github => "github",
            Self::Python => "python",
            Self::Javascript => "javascript",
            Self::Go => "go",
            Self::Infra => "infra",
            Self::Ruby => "ruby",
            Self::Dotnet => "dotnet",
            Self::Jvm => "jvm",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct CommandReducerSpec {
    pub family: String,
    pub canonical_kind: String,
    pub packet_type: String,
    pub operation_kind: suite_packet_core::ToolOperationKind,
    pub command: String,
    pub argv: Vec<String>,
    pub cache_fingerprint: String,
    pub cacheable: bool,
    pub mutation: bool,
    pub paths: Vec<String>,
    pub equivalence_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct CommandReduction {
    pub family: String,
    pub canonical_kind: String,
    pub packet_type: String,
    pub operation_kind: suite_packet_core::ToolOperationKind,
    pub summary: String,
    /// Condensed readable preview (e.g. RTK-style compact diff) for agent context.
    pub compact_preview: String,
    pub paths: Vec<String>,
    pub regions: Vec<String>,
    pub symbols: Vec<String>,
    pub failed: bool,
    pub error_class: Option<String>,
    pub error_message: Option<String>,
    pub retryable: Option<bool>,
    pub exit_code: i32,
    pub cache_fingerprint: String,
    pub cacheable: bool,
    pub mutation: bool,
    pub equivalence_key: Option<String>,
}
