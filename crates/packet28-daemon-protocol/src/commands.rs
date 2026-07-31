//! Cross-cutting daemon command payloads.

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WatchKind {
    #[serde(alias = "File")]
    File,
    #[serde(alias = "Git")]
    Git,
    #[serde(alias = "TestReport")]
    TestReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct WatchSpec {
    pub kind: WatchKind,
    pub task_id: String,
    pub root: String,
    pub paths: Vec<String>,
    pub include_globs: Vec<String>,
    pub exclude_globs: Vec<String>,
    pub debounce_ms: Option<u64>,
}

impl Default for WatchSpec {
    fn default() -> Self {
        Self {
            kind: WatchKind::File,
            task_id: String::new(),
            root: ".".to_string(),
            paths: Vec::new(),
            include_globs: Vec::new(),
            exclude_globs: Vec::new(),
            debounce_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TaskSubmitSpec {
    pub task_id: String,
    pub sequence: KernelSequenceRequest,
    pub watches: Vec<WatchSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceSubmitResponse {
    pub task_id: String,
    pub watch_ids: Vec<String>,
    pub response: KernelSequenceResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CoverCheckRequest {
    pub coverage: Vec<String>,
    pub paths: Vec<String>,
    pub format: String,
    pub issues: Vec<String>,
    pub issues_state: Option<String>,
    pub no_issues_state: bool,
    pub base: Option<String>,
    pub head: Option<String>,
    pub fail_under_total: Option<f64>,
    pub fail_under_changed: Option<f64>,
    pub fail_under_new: Option<f64>,
    pub max_new_errors: Option<u32>,
    pub max_new_warnings: Option<u32>,
    pub input: Option<String>,
    pub strip_prefix: Vec<String>,
    pub source_root: Option<String>,
    pub show_missing: bool,
    pub config_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverCheckResponse {
    pub exit_code: i32,
    pub packet_type: String,
    pub envelope: suite_packet_core::EnvelopeV1<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PacketFetchRequest {
    pub handle: String,
    pub root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PacketFetchResponse {
    pub wrapper: suite_packet_core::PacketWrapperV1<suite_packet_core::EnvelopeV1<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TestShardRequest {
    pub shards: Option<usize>,
    pub tasks_json: Option<String>,
    pub tier: String,
    pub include_tag: Vec<String>,
    pub exclude_tag: Vec<String>,
    pub tests_file: Option<String>,
    pub impact_json: Option<String>,
    pub timings: Option<String>,
    pub unknown_test_seconds: Option<f64>,
    pub algorithm: Option<String>,
    pub write_files: Option<String>,
    pub schema: bool,
    pub config_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TestShardResponse {
    pub schema: Option<String>,
    pub plan: Option<suite_packet_core::shard::ShardPlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TestMapRequest {
    pub manifest: Vec<String>,
    pub output: String,
    pub timings_output: String,
    pub schema: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TestMapSummary {
    pub manifest_files: usize,
    pub records: usize,
    pub tests: usize,
    pub files: usize,
    pub output_testmap_path: String,
    pub output_timings_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TestMapResponse {
    pub schema: Option<String>,
    pub warnings: Vec<String>,
    pub summary: Option<TestMapSummary>,
}
