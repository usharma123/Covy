use anyhow::Result;
use clap::{Args, Subcommand};

use crate::memory_store::{
    apply_feedback, delete_feedback, feedback_stats, list_feedback, record_feedback_with_metadata,
    search_feedback, FeedbackInput,
};

#[derive(Args)]
pub struct FeedbackArgs {
    #[command(subcommand)]
    pub command: FeedbackCommands,
}

#[derive(Subcommand)]
pub enum FeedbackCommands {
    Record(FeedbackRecordArgs),
    Search(FeedbackSearchArgs),
    List(FeedbackListArgs),
    Apply(FeedbackApplyArgs),
    Delete(FeedbackDeleteArgs),
    Stats(FeedbackStatsArgs),
}

#[derive(Args)]
pub struct FeedbackRecordArgs {
    pub subject: String,
    pub correction: String,
    #[arg(long)]
    pub topic: Option<String>,
    #[arg(long)]
    pub context: Option<String>,
    #[arg(long)]
    pub predicted: Option<String>,
    #[arg(long)]
    pub reason: Option<String>,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
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
pub struct FeedbackListArgs {
    #[arg(long)]
    pub topic: Option<String>,
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct FeedbackApplyArgs {
    pub id: i64,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct FeedbackDeleteArgs {
    pub id: i64,
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
            let record = record_feedback_with_metadata(FeedbackInput {
                subject: &args.subject,
                correction: &args.correction,
                topic: args.topic.as_deref(),
                context: args.context.as_deref(),
                predicted: args.predicted.as_deref(),
                reason: args.reason.as_deref(),
                source: args.source.as_deref(),
                project: args.project.as_deref(),
            })?;
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
        FeedbackCommands::List(args) => {
            let records = list_feedback(args.topic.as_deref(), args.limit)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(records)?, args.pretty)?;
            } else {
                for record in records {
                    println!(
                        "{} [{}] {} -> {}",
                        record.id, record.topic, record.subject, record.correction
                    );
                }
            }
        }
        FeedbackCommands::Apply(args) => {
            let record = apply_feedback(args.id)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(record)?, args.pretty)?;
            } else {
                println!(
                    "applied feedback {} count={}",
                    record.id, record.applied_count
                );
            }
        }
        FeedbackCommands::Delete(args) => {
            let deleted = delete_feedback(args.id)?;
            if args.json {
                crate::cmd_common::emit_json(
                    &serde_json::json!({ "deleted": deleted }),
                    args.pretty,
                )?;
            } else {
                println!("deleted={deleted}");
            }
        }
        FeedbackCommands::Stats(args) => {
            let stats = feedback_stats()?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(stats)?, args.pretty)?;
            } else {
                println!("feedback_count={}", stats.feedback_count);
            }
        }
    }
    Ok(0)
}
