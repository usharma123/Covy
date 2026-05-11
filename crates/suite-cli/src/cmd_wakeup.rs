use anyhow::Result;
use clap::Args;
use serde::Serialize;

use crate::memory_store::{
    inspect_graph, list_memories, local_store_stats, recall_memories, search_feedback,
    FeedbackRecord, GraphInspect, LocalStoreStats, MemoryRecord,
};

#[derive(Args)]
pub struct WakeupArgs {
    /// Optional focus query for recalled memories and feedback
    #[arg(long)]
    pub query: Option<String>,

    /// Maximum memories, feedback records, concepts, and relations to include
    #[arg(long, default_value_t = 5)]
    pub limit: usize,

    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub pretty: bool,
}

#[derive(Serialize)]
struct WakeupReport {
    kind: &'static str,
    query: Option<String>,
    stats: LocalStoreStats,
    memories: Vec<MemoryRecord>,
    feedback: Vec<FeedbackRecord>,
    graph: GraphInspect,
}

pub fn run(args: WakeupArgs) -> Result<i32> {
    let limit = args.limit.max(1);
    let query = args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    let memories = match query {
        Some(query) => recall_memories(query, limit)?,
        None => list_memories(limit)?,
    };
    let feedback = search_feedback(query.unwrap_or_default(), limit)?;
    let graph = inspect_graph(limit)?;
    let stats = local_store_stats()?;
    let report = WakeupReport {
        kind: "packet28.wakeup.v1",
        query: query.map(ToOwned::to_owned),
        stats,
        memories,
        feedback,
        graph,
    };

    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else {
        println!("memory_count={}", report.stats.memory_count);
        println!("feedback_count={}", report.stats.feedback_count);
        println!("concept_count={}", report.stats.concept_count);
        for memory in &report.memories {
            println!("memory {} {}", memory.id, memory.content);
        }
        for feedback in &report.feedback {
            println!("feedback {} {}", feedback.id, feedback.correction);
        }
    }
    Ok(0)
}
