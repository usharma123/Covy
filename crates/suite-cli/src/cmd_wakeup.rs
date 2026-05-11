use anyhow::Result;
use clap::Args;
use serde::Serialize;

use crate::memory_store::{
    inspect_graph, list_memories_filtered, local_store_stats, recall_memories_filtered,
    search_feedback, search_transcripts, FeedbackRecord, GraphInspect, LocalStoreStats,
    MemoryListQuery, MemoryRecallQuery, MemoryRecord, TranscriptMessage,
};

#[derive(Args)]
pub struct WakeupArgs {
    /// Optional focus query for recalled memories and feedback
    #[arg(long)]
    pub query: Option<String>,

    /// Optional project filter for recalled memories
    #[arg(long)]
    pub project: Option<String>,

    /// Maximum memories, feedback records, concepts, and relations to include
    #[arg(long, default_value_t = 5)]
    pub limit: usize,

    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub pretty: bool,
}

#[derive(Serialize)]
pub(crate) struct WakeupReport {
    kind: &'static str,
    query: Option<String>,
    project: Option<String>,
    stats: LocalStoreStats,
    memories: Vec<MemoryRecord>,
    feedback: Vec<FeedbackRecord>,
    transcripts: Vec<TranscriptMessage>,
    graph: GraphInspect,
}

pub(crate) fn build_wakeup_report(
    query: Option<&str>,
    project: Option<&str>,
    limit: usize,
) -> Result<WakeupReport> {
    let limit = limit.max(1);
    let query = query.map(str::trim).filter(|q| !q.is_empty());
    let project = project.map(str::trim).filter(|q| !q.is_empty());
    let memories = match query {
        Some(query) => recall_memories_filtered(MemoryRecallQuery {
            query,
            limit,
            topic: None,
            project,
            tag: None,
            keyword: None,
        })?,
        None => list_memories_filtered(MemoryListQuery {
            limit,
            topic: None,
            project,
            all: false,
            sort: "recent",
        })?,
    };
    let feedback = search_feedback(query.unwrap_or_default(), limit)?;
    let transcripts = search_transcripts(query.unwrap_or_default(), limit)?;
    let graph = inspect_graph(limit)?;
    let stats = local_store_stats()?;
    Ok(WakeupReport {
        kind: "packet28.wakeup.v1",
        query: query.map(ToOwned::to_owned),
        project: project.map(ToOwned::to_owned),
        stats,
        memories,
        feedback,
        transcripts,
        graph,
    })
}

pub fn run(args: WakeupArgs) -> Result<i32> {
    let report = build_wakeup_report(args.query.as_deref(), args.project.as_deref(), args.limit)?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else {
        println!("memory_count={}", report.stats.memory_count);
        println!("feedback_count={}", report.stats.feedback_count);
        println!(
            "transcript_message_count={}",
            report.stats.transcript_message_count
        );
        println!("concept_count={}", report.stats.concept_count);
        for memory in &report.memories {
            println!("memory {} {}", memory.id, memory.content);
        }
        for feedback in &report.feedback {
            println!("feedback {} {}", feedback.id, feedback.correction);
        }
        for transcript in &report.transcripts {
            println!(
                "transcript {} {}: {}",
                transcript.session_key, transcript.role, transcript.content
            );
        }
    }
    Ok(0)
}
