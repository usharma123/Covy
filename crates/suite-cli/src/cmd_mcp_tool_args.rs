use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct MemoryStoreToolArgs {
    pub(super) content: String,
    pub(super) tags: Option<String>,
    pub(super) topic: Option<String>,
    pub(super) importance: Option<String>,
    pub(super) keywords: Option<String>,
    pub(super) project: Option<String>,
    pub(super) source: Option<String>,
    pub(super) raw_excerpt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryRecallToolArgs {
    pub(super) query: String,
    pub(super) limit: Option<usize>,
    pub(super) topic: Option<String>,
    pub(super) project: Option<String>,
    pub(super) tag: Option<String>,
    pub(super) keyword: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryListToolArgs {
    pub(super) limit: Option<usize>,
    pub(super) topic: Option<String>,
    pub(super) project: Option<String>,
    pub(super) all: Option<bool>,
    pub(super) sort: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryUpdateToolArgs {
    pub(super) id: i64,
    pub(super) content: Option<String>,
    pub(super) tags: Option<String>,
    pub(super) topic: Option<String>,
    pub(super) importance: Option<String>,
    pub(super) keywords: Option<String>,
    pub(super) project: Option<String>,
    pub(super) source: Option<String>,
    pub(super) raw_excerpt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryForgetToolArgs {
    pub(super) id: Option<i64>,
    pub(super) topic: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryHealthToolArgs {
    pub(super) topic: Option<String>,
    pub(super) stale_after_days: Option<i64>,
    pub(super) consolidation_threshold: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryLintToolArgs {
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct VerifyContextAnomaliesToolArgs {
    pub(super) max_anomalies: Option<usize>,
    pub(super) max_high: Option<usize>,
    pub(super) max_trend_age_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryConsolidateToolArgs {
    pub(super) topic: Option<String>,
    pub(super) keep_originals: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryDecayToolArgs {
    pub(super) factor: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryPruneToolArgs {
    pub(super) threshold: Option<f64>,
    pub(super) dry_run: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryEmbedToolArgs {
    pub(super) id: Option<i64>,
    pub(super) all: Option<bool>,
    pub(super) dimensions: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryExtractPatternsToolArgs {
    pub(super) topic: String,
    pub(super) memoir: Option<String>,
    pub(super) min_cluster_size: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryPendingEnqueueToolArgs {
    pub(super) raw_output: String,
    pub(super) project: Option<String>,
    pub(super) tool_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryPendingListToolArgs {
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryPendingProcessToolArgs {
    pub(super) limit: Option<usize>,
    pub(super) dry_run: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemoryPendingDeleteToolArgs {
    pub(super) ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct FeedbackRecordToolArgs {
    pub(super) subject: String,
    pub(super) correction: String,
    pub(super) topic: Option<String>,
    pub(super) context: Option<String>,
    pub(super) predicted: Option<String>,
    pub(super) reason: Option<String>,
    pub(super) source: Option<String>,
    pub(super) project: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct FeedbackSearchToolArgs {
    pub(super) query: String,
    pub(super) limit: Option<usize>,
    pub(super) project: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct FeedbackListToolArgs {
    pub(super) topic: Option<String>,
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct FeedbackIdToolArgs {
    pub(super) id: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct WakeupToolArgs {
    pub(super) query: Option<String>,
    pub(super) project: Option<String>,
    pub(super) path: Option<Vec<String>>,
    pub(super) paths: Option<Vec<String>>,
    pub(super) symbol: Option<Vec<String>>,
    pub(super) symbols: Option<Vec<String>>,
    pub(super) intent: Option<String>,
    pub(super) limit: Option<usize>,
    pub(super) max_tokens: Option<usize>,
    pub(super) format: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LearnProjectToolArgs {
    pub(super) directory: Option<String>,
    pub(super) name: Option<String>,
    pub(super) memoir: Option<String>,
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TranscriptAppendToolArgs {
    pub(super) content: String,
    pub(super) session: Option<String>,
    pub(super) agent: Option<String>,
    pub(super) role: Option<String>,
    pub(super) source: Option<String>,
    pub(super) project: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TranscriptListToolArgs {
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TranscriptShowToolArgs {
    pub(super) session: String,
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TranscriptSearchToolArgs {
    pub(super) query: String,
    pub(super) limit: Option<usize>,
    pub(super) project: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TranscriptExportToolArgs {
    pub(super) session: Option<String>,
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TranscriptImportToolArgs {
    pub(super) content: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphCreateToolArgs {
    pub(super) name: Option<String>,
    pub(super) description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphShowToolArgs {
    pub(super) name: Option<String>,
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphConceptToolArgs {
    pub(super) name: String,
    pub(super) description: Option<String>,
    pub(super) memoir: Option<String>,
    pub(super) labels: Option<Vec<String>>,
    pub(super) confidence: Option<f64>,
    pub(super) source_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphRefineToolArgs {
    pub(super) name: String,
    pub(super) description: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphLinkToolArgs {
    pub(super) source: String,
    pub(super) target: String,
    pub(super) relation: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphSearchToolArgs {
    pub(super) query: String,
    pub(super) memoir: Option<String>,
    pub(super) label: Option<String>,
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphExportToolArgs {
    pub(super) format: Option<String>,
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphDeleteToolArgs {
    pub(super) name: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphInspectToolArgs {
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphInspectConceptToolArgs {
    pub(super) name: String,
    pub(super) memoir: Option<String>,
    pub(super) depth: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphDistillToolArgs {
    pub(super) from_topic: String,
    pub(super) into: Option<String>,
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ReduceToolArgs {
    pub(super) command: String,
    pub(super) stdout: Option<String>,
    pub(super) stderr: Option<String>,
    pub(super) exit_code: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RewriteToolArgs {
    pub(super) command: String,
    pub(super) task_id: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DoctorToolArgs {
    pub(super) agent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct VerifyExperimentsToolArgs {
    pub(super) manifest: Option<String>,
    pub(super) require_workflows: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ReducerDriftToolArgs {
    pub(super) fixture: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct HypothesisAddToolArgs {
    pub(super) task_id: Option<String>,
    pub(super) id: Option<String>,
    pub(super) text: String,
    pub(super) paths: Option<Vec<String>>,
    pub(super) symbols: Option<Vec<String>>,
    pub(super) artifact_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct HypothesisListToolArgs {
    pub(super) task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct HypothesisResolveToolArgs {
    pub(super) task_id: Option<String>,
    pub(super) id: String,
    pub(super) status: String,
    pub(super) note: Option<String>,
}
