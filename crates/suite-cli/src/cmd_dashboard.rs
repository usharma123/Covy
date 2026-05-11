use std::collections::BTreeMap;

use anyhow::Result;
use clap::Args;
use packet28_daemon_core::load_task_registry;
use serde::Serialize;

use crate::memory_store::{inspect_graph, list_memories, local_store_stats, search_feedback};
use crate::savings_analytics::load_run_savings;

#[derive(Args)]
pub struct DashboardArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Serialize)]
struct DashboardReport {
    token_savings: TokenSavings,
    commands_reduced: usize,
    sessions: usize,
    top_noisy_commands: Vec<String>,
    missed_savings: Vec<String>,
    memory_count: i64,
    recent_memories: Vec<String>,
    graph_concepts: usize,
    graph_relations: usize,
    feedback_corrections: i64,
    mcp_call_history: i64,
    hook_event_history: i64,
    pending_extractions: i64,
    integration_health: BTreeMap<String, String>,
    windsurf_doctor_status: String,
}

#[derive(Debug, Serialize, Default)]
struct TokenSavings {
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
    saved_est_tokens: u64,
    savings_percent: f64,
}

pub fn run(args: DashboardArgs) -> Result<i32> {
    let root = crate::cmd_daemon::resolve_root_arg(&args.root);
    let savings = load_run_savings(&root, 100)?;
    let mut token_savings = TokenSavings::default();
    let mut top_noisy_commands = Vec::new();
    let mut missed_savings = Vec::new();
    for record in &savings {
        token_savings.raw_est_tokens += record.raw_est_tokens;
        token_savings.reduced_est_tokens += record.reduced_est_tokens;
        if record.raw_est_tokens > 0 {
            top_noisy_commands.push(record.command.clone());
        }
        if record.fallback_reason.is_some() || record.savings_percent < 10.0 {
            missed_savings.push(record.command.clone());
        }
    }
    token_savings.saved_est_tokens = token_savings
        .raw_est_tokens
        .saturating_sub(token_savings.reduced_est_tokens);
    token_savings.savings_percent = if token_savings.raw_est_tokens == 0 {
        0.0
    } else {
        (token_savings.saved_est_tokens as f64 / token_savings.raw_est_tokens as f64) * 100.0
    };
    top_noisy_commands.truncate(5);
    missed_savings.truncate(5);

    let sessions = load_task_registry(&root)
        .map(|registry| registry.tasks.len())
        .unwrap_or_default();
    let store_stats = local_store_stats()?;
    let recent_memories = list_memories(5)?
        .into_iter()
        .map(|memory| memory.content)
        .collect::<Vec<_>>();
    let graph = inspect_graph(100)?;
    let feedback_corrections = search_feedback("", 100)?.len() as i64;
    let windsurf_rules = root.join(".windsurf").join("rules").join("packet28.md");
    let windsurf_status = if windsurf_rules.exists() {
        "rules_present"
    } else {
        "rules_missing"
    };
    let mut integration_health = BTreeMap::new();
    integration_health.insert("windsurf".to_string(), windsurf_status.to_string());
    integration_health.insert("local_dashboard".to_string(), "ok".to_string());

    let report = DashboardReport {
        token_savings,
        commands_reduced: savings
            .iter()
            .filter(|record| record.fallback_reason.is_none())
            .count(),
        sessions,
        top_noisy_commands,
        missed_savings,
        memory_count: store_stats.memory_count,
        recent_memories,
        graph_concepts: graph.concepts.len(),
        graph_relations: graph.relations.len(),
        feedback_corrections,
        mcp_call_history: store_stats.mcp_call_count,
        hook_event_history: store_stats.hook_event_count,
        pending_extractions: store_stats.pending_extraction_count,
        integration_health,
        windsurf_doctor_status: windsurf_status.to_string(),
    };

    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else {
        println!(
            "token_savings.saved_est_tokens={}",
            report.token_savings.saved_est_tokens
        );
        println!("commands_reduced={}", report.commands_reduced);
        println!("sessions={}", report.sessions);
        println!("memory_count={}", report.memory_count);
        println!("graph_concepts={}", report.graph_concepts);
        println!("graph_relations={}", report.graph_relations);
        println!("feedback_corrections={}", report.feedback_corrections);
        println!("mcp_call_history={}", report.mcp_call_history);
        println!("hook_event_history={}", report.hook_event_history);
        println!("pending_extractions={}", report.pending_extractions);
        println!("windsurf_doctor_status={}", report.windsurf_doctor_status);
    }
    Ok(0)
}
