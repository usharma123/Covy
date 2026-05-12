use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};

use anyhow::Result;
use clap::Args;
use packet28_daemon_core::load_task_registry;
use serde::Serialize;

use crate::memory_store::{
    feedback_stats, graph_stats, list_memories, local_store_stats, memory_health, memory_topics,
    transcript_stats, FeedbackStats, GraphStats, MemoryHealthReport, MemoryTopicStats,
    TranscriptStats,
};
use crate::savings_analytics::load_run_savings;

#[derive(Args)]
pub struct DashboardArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    /// Output format: text, json, or html
    #[arg(long, default_value = "text")]
    pub format: String,
    /// Write rendered dashboard output to this path
    #[arg(long)]
    pub output: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
    /// Keep the terminal dashboard open and accept navigation commands on stdin
    #[arg(long)]
    pub interactive: bool,
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
    memory_topics: Vec<MemoryTopicStats>,
    memory_health: MemoryHealthReport,
    graph_concepts: usize,
    graph_relations: usize,
    graph_stats: GraphStats,
    feedback_corrections: i64,
    feedback_stats: FeedbackStats,
    transcript_stats: TranscriptStats,
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
    let topics = memory_topics()?;
    let health = memory_health(None, 30, 10)?;
    let graph = graph_stats()?;
    let feedback = feedback_stats()?;
    let transcripts = transcript_stats()?;
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
        memory_topics: topics,
        memory_health: health,
        graph_concepts: graph.concept_count as usize,
        graph_relations: graph.relation_count as usize,
        graph_stats: graph,
        feedback_corrections: feedback.feedback_count,
        feedback_stats: feedback,
        transcript_stats: transcripts,
        mcp_call_history: store_stats.mcp_call_count,
        hook_event_history: store_stats.hook_event_count,
        pending_extractions: store_stats.pending_extraction_count,
        integration_health,
        windsurf_doctor_status: windsurf_status.to_string(),
    };

    let format = args.format.trim().to_ascii_lowercase();
    if args.json || format == "json" {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else if format == "html" {
        let html = render_dashboard_html(&report);
        if let Some(path) = args.output.as_deref() {
            fs::write(path, html)?;
            println!("dashboard_html={path}");
        } else {
            print!("{html}");
        }
    } else if format == "tui" {
        if args.interactive {
            run_dashboard_tui(&report)?;
        } else {
            print!(
                "{}",
                render_dashboard_tui(&report, DashboardPanel::Overview)
            );
        }
    } else {
        println!(
            "token_savings.saved_est_tokens={}",
            report.token_savings.saved_est_tokens
        );
        println!("commands_reduced={}", report.commands_reduced);
        println!("sessions={}", report.sessions);
        println!("memory_count={}", report.memory_count);
        println!("memory_topics={}", report.memory_topics.len());
        println!(
            "topics_needing_consolidation={}",
            report.memory_health.topics_needing_consolidation
        );
        println!("graph_concepts={}", report.graph_concepts);
        println!("graph_relations={}", report.graph_relations);
        println!("feedback_corrections={}", report.feedback_corrections);
        println!(
            "transcript_messages={}",
            report.transcript_stats.message_count
        );
        println!("mcp_call_history={}", report.mcp_call_history);
        println!("hook_event_history={}", report.hook_event_history);
        println!("pending_extractions={}", report.pending_extractions);
        println!("windsurf_doctor_status={}", report.windsurf_doctor_status);
    }
    Ok(0)
}

#[derive(Clone, Copy)]
enum DashboardPanel {
    Overview,
    Memory,
    Graph,
    Feedback,
    Integrations,
}

impl DashboardPanel {
    fn from_command(command: &str) -> Option<Self> {
        match command.trim().to_ascii_lowercase().as_str() {
            "1" | "overview" | "o" => Some(Self::Overview),
            "2" | "memory" | "m" => Some(Self::Memory),
            "3" | "graph" | "g" => Some(Self::Graph),
            "4" | "feedback" | "f" => Some(Self::Feedback),
            "5" | "integrations" | "i" => Some(Self::Integrations),
            _ => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Memory => "Memory",
            Self::Graph => "Graph",
            Self::Feedback => "Feedback",
            Self::Integrations => "Integrations",
        }
    }
}

fn run_dashboard_tui(report: &DashboardReport) -> Result<()> {
    let mut panel = DashboardPanel::Overview;
    let mut input = String::new();
    loop {
        print!("{}", render_dashboard_tui(report, panel));
        io::stdout().flush()?;
        input.clear();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        let command = input.trim();
        if matches!(command, "q" | "quit" | "exit") {
            break;
        }
        if let Some(next) = DashboardPanel::from_command(command) {
            panel = next;
        }
    }
    Ok(())
}

fn render_dashboard_tui(report: &DashboardReport, panel: DashboardPanel) -> String {
    let mut out = String::new();
    out.push_str("Packet28 Dashboard\n");
    out.push_str("==================\n");
    out.push_str("1 Overview  2 Memory  3 Graph  4 Feedback  5 Integrations  q Quit\n");
    out.push_str(&format!("panel={}\n\n", panel.title()));
    match panel {
        DashboardPanel::Overview => {
            out.push_str(&format!(
                "saved_tokens={}\nsavings_percent={:.1}\ncommands_reduced={}\nsessions={}\n",
                report.token_savings.saved_est_tokens,
                report.token_savings.savings_percent,
                report.commands_reduced,
                report.sessions
            ));
            out.push_str("top_noisy_commands:\n");
            push_tui_list(&mut out, &report.top_noisy_commands);
            out.push_str("missed_savings:\n");
            push_tui_list(&mut out, &report.missed_savings);
        }
        DashboardPanel::Memory => {
            out.push_str(&format!(
                "memory_count={}\ntopics={}\ntopics_needing_consolidation={}\npending_extractions={}\n",
                report.memory_count,
                report.memory_topics.len(),
                report.memory_health.topics_needing_consolidation,
                report.pending_extractions
            ));
            out.push_str("recent_memories:\n");
            push_tui_list(&mut out, &report.recent_memories);
            out.push_str("memory_topics:\n");
            for topic in &report.memory_topics {
                out.push_str(&format!("- {} ({})\n", topic.topic, topic.memory_count));
            }
        }
        DashboardPanel::Graph => {
            out.push_str(&format!(
                "graph_concepts={}\ngraph_relations={}\nrelation_types={}\n",
                report.graph_concepts,
                report.graph_relations,
                report.graph_stats.relation_types.len()
            ));
        }
        DashboardPanel::Feedback => {
            out.push_str(&format!(
                "feedback_corrections={}\ntranscript_messages={}\nmcp_call_history={}\nhook_event_history={}\n",
                report.feedback_corrections,
                report.transcript_stats.message_count,
                report.mcp_call_history,
                report.hook_event_history
            ));
        }
        DashboardPanel::Integrations => {
            out.push_str(&format!(
                "windsurf_doctor_status={}\n",
                report.windsurf_doctor_status
            ));
            for (name, status) in &report.integration_health {
                out.push_str(&format!("{name}={status}\n"));
            }
        }
    }
    out.push('\n');
    out
}

fn push_tui_list(out: &mut String, values: &[String]) {
    if values.is_empty() {
        out.push_str("- none\n");
    } else {
        for value in values {
            out.push_str(&format!("- {}\n", value.replace('\n', " ")));
        }
    }
}

fn render_dashboard_html(report: &DashboardReport) -> String {
    let mut html = String::from(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Packet28 Dashboard</title>
<style>
body{font-family:system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;margin:0;background:#f7f7f4;color:#1f2328}
main{max-width:1120px;margin:0 auto;padding:28px}
h1{font-size:28px;margin:0 0 20px}
h2{font-size:17px;margin:24px 0 10px}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:12px}
.metric{border:1px solid #d8d7d0;background:#fff;padding:14px;border-radius:8px}
.label{font-size:12px;text-transform:uppercase;color:#667085}
.value{font-size:26px;font-weight:700;margin-top:6px}
table{width:100%;border-collapse:collapse;background:#fff;border:1px solid #d8d7d0}
th,td{text-align:left;padding:8px 10px;border-bottom:1px solid #ecebe6;font-size:14px}
th{background:#efeee8}
code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}
</style>
</head>
<body><main>
<h1>Packet28 Dashboard</h1>
"#,
    );
    html.push_str("<section class=\"grid\">");
    push_metric(
        &mut html,
        "Saved tokens",
        &report.token_savings.saved_est_tokens.to_string(),
    );
    push_metric(
        &mut html,
        "Savings",
        &format!("{:.1}%", report.token_savings.savings_percent),
    );
    push_metric(
        &mut html,
        "Commands reduced",
        &report.commands_reduced.to_string(),
    );
    push_metric(&mut html, "Sessions", &report.sessions.to_string());
    push_metric(&mut html, "Memories", &report.memory_count.to_string());
    push_metric(&mut html, "Topics", &report.memory_topics.len().to_string());
    push_metric(
        &mut html,
        "Graph concepts",
        &report.graph_concepts.to_string(),
    );
    push_metric(
        &mut html,
        "Graph relations",
        &report.graph_relations.to_string(),
    );
    push_metric(
        &mut html,
        "Feedback corrections",
        &report.feedback_corrections.to_string(),
    );
    push_metric(
        &mut html,
        "Transcript messages",
        &report.transcript_stats.message_count.to_string(),
    );
    push_metric(
        &mut html,
        "Pending extractions",
        &report.pending_extractions.to_string(),
    );
    html.push_str("</section>");

    html.push_str("<h2>Memory Topics</h2><table><tr><th>Topic</th><th>Count</th></tr>");
    for topic in &report.memory_topics {
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td></tr>",
            escape_html(&topic.topic),
            topic.memory_count
        ));
    }
    html.push_str("</table>");

    html.push_str("<h2>Top Noisy Commands</h2><table><tr><th>Command</th></tr>");
    for command in &report.top_noisy_commands {
        html.push_str(&format!(
            "<tr><td><code>{}</code></td></tr>",
            escape_html(command)
        ));
    }
    html.push_str("</table>");

    html.push_str("<h2>Integration Health</h2><table><tr><th>Integration</th><th>Status</th></tr>");
    for (name, status) in &report.integration_health {
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td></tr>",
            escape_html(name),
            escape_html(status)
        ));
    }
    html.push_str("</table>");
    html.push_str("</main></body></html>\n");
    html
}

fn push_metric(html: &mut String, label: &str, value: &str) {
    html.push_str(&format!(
        "<div class=\"metric\"><div class=\"label\">{}</div><div class=\"value\">{}</div></div>",
        escape_html(label),
        escape_html(value)
    ));
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
