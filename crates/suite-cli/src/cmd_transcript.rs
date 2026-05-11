use anyhow::Result;
use clap::{Args, Subcommand};

use crate::memory_store::{
    append_transcript_message, list_transcript_sessions, search_transcripts,
    show_transcript_session, transcript_stats, TranscriptAppendInput,
};

#[derive(Args)]
pub struct TranscriptArgs {
    #[command(subcommand)]
    pub command: TranscriptCommands,
}

#[derive(Subcommand)]
pub enum TranscriptCommands {
    Append(TranscriptAppendArgs),
    List(TranscriptListArgs),
    Show(TranscriptShowArgs),
    Search(TranscriptSearchArgs),
    Stats(TranscriptStatsArgs),
}

#[derive(Args)]
pub struct TranscriptAppendArgs {
    pub content: String,
    #[arg(long)]
    pub session: Option<String>,
    #[arg(long)]
    pub agent: Option<String>,
    #[arg(long, default_value = "assistant")]
    pub role: String,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct TranscriptListArgs {
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct TranscriptShowArgs {
    pub session: String,
    #[arg(long, default_value_t = 100)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct TranscriptSearchArgs {
    pub query: String,
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct TranscriptStatsArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

pub fn run(args: TranscriptArgs) -> Result<i32> {
    match args.command {
        TranscriptCommands::Append(args) => {
            let record = append_transcript_message(TranscriptAppendInput {
                session: args.session.as_deref(),
                agent: args.agent.as_deref(),
                role: Some(&args.role),
                content: &args.content,
                source: args.source.as_deref(),
            })?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(record)?, args.pretty)?;
            } else {
                println!("{} {} {}", record.session_key, record.role, record.id);
            }
        }
        TranscriptCommands::List(args) => {
            let sessions = list_transcript_sessions(args.limit)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(sessions)?, args.pretty)?;
            } else {
                for session in sessions {
                    println!("{} messages={}", session.session_key, session.message_count);
                }
            }
        }
        TranscriptCommands::Show(args) => {
            let messages = show_transcript_session(&args.session, args.limit)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(messages)?, args.pretty)?;
            } else {
                for message in messages {
                    println!("{}: {}", message.role, message.content);
                }
            }
        }
        TranscriptCommands::Search(args) => {
            let messages = search_transcripts(&args.query, args.limit)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(messages)?, args.pretty)?;
            } else {
                for message in messages {
                    println!(
                        "{} {}: {}",
                        message.session_key, message.role, message.content
                    );
                }
            }
        }
        TranscriptCommands::Stats(args) => {
            let stats = transcript_stats()?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(stats)?, args.pretty)?;
            } else {
                println!("transcript_sessions={}", stats.session_count);
                println!("transcript_messages={}", stats.message_count);
            }
        }
    }
    Ok(0)
}
