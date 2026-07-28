use std::path::Path;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::cmd_mcp::tool_args::*;
use crate::cmd_transcript::{export_transcripts, import_transcripts_from_str};
use crate::cmd_wakeup::{build_wakeup_report_scoped, WakeupScope};
use crate::memory_store::{
    add_concept_with_metadata, append_transcript_message, apply_feedback, consolidate_memories,
    create_graph_memoir, decay_memories, delete_concept, delete_feedback,
    delete_pending_extractions, distill_memories_to_graph, embed_memories,
    enqueue_pending_extraction, export_graph, extract_memory_patterns, feedback_stats,
    forget_memories_by_topic, forget_memory, graph_stats, inspect_graph, inspect_graph_concept,
    learn_project_graph, link_concepts, lint_memories, list_feedback, list_graph_memoirs,
    list_memories_filtered, list_pending_extractions, list_transcript_sessions, local_store_stats,
    memory_health, memory_topics, process_pending_extractions, prune_memories,
    recall_memories_filtered, record_feedback_with_metadata, refine_concept,
    search_concepts_filtered, search_feedback_filtered, search_transcripts_filtered,
    show_graph_memoir, show_transcript_session, store_memory_with_metadata, transcript_stats,
    update_memory, FeedbackInput, MemoryListQuery, MemoryRecallQuery, MemoryStoreInput,
    MemoryUpdateInput, PendingExtractionInput, TranscriptAppendInput,
};

pub(super) fn handle_memory_tool_call(
    root: &Path,
    name: &str,
    arguments: &Value,
) -> Result<Option<Value>> {
    let payload = match name {
        "packet28.memory_store" => {
            let request: MemoryStoreToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(store_memory_with_metadata(MemoryStoreInput {
                content: &request.content,
                tags: request.tags.as_deref(),
                topic: request.topic.as_deref(),
                importance: request.importance.as_deref(),
                keywords: request.keywords.as_deref(),
                project: request.project.as_deref(),
                source: request.source.as_deref(),
                raw_excerpt: request.raw_excerpt.as_deref(),
            })?)?
        }
        "packet28.memory_recall" => {
            let request: MemoryRecallToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(recall_memories_filtered(MemoryRecallQuery {
                query: &request.query,
                limit: request.limit.unwrap_or(10),
                topic: request.topic.as_deref(),
                project: request.project.as_deref(),
                tag: request.tag.as_deref(),
                keyword: request.keyword.as_deref(),
            })?)?
        }
        "packet28.memory_list" => {
            let request: MemoryListToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(list_memories_filtered(MemoryListQuery {
                limit: request.limit.unwrap_or(20),
                topic: request.topic.as_deref(),
                project: request.project.as_deref(),
                all: request.all.unwrap_or(false),
                sort: request.sort.as_deref().unwrap_or("recent"),
            })?)?
        }
        "packet28.memory_update" => {
            let request: MemoryUpdateToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(update_memory(MemoryUpdateInput {
                id: request.id,
                content: request.content.as_deref(),
                tags: request.tags.as_deref(),
                topic: request.topic.as_deref(),
                importance: request.importance.as_deref(),
                keywords: request.keywords.as_deref(),
                project: request.project.as_deref(),
                source: request.source.as_deref(),
                raw_excerpt: request.raw_excerpt.as_deref(),
            })?)?
        }
        "packet28.memory_forget" => {
            let request: MemoryForgetToolArgs = serde_json::from_value(arguments.clone())?;
            let deleted = match (request.id, request.topic.as_deref()) {
                (Some(id), None) => forget_memory(id)?,
                (None, Some(topic)) => forget_memories_by_topic(topic)?,
                _ => return Err(anyhow!("pass exactly one of id or topic")),
            };
            json!({ "deleted": deleted })
        }
        "packet28.memory_topics" => serde_json::to_value(memory_topics()?)?,
        "packet28.memory_stats" => serde_json::to_value(local_store_stats()?)?,
        "packet28.memory_health" => {
            let request: MemoryHealthToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(memory_health(
                request.topic.as_deref(),
                request.stale_after_days.unwrap_or(30),
                request.consolidation_threshold.unwrap_or(10),
            )?)?
        }
        "packet28.memory_lint" => {
            let request: MemoryLintToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(lint_memories(root, request.limit.unwrap_or(200))?)?
        }
        "packet28.context_anomalies" => {
            serde_json::to_value(crate::cmd_dashboard::context_anomaly_digest(root)?)?
        }
        "packet28.verify_context_anomalies" => {
            let request: VerifyContextAnomaliesToolArgs =
                serde_json::from_value(arguments.clone())?;
            crate::cmd_verify::verify_context_anomalies_payload(
                root,
                request.max_anomalies.unwrap_or(999),
                request.max_high.unwrap_or(0),
                request.max_trend_age_ms,
            )?
        }
        "packet28.memory_consolidate" => {
            let request: MemoryConsolidateToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(consolidate_memories(
                request.topic.as_deref(),
                request.keep_originals.unwrap_or(false),
            )?)?
        }
        "packet28.memory_decay" => {
            let request: MemoryDecayToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(decay_memories(request.factor.unwrap_or(0.95))?)?
        }
        "packet28.memory_prune" => {
            let request: MemoryPruneToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(prune_memories(
                request.threshold.unwrap_or(0.1),
                request.dry_run.unwrap_or(false),
            )?)?
        }
        "packet28.memory_embed" => {
            let request: MemoryEmbedToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(embed_memories(
                request.id,
                request.all.unwrap_or(false),
                request.dimensions.unwrap_or(384),
            )?)?
        }
        "packet28.memory_extract_patterns" => {
            let request: MemoryExtractPatternsToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(extract_memory_patterns(
                &request.topic,
                request.memoir.as_deref(),
                request.min_cluster_size.unwrap_or(3),
            )?)?
        }
        "packet28.memory_pending_enqueue" => {
            let request: MemoryPendingEnqueueToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(enqueue_pending_extraction(PendingExtractionInput {
                project: request.project.as_deref(),
                tool_name: request.tool_name.as_deref(),
                raw_output: &request.raw_output,
            })?)?
        }
        "packet28.memory_pending_list" => {
            let request: MemoryPendingListToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(list_pending_extractions(request.limit.unwrap_or(20))?)?
        }
        "packet28.memory_pending_process" => {
            let request: MemoryPendingProcessToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(process_pending_extractions(
                request.limit.unwrap_or(20),
                request.dry_run.unwrap_or(false),
            )?)?
        }
        "packet28.memory_pending_delete" => {
            let request: MemoryPendingDeleteToolArgs = serde_json::from_value(arguments.clone())?;
            json!({ "deleted": delete_pending_extractions(&request.ids)? })
        }
        "packet28.memory_pending_stats" => {
            let stats = local_store_stats()?;
            json!({ "pending_extraction_count": stats.pending_extraction_count })
        }
        "packet28.feedback_record" => {
            let request: FeedbackRecordToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(record_feedback_with_metadata(FeedbackInput {
                subject: &request.subject,
                correction: &request.correction,
                topic: request.topic.as_deref(),
                context: request.context.as_deref(),
                predicted: request.predicted.as_deref(),
                reason: request.reason.as_deref(),
                source: request.source.as_deref(),
                project: request.project.as_deref(),
            })?)?
        }
        "packet28.feedback_search" => {
            let request: FeedbackSearchToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(search_feedback_filtered(
                &request.query,
                request.project.as_deref(),
                request.limit.unwrap_or(10),
            )?)?
        }
        "packet28.feedback_list" => {
            let request: FeedbackListToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(list_feedback(
                request.topic.as_deref(),
                request.limit.unwrap_or(20),
            )?)?
        }
        "packet28.feedback_apply" => {
            let request: FeedbackIdToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(apply_feedback(request.id)?)?
        }
        "packet28.feedback_delete" => {
            let request: FeedbackIdToolArgs = serde_json::from_value(arguments.clone())?;
            json!({ "deleted": delete_feedback(request.id)? })
        }
        "packet28.feedback_stats" => serde_json::to_value(feedback_stats()?)?,
        "packet28.wakeup" => {
            let request: WakeupToolArgs = serde_json::from_value(arguments.clone())?;
            let paths = request
                .paths
                .as_ref()
                .or(request.path.as_ref())
                .cloned()
                .unwrap_or_default();
            let symbols = request
                .symbols
                .as_ref()
                .or(request.symbol.as_ref())
                .cloned()
                .unwrap_or_default();
            serde_json::to_value(build_wakeup_report_scoped(
                request.query.as_deref(),
                request.project.as_deref(),
                WakeupScope {
                    paths: paths.iter().map(String::as_str).collect(),
                    symbols: symbols.iter().map(String::as_str).collect(),
                    intent: request.intent.as_deref(),
                },
                request.limit.unwrap_or(5),
                request.max_tokens.unwrap_or(500),
                request.format.as_deref().unwrap_or("markdown"),
            )?)?
        }
        "packet28.learn_project" => {
            let request: LearnProjectToolArgs = serde_json::from_value(arguments.clone())?;
            let dir = request
                .directory
                .unwrap_or_else(|| root.display().to_string());
            serde_json::to_value(learn_project_graph(
                Path::new(&dir),
                request.name.as_deref(),
                request.memoir.as_deref(),
                request.limit.unwrap_or(20),
            )?)?
        }
        "packet28.transcript_append" => {
            let request: TranscriptAppendToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(append_transcript_message(TranscriptAppendInput {
                session: request.session.as_deref(),
                agent: request.agent.as_deref(),
                role: request.role.as_deref(),
                content: &request.content,
                source: request.source.as_deref(),
                project: request.project.as_deref(),
            })?)?
        }
        "packet28.transcript_list" => {
            let request: TranscriptListToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(list_transcript_sessions(request.limit.unwrap_or(20))?)?
        }
        "packet28.transcript_show" => {
            let request: TranscriptShowToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(show_transcript_session(
                &request.session,
                request.limit.unwrap_or(100),
            )?)?
        }
        "packet28.transcript_search" => {
            let request: TranscriptSearchToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(search_transcripts_filtered(
                &request.query,
                request.project.as_deref(),
                request.limit.unwrap_or(10),
            )?)?
        }
        "packet28.transcript_stats" => serde_json::to_value(transcript_stats()?)?,
        "packet28.transcript_export" => {
            let request: TranscriptExportToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(export_transcripts(
                request.session.as_deref(),
                request.limit.unwrap_or(10_000),
            )?)?
        }
        "packet28.transcript_import" => {
            let request: TranscriptImportToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(import_transcripts_from_str(&request.content)?)?
        }
        "packet28.graph_create" => {
            let request: GraphCreateToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(create_graph_memoir(
                request.name.as_deref(),
                request.description.as_deref(),
            )?)?
        }
        "packet28.graph_list" => serde_json::to_value(list_graph_memoirs()?)?,
        "packet28.graph_show" => {
            let request: GraphShowToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(show_graph_memoir(
                request.name.as_deref(),
                request.limit.unwrap_or(50),
            )?)?
        }
        "packet28.graph_add_concept" => {
            let request: GraphConceptToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(add_concept_with_metadata(
                &request.name,
                request.description.as_deref(),
                request.memoir.as_deref(),
                &request.labels.unwrap_or_default(),
                request.confidence,
                &request.source_ids.unwrap_or_default(),
            )?)?
        }
        "packet28.graph_refine" => {
            let request: GraphRefineToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(refine_concept(&request.name, &request.description)?)?
        }
        "packet28.graph_link" => {
            let request: GraphLinkToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(link_concepts(
                &request.source,
                &request.target,
                request.relation.as_deref().unwrap_or("related_to"),
            )?)?
        }
        "packet28.graph_search" => {
            let request: GraphSearchToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(search_concepts_filtered(
                &request.query,
                request.memoir.as_deref(),
                request.label.as_deref(),
                request.limit.unwrap_or(10),
            )?)?
        }
        "packet28.graph_export" => {
            let request: GraphExportToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(export_graph(
                request.format.as_deref().unwrap_or("json"),
                request.limit.unwrap_or(100),
            )?)?
        }
        "packet28.graph_stats" => serde_json::to_value(graph_stats()?)?,
        "packet28.graph_delete" => {
            let request: GraphDeleteToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(delete_concept(&request.name)?)?
        }
        "packet28.graph_inspect" => {
            let request: GraphInspectToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(inspect_graph(request.limit.unwrap_or(50))?)?
        }
        "packet28.graph_inspect_concept" => {
            let request: GraphInspectConceptToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(inspect_graph_concept(
                &request.name,
                request.memoir.as_deref(),
                request.depth.unwrap_or(1),
            )?)?
        }
        "packet28.graph_distill" => {
            let request: GraphDistillToolArgs = serde_json::from_value(arguments.clone())?;
            serde_json::to_value(distill_memories_to_graph(
                &request.from_topic,
                request.into.as_deref(),
                request.limit.unwrap_or(100),
            )?)?
        }
        _ => return Ok(None),
    };
    Ok(Some(payload))
}
