//! Serializable context-kernel requests and responses.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::governance::AuditResult;
use crate::AgentStateEventKind;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ExecutionBudget {
    pub token_cap: Option<u64>,
    pub byte_cap: Option<usize>,
    pub runtime_ms_cap: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct BudgetUsage {
    pub tokens: u64,
    pub bytes: usize,
    pub runtime_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KernelPacket {
    pub packet_id: Option<String>,
    pub format: String,
    pub body: Value,
    pub token_usage: Option<u64>,
    pub runtime_ms: Option<u64>,
    pub metadata: Value,
}

impl Default for KernelPacket {
    fn default() -> Self {
        Self {
            packet_id: None,
            format: "packet-json".to_string(),
            body: Value::Null,
            token_usage: None,
            runtime_ms: None,
            metadata: Value::Null,
        }
    }
}

impl KernelPacket {
    pub fn from_value(value: Value, fallback_packet_id: Option<String>) -> Self {
        let packet_id = value
            .get("packet_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or(fallback_packet_id);

        Self {
            packet_id,
            format: "packet-json".to_string(),
            body: value,
            token_usage: None,
            runtime_ms: None,
            metadata: Value::Null,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KernelRequest {
    pub target: String,
    pub input_packets: Vec<KernelPacket>,
    pub budget: ExecutionBudget,
    pub policy_context: Value,
    pub reducer_input: Value,
}

impl Default for KernelRequest {
    fn default() -> Self {
        Self {
            target: String::new(),
            input_packets: Vec::new(),
            budget: ExecutionBudget::default(),
            policy_context: Value::Null,
            reducer_input: Value::Null,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct KernelStepRequest {
    pub id: String,
    pub target: String,
    pub depends_on: Vec<String>,
    pub input_packets: Vec<KernelPacket>,
    pub policy_context: Value,
    pub reducer_input: Value,
    pub budget: ExecutionBudget,
    pub reactive: Option<KernelStepReactiveConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct KernelSequenceRequest {
    pub budget: ExecutionBudget,
    pub steps: Vec<KernelStepRequest>,
    pub reactive: ReactiveSequenceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct ReactiveSequenceConfig {
    pub enabled: bool,
    pub task_id: Option<String>,
    pub append_focused_map: bool,
    pub mode: ReactiveReplanMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReactiveReplanMode {
    #[default]
    Basic,
    TaskAware,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct KernelStepReactiveConfig {
    pub event_kinds: Vec<AgentStateEventKind>,
    pub path_globs: Vec<String>,
    pub rerun_on_focus_change: bool,
    pub skip_if_inputs_unchanged: bool,
    pub produces_focus: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelStepResponse {
    pub id: String,
    pub target: String,
    pub status: String,
    pub response: Option<KernelResponse>,
    pub failure: Option<KernelFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelSequenceResponse {
    pub request_id: u64,
    pub scheduled: Vec<String>,
    pub skipped: Vec<String>,
    pub budget_exhausted: bool,
    pub step_results: Vec<KernelStepResponse>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelResponse {
    pub request_id: u64,
    pub target: String,
    pub output_packets: Vec<KernelPacket>,
    pub audit: KernelAudit,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelAudit {
    pub reducer: String,
    pub input_packets: usize,
    pub output_packets: usize,
    pub budget: ExecutionBudget,
    pub input_usage: BudgetUsage,
    pub output_usage: BudgetUsage,
    pub total_usage: BudgetUsage,
    #[serde(default)]
    pub governance: GovernanceAudit,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GovernanceAudit {
    pub enabled: bool,
    pub config_path: Option<String>,
    pub reducer_execution: Option<ReducerExecutionAudit>,
    pub input_audits: Vec<AuditResult>,
    pub output_audits: Vec<AuditResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ReducerExecutionAudit {
    pub reducer: String,
    pub allowed: bool,
    pub matched_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ReducerResult {
    pub output_packets: Vec<KernelPacket>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelFailure {
    pub code: String,
    pub message: String,
    pub target: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn kernel_request_json_shape_is_stable() {
        let request = KernelRequest {
            target: "context.assemble".to_string(),
            input_packets: vec![KernelPacket::from_value(
                json!({"packet_id": "packet-1", "value": 7}),
                None,
            )],
            budget: ExecutionBudget {
                token_cap: Some(40),
                byte_cap: Some(160),
                runtime_ms_cap: Some(25),
            },
            policy_context: json!({"mode": "strict"}),
            reducer_input: json!({"task_id": "task-1"}),
        };

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "target": "context.assemble",
                "input_packets": [{
                    "packet_id": "packet-1",
                    "format": "packet-json",
                    "body": {"packet_id": "packet-1", "value": 7},
                    "token_usage": null,
                    "runtime_ms": null,
                    "metadata": null
                }],
                "budget": {
                    "token_cap": 40,
                    "byte_cap": 160,
                    "runtime_ms_cap": 25
                },
                "policy_context": {"mode": "strict"},
                "reducer_input": {"task_id": "task-1"}
            })
        );
    }
}
