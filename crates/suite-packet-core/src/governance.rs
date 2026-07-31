//! Serializable governance results shared across runtime boundaries.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditResult {
    pub passed: bool,
    pub policy_version: u32,
    pub checked_at_unix: u64,
    pub totals: AuditTotals,
    pub findings: Vec<AuditFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditTotals {
    pub tools_seen: usize,
    pub reducers_seen: usize,
    pub paths_seen: usize,
    pub total_token_usage: u64,
    pub total_runtime_ms: u64,
    pub total_tool_calls: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditFinding {
    pub rule: String,
    pub subject: String,
    pub message: String,
}
