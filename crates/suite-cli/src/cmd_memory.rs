use anyhow::Result;
use clap::{Args, Subcommand};

use crate::memory_store::{list_memories, recall_memories, store_memory};

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
    Consolidate(MemoryConsolidateArgs),
}

#[derive(Args)]
pub struct MemoryStoreArgs {
    pub content: String,
    #[arg(long)]
    pub tags: Option<String>,
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
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryListArgs {
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct MemoryConsolidateArgs {
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
        MemoryCommands::Consolidate(args) => run_consolidate(args),
    }
}

fn run_store(args: MemoryStoreArgs) -> Result<i32> {
    let record = store_memory(&args.content, args.tags.as_deref())?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(record)?, args.pretty)?;
    } else {
        println!("stored memory {}", record.id);
    }
    Ok(0)
}

fn run_recall(args: MemoryRecallArgs) -> Result<i32> {
    let records = recall_memories(&args.query, args.limit)?;
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
    let records = list_memories(args.limit)?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(records)?, args.pretty)?;
    } else {
        for record in records {
            println!("{} {}", record.id, record.content);
        }
    }
    Ok(0)
}

fn run_consolidate(args: MemoryConsolidateArgs) -> Result<i32> {
    let records = list_memories(100)?;
    if args.json {
        crate::cmd_common::emit_json(
            &serde_json::json!({"memory_count": records.len(), "status": "noop"}),
            args.pretty,
        )?;
    } else {
        println!("memory_count={}", records.len());
        println!("status=noop");
    }
    Ok(0)
}
