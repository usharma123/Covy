use packet28_daemon_protocol::broker::{BrokerAction, BrokerPlanStep, BrokerResponseMode};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Packet28SearchResponseMode {
    #[default]
    Slim,
    Full,
}

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Packet28SearchStrategy {
    Indexed,
    Native,
    Fff,
    Recall,
    #[default]
    Hybrid,
}

impl Packet28SearchStrategy {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Hybrid => "hybrid",
            Self::Recall => "recall",
            Self::Indexed => "indexed",
            Self::Native => "native",
            Self::Fff => "fff",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28SearchArgs {
    pub(crate) task_id: String,
    pub(crate) query: String,
    pub(crate) paths: Vec<String>,
    pub(crate) fixed_string: bool,
    pub(crate) case_sensitive: Option<bool>,
    pub(crate) whole_word: bool,
    pub(crate) context_lines: Option<usize>,
    pub(crate) max_matches_per_file: Option<usize>,
    pub(crate) max_total_matches: Option<usize>,
    pub(crate) search_strategy: Packet28SearchStrategy,
    pub(crate) response_mode: Packet28SearchResponseMode,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28SearchFastArgs {
    pub(crate) query: String,
    pub(crate) paths: Vec<String>,
    pub(crate) fixed_string: bool,
    pub(crate) case_sensitive: Option<bool>,
    pub(crate) whole_word: bool,
    pub(crate) context_lines: Option<usize>,
    pub(crate) max_matches_per_file: Option<usize>,
    pub(crate) max_total_matches: Option<usize>,
    pub(crate) search_strategy: Packet28SearchStrategy,
    pub(crate) response_mode: Packet28SearchResponseMode,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28ReadRegionsArgs {
    pub(crate) task_id: String,
    pub(crate) path: String,
    pub(crate) regions: Vec<String>,
    pub(crate) line_start: Option<usize>,
    pub(crate) line_end: Option<usize>,
    pub(crate) response_mode: Packet28SearchResponseMode,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28GlobArgs {
    pub(crate) task_id: String,
    pub(crate) pattern: String,
    pub(crate) paths: Vec<String>,
    pub(crate) max_results: Option<usize>,
    pub(crate) response_mode: Packet28SearchResponseMode,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28FetchToolResultArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) invocation_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28FetchRawOutputArgs {
    pub(crate) task_id: String,
    pub(crate) handle: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28FetchContextArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
    pub(crate) response_mode: Option<BrokerResponseMode>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28PrepareHandoffArgs {
    pub(crate) task_id: String,
    pub(crate) query: Option<String>,
    pub(crate) response_mode: Option<BrokerResponseMode>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28ValidatePlanArgs {
    pub(crate) task_id: String,
    pub(crate) steps: Vec<BrokerPlanStep>,
    pub(crate) require_read_before_edit: Option<bool>,
    pub(crate) require_test_gate: Option<bool>,
    pub(crate) budget_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct Packet28ActionCriticArgs {
    pub(crate) task_id: String,
    pub(crate) action: BrokerAction,
    pub(crate) query: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) focus_paths: Vec<String>,
    pub(crate) focus_symbols: Vec<String>,
    pub(crate) budget_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28RecommendNextToolArgs {
    pub(crate) task_id: String,
    pub(crate) query: Option<String>,
    pub(crate) focus_paths: Vec<String>,
    pub(crate) focus_symbols: Vec<String>,
    pub(crate) max_recommendations: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28ValidateToolOutcomeArgs {
    pub(crate) task_id: String,
    pub(crate) command: Option<String>,
    pub(crate) focus_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28PatchRiskArgs {
    pub(crate) task_id: String,
    pub(crate) paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28VerifyHandoffArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28PromptPressureArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
    pub(crate) next_prompt: Option<String>,
    pub(crate) budget_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffDiffArgs {
    pub(crate) task_id: String,
    pub(crate) left_artifact_id: Option<String>,
    pub(crate) left_context_version: Option<String>,
    pub(crate) right_artifact_id: Option<String>,
    pub(crate) right_context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffCompressionArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
    pub(crate) next_prompt: Option<String>,
    pub(crate) budget_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffDependencyLintArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffPathLintArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffTestLintArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffStaleCommandLintArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffEnvironmentLintArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffLintAllArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffFixPlanArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_id: Option<String>,
    pub(crate) context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffRepairVerifyArgs {
    pub(crate) task_id: String,
    pub(crate) before_artifact_id: Option<String>,
    pub(crate) before_context_version: Option<String>,
    pub(crate) after_artifact_id: Option<String>,
    pub(crate) after_context_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffLintTrendArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_ids: Vec<String>,
    pub(crate) max_artifacts: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct Packet28HandoffLintRegressionArgs {
    pub(crate) task_id: String,
    pub(crate) artifact_ids: Vec<String>,
    pub(crate) max_artifacts: Option<usize>,
}

impl Default for Packet28ActionCriticArgs {
    fn default() -> Self {
        Self {
            task_id: String::new(),
            action: BrokerAction::ChooseTool,
            query: None,
            tool_name: None,
            focus_paths: Vec::new(),
            focus_symbols: Vec::new(),
            budget_tokens: None,
        }
    }
}
