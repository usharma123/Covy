use anyhow::Result;
use clap::{Args, Subcommand};

use crate::memory_store::{
    consolidate_memories, decay_memories, embed_memories, forget_memories_by_topic, forget_memory,
    list_memories_filtered, local_store_stats, memory_health, memory_topics, prune_memories,
    recall_memories_filtered, store_memory_with_metadata, update_memory, MemoryListQuery,
    MemoryRecallQuery, MemoryStoreInput, MemoryUpdateInput,
};

#[derive(Args)]
pub struct MemoryArgs {
    #[command(subcommand)]
    pub command: MemoryCommands,
}

#[derive(Subcommand)]
pub enum MemoryCommands {
    Store(MemoryStoreArgs),
    Recall(MemoryRecallArgs),
    List(MemoryListArgs),
    Update(MemoryUpdateArgs),
    Forget(MemoryForgetArgs),
    Topics(MemoryTopicsArgs),
    Stats(MemoryStatsArgs),
    Health(MemoryHealthArgs),
    Decay(MemoryDecayArgs),
    Prune(MemoryPruneArgs),
    Consolidate(MemoryConsolidateArgs),
    Embed(MemoryEmbedArgs),
}

#[derive(Args)]
pub struct MemoryStoreArgs {
    pub content: String,
    #[arg(long)]
    pub tags: Option<String>,
    #[arg(long)]
    pub topic: Option<String>,
    #[arg(long)]
    pub importance: Option<String>,
    #[arg(long)]
    pub keywords: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long)]
    pub raw: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryRecallArgs {
    pub query: String,
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
    #[arg(long)]
    pub topic: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub tag: Option<String>,
    #[arg(long)]
    pub keyword: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryListArgs {
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    #[arg(long)]
    pub topic: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub all: bool,
    #[arg(long, default_value = "recent")]
    pub sort: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryUpdateArgs {
    pub id: i64,
    #[arg(long)]
    pub content: Option<String>,
    #[arg(long)]
    pub tags: Option<String>,
    #[arg(long)]
    pub topic: Option<String>,
    #[arg(long)]
    pub importance: Option<String>,
    #[arg(long)]
    pub keywords: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long)]
    pub raw: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryForgetArgs {
    pub id: Option<i64>,
    #[arg(long)]
    pub topic: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryTopicsArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryStatsArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryHealthArgs {
    #[arg(long)]
    pub topic: Option<String>,
    #[arg(long, default_value_t = 30)]
    pub stale_after_days: i64,
    #[arg(long, default_value_t = 10)]
    pub consolidation_threshold: i64,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryDecayArgs {
    #[arg(long, default_value_t = 0.95)]
    pub factor: f64,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryPruneArgs {
    #[arg(long, default_value_t = 0.1)]
    pub threshold: f64,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryConsolidateArgs {
    #[arg(long)]
    pub topic: Option<String>,
    #[arg(long)]
    pub keep_originals: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryEmbedArgs {
    pub id: Option<i64>,
    #[arg(long)]
    pub all: bool,
    #[arg(long, default_value_t = 384)]
    pub dimensions: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

pub fn run(args: MemoryArgs) -> Result<i32> {
    match args.command {
        MemoryCommands::Store(args) => run_store(args),
        MemoryCommands::Recall(args) => run_recall(args),
        MemoryCommands::List(args) => run_list(args),
        MemoryCommands::Update(args) => run_update(args),
        MemoryCommands::Forget(args) => run_forget(args),
        MemoryCommands::Topics(args) => run_topics(args),
        MemoryCommands::Stats(args) => run_stats(args),
        MemoryCommands::Health(args) => run_health(args),
        MemoryCommands::Decay(args) => run_decay(args),
        MemoryCommands::Prune(args) => run_prune(args),
        MemoryCommands::Consolidate(args) => run_consolidate(args),
        MemoryCommands::Embed(args) => run_embed(args),
    }
}

fn run_store(args: MemoryStoreArgs) -> Result<i32> {
    let record = store_memory_with_metadata(MemoryStoreInput {
        content: &args.content,
        tags: args.tags.as_deref(),
        topic: args.topic.as_deref(),
        importance: args.importance.as_deref(),
        keywords: args.keywords.as_deref(),
        project: args.project.as_deref(),
        source: args.source.as_deref(),
        raw_excerpt: args.raw.as_deref(),
    })?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(record)?, args.pretty)?;
    } else {
        println!("stored memory {}", record.id);
    }
    Ok(0)
}

fn run_recall(args: MemoryRecallArgs) -> Result<i32> {
    let records = recall_memories_filtered(MemoryRecallQuery {
        query: &args.query,
        limit: args.limit,
        topic: args.topic.as_deref(),
        project: args.project.as_deref(),
        tag: args.tag.as_deref(),
        keyword: args.keyword.as_deref(),
    })?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(records)?, args.pretty)?;
    } else {
        for record in records {
            println!("{} {}", record.id, record.content);
        }
    }
    Ok(0)
}

fn run_list(args: MemoryListArgs) -> Result<i32> {
    let records = list_memories_filtered(MemoryListQuery {
        limit: args.limit,
        topic: args.topic.as_deref(),
        project: args.project.as_deref(),
        all: args.all,
        sort: &args.sort,
    })?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(records)?, args.pretty)?;
    } else {
        for record in records {
            println!("{} {}", record.id, record.content);
        }
    }
    Ok(0)
}

fn run_update(args: MemoryUpdateArgs) -> Result<i32> {
    let record = update_memory(MemoryUpdateInput {
        id: args.id,
        content: args.content.as_deref(),
        tags: args.tags.as_deref(),
        topic: args.topic.as_deref(),
        importance: args.importance.as_deref(),
        keywords: args.keywords.as_deref(),
        project: args.project.as_deref(),
        source: args.source.as_deref(),
        raw_excerpt: args.raw.as_deref(),
    })?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(record)?, args.pretty)?;
    } else {
        println!("updated memory {}", record.id);
    }
    Ok(0)
}

fn run_forget(args: MemoryForgetArgs) -> Result<i32> {
    let deleted = match (args.id, args.topic.as_deref()) {
        (Some(id), None) => forget_memory(id)?,
        (None, Some(topic)) => forget_memories_by_topic(topic)?,
        _ => anyhow::bail!("pass exactly one of memory id or --topic"),
    };
    if args.json {
        crate::cmd_common::emit_json(&serde_json::json!({ "deleted": deleted }), args.pretty)?;
    } else {
        println!("deleted={deleted}");
    }
    Ok(0)
}

fn run_topics(args: MemoryTopicsArgs) -> Result<i32> {
    let topics = memory_topics()?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(topics)?, args.pretty)?;
    } else {
        for topic in topics {
            println!("{} {}", topic.topic, topic.memory_count);
        }
    }
    Ok(0)
}

fn run_stats(args: MemoryStatsArgs) -> Result<i32> {
    let stats = local_store_stats()?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(stats)?, args.pretty)?;
    } else {
        println!("memory_count={}", stats.memory_count);
    }
    Ok(0)
}

fn run_health(args: MemoryHealthArgs) -> Result<i32> {
    let report = memory_health(
        args.topic.as_deref(),
        args.stale_after_days,
        args.consolidation_threshold,
    )?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else {
        println!("total_topics={}", report.total_topics);
        println!("total_memories={}", report.total_memories);
        println!("stale_memories={}", report.stale_memories);
        println!(
            "topics_needing_consolidation={}",
            report.topics_needing_consolidation
        );
        for topic in report.topics {
            println!(
                "{} count={} stale={} oldest_age_days={} newest_age_days={} consolidation_needed={}",
                topic.topic,
                topic.memory_count,
                topic.stale_count,
                topic.oldest_age_days,
                topic.newest_age_days,
                topic.consolidation_needed
            );
        }
    }
    Ok(0)
}

fn run_decay(args: MemoryDecayArgs) -> Result<i32> {
    let report = decay_memories(args.factor)?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else {
        println!("factor={}", report.factor);
        println!("decayed_count={}", report.decayed_count);
        println!("skipped_critical_count={}", report.skipped_critical_count);
    }
    Ok(0)
}

fn run_prune(args: MemoryPruneArgs) -> Result<i32> {
    let report = prune_memories(args.threshold, args.dry_run)?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else {
        println!("threshold={}", report.threshold);
        println!("dry_run={}", report.dry_run);
        println!("candidate_count={}", report.candidate_count);
        println!("deleted_count={}", report.deleted_count);
    }
    Ok(0)
}

fn run_consolidate(args: MemoryConsolidateArgs) -> Result<i32> {
    let report = consolidate_memories(args.topic.as_deref(), args.keep_originals)?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else {
        println!("topic={}", report.topic);
        println!("source_count={}", report.source_count);
        println!("status={}", report.status);
        if let Some(memory) = report.consolidated_memory {
            println!("consolidated_memory={}", memory.id);
        }
    }
    Ok(0)
}

fn run_embed(args: MemoryEmbedArgs) -> Result<i32> {
    let report = embed_memories(args.id, args.all, args.dimensions)?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else {
        println!("embedded_count={}", report.embedded_count);
        println!("model={}", report.model);
        println!("dimensions={}", report.dimensions);
    }
    Ok(0)
}
