use anyhow::Result;
use clap::{Args, Subcommand};

use crate::memory_store::{local_store_stats, record_feedback, search_feedback};

#[derive(Args)]
pub struct FeedbackArgs {
    #[command(subcommand)]
    pub command: FeedbackCommands,
}

#[derive(Subcommand)]
pub enum FeedbackCommands {
    Record(FeedbackRecordArgs),
    Search(FeedbackSearchArgs),
    Stats(FeedbackStatsArgs),
}

#[derive(Args)]
pub struct FeedbackRecordArgs {
    pub subject: String,
    pub correction: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct FeedbackSearchArgs {
    pub query: String,
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct FeedbackStatsArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

pub fn run(args: FeedbackArgs) -> Result<i32> {
    match args.command {
        FeedbackCommands::Record(args) => {
            let record = record_feedback(&args.subject, &args.correction)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(record)?, args.pretty)?;
            } else {
                println!("recorded feedback {}", record.id);
            }
        }
        FeedbackCommands::Search(args) => {
            let records = search_feedback(&args.query, args.limit)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(records)?, args.pretty)?;
            } else {
                for record in records {
                    println!("{} {} -> {}", record.id, record.subject, record.correction);
                }
            }
        }
        FeedbackCommands::Stats(args) => {
            let stats = local_store_stats()?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(stats)?, args.pretty)?;
            } else {
                println!("feedback_count={}", stats.feedback_count);
            }
        }
    }
    Ok(0)
}
