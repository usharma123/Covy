//! Serializable context-memory records and query results.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{MemoryKind, MemorySourceTier};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EvictionReason {
    ExpiredTtl,
    ManualPrune,
    VersionMismatch,
    CorruptLoadRecovery,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EvictionCounters {
    pub expired_ttl: usize,
    pub manual_prune: usize,
    pub version_mismatch: usize,
    pub corrupt_load_recovery: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ContextStoreEntrySummary {
    pub cache_key: String,
    pub target: String,
    pub input_hash: String,
    pub created_at_unix: u64,
    pub age_secs: u64,
    pub packet_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextStoreEntryDetail {
    pub entry: PacketCacheEntry,
    pub age_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ContextStoreStats {
    pub entries: usize,
    pub oldest_created_at_unix: Option<u64>,
    pub newest_created_at_unix: Option<u64>,
    pub evictions: EvictionCounters,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ContextStorePruneReport {
    pub removed: usize,
    pub remaining: usize,
    pub reasons: EvictionCounters,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Default, wincode::SchemaRead, wincode::SchemaWrite,
)]
#[serde(default)]
pub struct RecallBudgetEstimate {
    pub est_tokens: u64,
    pub est_bytes: u64,
    pub runtime_ms: u64,
}

pub type RecallSourceTier = MemorySourceTier;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecallMode {
    #[default]
    Auto,
    Conceptual,
    Telemetry,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RecallHit {
    pub cache_key: String,
    pub target: String,
    pub created_at_unix: u64,
    pub age_secs: u64,
    pub score: f64,
    pub summary: Option<String>,
    pub snippet: String,
    pub matched_tokens: Vec<String>,
    pub matched_paths: Vec<String>,
    pub matched_symbols: Vec<String>,
    pub match_reasons: Vec<String>,
    pub packet_types: Vec<String>,
    pub task_ids: Vec<String>,
    pub budget_estimate: RecallBudgetEstimate,
    pub source_tier: RecallSourceTier,
    pub memory_kind: MemoryKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CachePacket {
    pub packet_id: Option<String>,
    pub body: Value,
    pub token_usage: Option<u64>,
    pub runtime_ms: Option<u64>,
    pub metadata: Value,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Default, wincode::SchemaRead, wincode::SchemaWrite,
)]
#[serde(default)]
pub struct DeltaReuse {
    pub reused_from: Option<String>,
    pub delta_ratio: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PacketCacheEntry {
    pub cache_key: String,
    pub target: String,
    pub input_hash: String,
    pub created_at_unix: u64,
    pub packets: Vec<CachePacket>,
    pub metadata: Value,
    pub delta_reuse: DeltaReuse,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn recall_hit_json_shape_is_stable() {
        let hit = RecallHit {
            cache_key: "cache-1".to_string(),
            target: "context.assemble".to_string(),
            source_tier: MemorySourceTier::CuratedMemory,
            memory_kind: MemoryKind::Evidence,
            ..RecallHit::default()
        };

        assert_eq!(
            serde_json::to_value(hit).unwrap(),
            json!({
                "cache_key": "cache-1",
                "target": "context.assemble",
                "created_at_unix": 0,
                "age_secs": 0,
                "score": 0.0,
                "summary": null,
                "snippet": "",
                "matched_tokens": [],
                "matched_paths": [],
                "matched_symbols": [],
                "match_reasons": [],
                "packet_types": [],
                "task_ids": [],
                "budget_estimate": {
                    "est_tokens": 0,
                    "est_bytes": 0,
                    "runtime_ms": 0
                },
                "source_tier": "curated_memory",
                "memory_kind": "evidence"
            })
        );
    }
}
