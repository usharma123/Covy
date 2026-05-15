use anyhow::Result;
use clap::{Args, Subcommand};
use packet28_daemon_core::{BrokerWriteOp, BrokerWriteStateRequest};
use serde::Serialize;

const HYPOTHESIS_PREFIX: &str = "hypothesis:";

#[derive(Args)]
pub struct HypothesisArgs {
    #[command(subcommand)]
    pub command: HypothesisCommands,
}

#[derive(Subcommand)]
pub enum HypothesisCommands {
    Add(HypothesisAddArgs),
    Confirm(HypothesisResolveArgs),
    Reject(HypothesisResolveArgs),
    List(HypothesisListArgs),
}

#[derive(Args)]
pub struct HypothesisAddArgs {
    pub text: String,
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: String,
    #[arg(long)]
    pub id: Option<String>,
    #[arg(long = "path")]
    pub paths: Vec<String>,
    #[arg(long = "symbol")]
    pub symbols: Vec<String>,
    #[arg(long)]
    pub artifact_id: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct HypothesisResolveArgs {
    pub id: String,
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: String,
    #[arg(long)]
    pub note: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct HypothesisListArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub task_id: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HypothesisRecord {
    pub(crate) id: String,
    pub(crate) decision_id: String,
    pub(crate) text: String,
    pub(crate) related_paths: Vec<String>,
    pub(crate) related_symbols: Vec<String>,
    pub(crate) related_artifact_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct HypothesisMutation {
    pub(crate) id: String,
    pub(crate) decision_id: String,
    pub(crate) status: String,
}

pub fn run(args: HypothesisArgs) -> Result<i32> {
    match args.command {
        HypothesisCommands::Add(args) => add_hypothesis(args),
        HypothesisCommands::Confirm(args) => resolve_hypothesis(args, "confirmed"),
        HypothesisCommands::Reject(args) => resolve_hypothesis(args, "rejected"),
        HypothesisCommands::List(args) => list_hypotheses(args),
    }
}

fn add_hypothesis(args: HypothesisAddArgs) -> Result<i32> {
    let root = crate::cmd_daemon::resolve_root_arg(&args.root);
    let mutation = add_hypothesis_record(
        &root,
        &args.task_id,
        args.id,
        &args.text,
        args.paths,
        args.symbols,
        args.artifact_id,
    )?;
    emit_mutation(&mutation, args.json, args.pretty)
}

pub(crate) fn add_hypothesis_record(
    root: &std::path::Path,
    task_id: &str,
    id: Option<String>,
    text: &str,
    paths: Vec<String>,
    symbols: Vec<String>,
    artifact_id: Option<String>,
) -> Result<HypothesisMutation> {
    let id = id.unwrap_or_else(|| hypothesis_id(text));
    let decision_id = decision_id(&id);
    let text = format!("hypothesis active: {}", text.trim());
    crate::broker_client::write_state(
        root,
        BrokerWriteStateRequest {
            task_id: task_id.to_string(),
            op: Some(BrokerWriteOp::DecisionAdd),
            decision_id: Some(decision_id.clone()),
            text: Some(text),
            paths,
            symbols,
            artifact_id,
            ..BrokerWriteStateRequest::default()
        },
    )?;
    let mutation = HypothesisMutation {
        id,
        decision_id,
        status: "active".to_string(),
    };
    Ok(mutation)
}

fn resolve_hypothesis(args: HypothesisResolveArgs, status: &str) -> Result<i32> {
    let root = crate::cmd_daemon::resolve_root_arg(&args.root);
    let mutation = resolve_hypothesis_record(&root, &args.task_id, &args.id, status, args.note)?;
    emit_mutation(&mutation, args.json, args.pretty)
}

pub(crate) fn resolve_hypothesis_record(
    root: &std::path::Path,
    task_id: &str,
    id: &str,
    status: &str,
    note: Option<String>,
) -> Result<HypothesisMutation> {
    let decision_id = decision_id(id);
    crate::broker_client::write_state(
        root,
        BrokerWriteStateRequest {
            task_id: task_id.to_string(),
            op: Some(BrokerWriteOp::DecisionSupersede),
            decision_id: Some(decision_id.clone()),
            note: note.or_else(|| Some(status.to_string())),
            ..BrokerWriteStateRequest::default()
        },
    )?;
    let mutation = HypothesisMutation {
        id: id.to_string(),
        decision_id,
        status: status.to_string(),
    };
    Ok(mutation)
}

fn list_hypotheses(args: HypothesisListArgs) -> Result<i32> {
    let root = crate::cmd_daemon::resolve_root_arg(&args.root);
    let records = active_hypotheses(&root, &args.task_id)?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(records)?, args.pretty)?;
    } else if records.is_empty() {
        println!("active_hypotheses=0");
    } else {
        for record in records {
            println!("{} {}", record.id, record.text);
        }
    }
    Ok(0)
}

fn emit_mutation(mutation: &HypothesisMutation, json: bool, pretty: bool) -> Result<i32> {
    if json {
        crate::cmd_common::emit_json(&serde_json::to_value(mutation)?, pretty)?;
    } else {
        println!(
            "hypothesis {} {} ({})",
            mutation.id, mutation.status, mutation.decision_id
        );
    }
    Ok(0)
}

pub(crate) fn active_hypotheses(
    root: &std::path::Path,
    task_id: &str,
) -> Result<Vec<HypothesisRecord>> {
    let kernel = crate::cmd_context::build_persistent_kernel(root.to_path_buf());
    let response = kernel.execute(context_kernel_core::KernelRequest {
        target: "agenty.state.snapshot".to_string(),
        reducer_input: serde_json::json!({
            "task_id": task_id,
        }),
        policy_context: serde_json::json!({
            "disable_cache": true,
        }),
        ..context_kernel_core::KernelRequest::default()
    })?;
    let packet = response
        .output_packets
        .first()
        .ok_or_else(|| anyhow::anyhow!("kernel returned no agent snapshot packet"))?;
    let envelope: suite_packet_core::EnvelopeV1<suite_packet_core::AgentSnapshotPayload> =
        serde_json::from_value(packet.body.clone())
            .map_err(|source| anyhow::anyhow!("invalid agent snapshot packet: {source}"))?;
    Ok(envelope
        .payload
        .active_decisions
        .into_iter()
        .filter(|decision| decision.id.starts_with(HYPOTHESIS_PREFIX))
        .map(|decision| HypothesisRecord {
            id: decision
                .id
                .strip_prefix(HYPOTHESIS_PREFIX)
                .unwrap_or(&decision.id)
                .to_string(),
            decision_id: decision.id,
            text: decision
                .text
                .strip_prefix("hypothesis active: ")
                .unwrap_or(&decision.text)
                .to_string(),
            related_paths: decision.related_paths,
            related_symbols: decision.related_symbols,
            related_artifact_ids: decision.related_artifact_ids,
        })
        .collect())
}

fn decision_id(id: &str) -> String {
    let trimmed = id.trim().trim_start_matches(HYPOTHESIS_PREFIX);
    format!("{HYPOTHESIS_PREFIX}{trimmed}")
}

fn hypothesis_id(text: &str) -> String {
    let hash = blake3::hash(text.trim().as_bytes()).to_hex().to_string();
    format!("h-{}", &hash[..12])
}
