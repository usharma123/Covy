use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryRecord {
    pub(crate) id: i64,
    pub(crate) content: String,
    pub(crate) tags: Option<String>,
    pub(crate) topic: String,
    pub(crate) importance: String,
    pub(crate) keywords: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) raw_excerpt: Option<String>,
    pub(crate) weight: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) recall_score: Option<f64>,
    pub(crate) created_at_unix_ms: i64,
    pub(crate) updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FeedbackRecord {
    pub(crate) id: i64,
    pub(crate) subject: String,
    pub(crate) correction: String,
    pub(crate) topic: String,
    pub(crate) context: Option<String>,
    pub(crate) predicted: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) applied_count: i64,
    pub(crate) created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TranscriptSession {
    pub(crate) id: i64,
    pub(crate) session_key: String,
    pub(crate) agent: Option<String>,
    pub(crate) message_count: i64,
    pub(crate) started_at_unix_ms: i64,
    pub(crate) updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TranscriptMessage {
    pub(crate) id: i64,
    pub(crate) session_id: i64,
    pub(crate) session_key: String,
    pub(crate) agent: Option<String>,
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) source: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TranscriptStats {
    pub(crate) session_count: i64,
    pub(crate) message_count: i64,
    pub(crate) agent_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphConcept {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) memoir_name: String,
    pub(crate) labels: Vec<String>,
    pub(crate) confidence: f64,
    pub(crate) revision: i64,
    pub(crate) source_ids: Vec<String>,
    pub(crate) created_at_unix_ms: i64,
    pub(crate) updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphRelation {
    pub(crate) id: i64,
    pub(crate) source: String,
    pub(crate) target: String,
    pub(crate) relation: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphInspect {
    pub(crate) concepts: Vec<GraphConcept>,
    pub(crate) relations: Vec<GraphRelation>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphMemoir {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) concept_count: i64,
    pub(crate) relation_count: i64,
    pub(crate) average_confidence: f64,
    pub(crate) created_at_unix_ms: i64,
    pub(crate) updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphMemoirShow {
    pub(crate) memoir: GraphMemoir,
    pub(crate) concepts: Vec<GraphConcept>,
    pub(crate) relations: Vec<GraphRelation>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphConceptInspect {
    pub(crate) concept: GraphConcept,
    pub(crate) depth: usize,
    pub(crate) neighbors: Vec<GraphConcept>,
    pub(crate) relations: Vec<GraphRelation>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphDistillReport {
    pub(crate) topic: String,
    pub(crate) memoir: String,
    pub(crate) source_memory_count: usize,
    pub(crate) created_count: usize,
    pub(crate) refined_count: usize,
    pub(crate) concepts: Vec<GraphConcept>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphDeleteReport {
    pub(crate) deleted_concepts: usize,
    pub(crate) deleted_relations: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphExport {
    pub(crate) format: String,
    pub(crate) content: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphRelationTypeStats {
    pub(crate) relation: String,
    pub(crate) relation_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphStats {
    pub(crate) concept_count: i64,
    pub(crate) relation_count: i64,
    pub(crate) relation_type_count: i64,
    pub(crate) isolated_concept_count: i64,
    pub(crate) relation_types: Vec<GraphRelationTypeStats>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProjectLearnReport {
    pub(crate) project_name: String,
    pub(crate) project_root: String,
    pub(crate) memoir_name: String,
    pub(crate) total_concepts: usize,
    pub(crate) link_count: usize,
    pub(crate) concepts: Vec<GraphConcept>,
    pub(crate) relations: Vec<GraphRelation>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LocalStoreStats {
    pub(crate) memory_count: i64,
    pub(crate) memory_embedding_count: i64,
    pub(crate) feedback_count: i64,
    pub(crate) concept_count: i64,
    pub(crate) relation_count: i64,
    pub(crate) transcript_session_count: i64,
    pub(crate) transcript_message_count: i64,
    pub(crate) mcp_call_count: i64,
    pub(crate) hook_event_count: i64,
    pub(crate) pending_extraction_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PendingExtractionRecord {
    pub(crate) id: i64,
    pub(crate) project: String,
    pub(crate) tool_name: String,
    pub(crate) raw_output: String,
    pub(crate) captured_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PendingExtractionProcessReport {
    pub(crate) pending_count: usize,
    pub(crate) extracted_count: usize,
    pub(crate) deleted_count: usize,
    pub(crate) dry_run: bool,
    pub(crate) facts: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HookEventRecord {
    pub(crate) id: i64,
    pub(crate) runtime: String,
    pub(crate) event_kind: String,
    pub(crate) session_id: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) matcher: Option<String>,
    pub(crate) payload_json: String,
    pub(crate) created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HookEventStats {
    pub(crate) runtime: String,
    pub(crate) event_kind: String,
    pub(crate) event_count: i64,
}

pub(crate) struct HookEventInput<'a> {
    pub(crate) runtime: &'a str,
    pub(crate) event_kind: &'a str,
    pub(crate) session_id: Option<&'a str>,
    pub(crate) task_id: Option<&'a str>,
    pub(crate) matcher: Option<&'a str>,
    pub(crate) payload_json: &'a str,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingExtractionInput<'a> {
    pub(crate) project: Option<&'a str>,
    pub(crate) tool_name: Option<&'a str>,
    pub(crate) raw_output: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryTopicStats {
    pub(crate) topic: String,
    pub(crate) memory_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryHealthTopic {
    pub(crate) topic: String,
    pub(crate) memory_count: i64,
    pub(crate) avg_weight: f64,
    pub(crate) avg_access_count: f64,
    pub(crate) stale_count: i64,
    pub(crate) oldest_age_days: i64,
    pub(crate) newest_age_days: i64,
    pub(crate) consolidation_needed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryHealthReport {
    pub(crate) topic_filter: Option<String>,
    pub(crate) stale_after_days: i64,
    pub(crate) consolidation_threshold: i64,
    pub(crate) total_topics: usize,
    pub(crate) total_memories: i64,
    pub(crate) stale_memories: i64,
    pub(crate) topics_needing_consolidation: i64,
    pub(crate) topics: Vec<MemoryHealthTopic>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryConsolidationReport {
    pub(crate) topic: String,
    pub(crate) source_count: usize,
    pub(crate) status: String,
    pub(crate) keep_originals: bool,
    pub(crate) consolidated_memory: Option<MemoryRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryDecayReport {
    pub(crate) factor: f64,
    pub(crate) decayed_count: usize,
    pub(crate) skipped_critical_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryPruneReport {
    pub(crate) threshold: f64,
    pub(crate) dry_run: bool,
    pub(crate) candidate_count: usize,
    pub(crate) deleted_count: usize,
    pub(crate) skipped_protected_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryEmbeddingRecord {
    pub(crate) memory_id: i64,
    pub(crate) model: String,
    pub(crate) dimensions: usize,
    pub(crate) embedding_json: String,
    pub(crate) created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryEmbedReport {
    pub(crate) model: String,
    pub(crate) dimensions: usize,
    pub(crate) embedded_count: usize,
    pub(crate) embeddings: Vec<MemoryEmbeddingRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryPattern {
    pub(crate) key: String,
    pub(crate) memory_count: usize,
    pub(crate) memory_ids: Vec<i64>,
    pub(crate) keywords: Vec<String>,
    pub(crate) sample_contents: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryPatternReport {
    pub(crate) topic: String,
    pub(crate) min_cluster_size: usize,
    pub(crate) memoir: Option<String>,
    pub(crate) source_memory_count: usize,
    pub(crate) pattern_count: usize,
    pub(crate) patterns: Vec<MemoryPattern>,
    pub(crate) created_concepts: Vec<GraphConcept>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryLintIssue {
    pub(crate) memory_id: i64,
    pub(crate) kind: String,
    pub(crate) detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MemoryLintReport {
    pub(crate) memory_count: usize,
    pub(crate) hook_runtime_count: usize,
    pub(crate) issue_count: usize,
    pub(crate) issues: Vec<MemoryLintIssue>,
    pub(crate) summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FeedbackStats {
    pub(crate) feedback_count: i64,
    pub(crate) applied_count: i64,
    pub(crate) topic_count: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct TranscriptAppendInput<'a> {
    pub(crate) session: Option<&'a str>,
    pub(crate) agent: Option<&'a str>,
    pub(crate) role: Option<&'a str>,
    pub(crate) content: &'a str,
    pub(crate) source: Option<&'a str>,
    pub(crate) project: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(crate) struct FeedbackInput<'a> {
    pub(crate) subject: &'a str,
    pub(crate) correction: &'a str,
    pub(crate) topic: Option<&'a str>,
    pub(crate) context: Option<&'a str>,
    pub(crate) predicted: Option<&'a str>,
    pub(crate) reason: Option<&'a str>,
    pub(crate) source: Option<&'a str>,
    pub(crate) project: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryStoreInput<'a> {
    pub(crate) content: &'a str,
    pub(crate) tags: Option<&'a str>,
    pub(crate) topic: Option<&'a str>,
    pub(crate) importance: Option<&'a str>,
    pub(crate) keywords: Option<&'a str>,
    pub(crate) project: Option<&'a str>,
    pub(crate) source: Option<&'a str>,
    pub(crate) raw_excerpt: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryUpdateInput<'a> {
    pub(crate) id: i64,
    pub(crate) content: Option<&'a str>,
    pub(crate) tags: Option<&'a str>,
    pub(crate) topic: Option<&'a str>,
    pub(crate) importance: Option<&'a str>,
    pub(crate) keywords: Option<&'a str>,
    pub(crate) project: Option<&'a str>,
    pub(crate) source: Option<&'a str>,
    pub(crate) raw_excerpt: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MemoryRecallQuery<'a> {
    pub(crate) query: &'a str,
    pub(crate) limit: usize,
    pub(crate) topic: Option<&'a str>,
    pub(crate) project: Option<&'a str>,
    pub(crate) tag: Option<&'a str>,
    pub(crate) keyword: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MemoryListQuery<'a> {
    pub(crate) limit: usize,
    pub(crate) topic: Option<&'a str>,
    pub(crate) project: Option<&'a str>,
    pub(crate) all: bool,
    pub(crate) sort: &'a str,
}
