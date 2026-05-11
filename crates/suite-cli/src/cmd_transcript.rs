use std::fs;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

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
    Export(TranscriptExportArgs),
    Import(TranscriptImportArgs),
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

#[derive(Args)]
pub struct TranscriptExportArgs {
    #[arg(long)]
    pub session: Option<String>,
    #[arg(long)]
    pub output: Option<String>,
    #[arg(long, default_value_t = 10_000)]
    pub limit: usize,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct TranscriptImportArgs {
    pub path: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TranscriptExportFile {
    format: String,
    version: u32,
    messages: Vec<TranscriptImportMessage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TranscriptImportMessage {
    session_key: String,
    agent: Option<String>,
    role: String,
    content: String,
    source: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TranscriptImportReport {
    imported_count: usize,
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
        TranscriptCommands::Export(args) => {
            let export = export_transcripts(args.session.as_deref(), args.limit)?;
            let rendered = if args.pretty {
                serde_json::to_string_pretty(&export)?
            } else {
                serde_json::to_string(&export)?
            };
            if let Some(output) = args.output {
                fs::write(output, rendered)?;
            } else {
                println!("{rendered}");
            }
        }
        TranscriptCommands::Import(args) => {
            let report = import_transcripts(&args.path)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
            } else {
                println!("imported_transcript_messages={}", report.imported_count);
            }
        }
    }
    Ok(0)
}

pub(crate) fn export_transcripts(
    session: Option<&str>,
    limit: usize,
) -> Result<TranscriptExportFile> {
    let mut messages = Vec::new();
    if let Some(session) = session {
        messages.extend(show_transcript_session(session, limit)?);
    } else {
        for session in list_transcript_sessions(limit)? {
            messages.extend(show_transcript_session(&session.session_key, limit)?);
        }
    }
    Ok(TranscriptExportFile {
        format: "packet28.transcript.export".to_string(),
        version: 1,
        messages: messages
            .into_iter()
            .map(|message| TranscriptImportMessage {
                session_key: message.session_key,
                agent: message.agent,
                role: message.role,
                content: message.content,
                source: message.source,
            })
            .collect(),
    })
}

pub(crate) fn import_transcripts(path: &str) -> Result<TranscriptImportReport> {
    let content = fs::read_to_string(path)?;
    import_transcripts_from_str(&content)
}

pub(crate) fn import_transcripts_from_str(content: &str) -> Result<TranscriptImportReport> {
    let export: TranscriptExportFile = serde_json::from_str(&content)?;
    let mut imported_count = 0usize;
    for message in export.messages {
        append_transcript_message(TranscriptAppendInput {
            session: Some(&message.session_key),
            agent: message.agent.as_deref(),
            role: Some(&message.role),
            content: &message.content,
            source: message.source.as_deref(),
        })?;
        imported_count += 1;
    }
    Ok(TranscriptImportReport { imported_count })
}
