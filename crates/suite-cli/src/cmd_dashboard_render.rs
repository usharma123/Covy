use std::io::{self, Write};

use anyhow::Result;

use super::{ContextHiddenSample, DashboardReport};

#[derive(Clone, Copy)]
pub(super) enum DashboardPanel {
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

pub(super) fn run_dashboard_tui(report: &DashboardReport) -> Result<()> {
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

pub(super) fn render_dashboard_tui(report: &DashboardReport, panel: DashboardPanel) -> String {
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
            out.push_str(&format!(
                "handoff_latest_status={}\nhandoff_regression_count={}\n",
                report.handoff_readiness.latest_status, report.handoff_readiness.regression_count
            ));
            out.push_str(&format!(
                "reducer_drift_latest_status={}\nreducer_drift_latest_issue_count={}\n",
                report.reducer_drift.latest_status, report.reducer_drift.latest_issue_count
            ));
            out.push_str(&format!(
                "memory_lint_latest_status={}\nmemory_lint_latest_issue_count={}\n",
                report.memory_lint.latest_status, report.memory_lint.latest_issue_count
            ));
            out.push_str(&format!(
                "context_anomaly_latest_status={}\ncontext_anomaly_latest_high_count={}\ncontext_anomaly_latest_age_ms={}\ncontext_anomaly_oldest_recurring_hidden_age_ms={}\ncontext_anomaly_recurring_hidden_samples={}\n",
                report.context_anomalies.latest_status,
                report.context_anomalies.latest_high_count,
                report.context_anomalies.latest_age_ms,
                report.context_anomalies.oldest_recurring_hidden_age_ms,
                context_hidden_sample_summary(&report.context_anomalies.recurring_hidden_samples)
            ));
            out.push_str("handoff_latest_blockers:\n");
            push_tui_list(
                &mut out,
                &report.handoff_readiness.latest_blocking_categories,
            );
            out.push_str("top_saved_routes:\n");
            for route in &report.top_saved_routes {
                out.push_str(&format!(
                    "- {} saved={} pct={:.1}\n",
                    route.route, route.saved_est_tokens, route.savings_percent
                ));
            }
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

pub(super) fn render_dashboard_html(report: &DashboardReport) -> String {
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
    push_metric(
        &mut html,
        "Handoff status",
        &report.handoff_readiness.latest_status,
    );
    push_metric(
        &mut html,
        "Handoff regressions",
        &report.handoff_readiness.regression_count.to_string(),
    );
    push_metric(
        &mut html,
        "Reducer drift",
        &report.reducer_drift.latest_status,
    );
    push_metric(&mut html, "Memory lint", &report.memory_lint.latest_status);
    push_metric(
        &mut html,
        "Context anomalies",
        &report.context_anomalies.latest_status,
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

    html.push_str("<h2>Top Saved Routes</h2><table><tr><th>Route</th><th>Saved tokens</th><th>Savings</th></tr>");
    for route in &report.top_saved_routes {
        html.push_str(&format!(
            "<tr><td><code>{}</code></td><td>{}</td><td>{:.1}%</td></tr>",
            escape_html(&route.route),
            route.saved_est_tokens,
            route.savings_percent
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

    html.push_str("<h2>Handoff Readiness</h2><table><tr><th>Signal</th><th>Value</th></tr>");
    html.push_str(&format!(
        "<tr><td>Latest status</td><td>{}</td></tr>",
        escape_html(&report.handoff_readiness.latest_status)
    ));
    html.push_str(&format!(
        "<tr><td>Latest blockers</td><td><code>{}</code></td></tr>",
        escape_html(
            &report
                .handoff_readiness
                .latest_blocking_categories
                .join(",")
        )
    ));
    html.push_str(&format!(
        "<tr><td>Recurring categories</td><td><code>{}</code></td></tr>",
        escape_html(&report.handoff_readiness.recurring_categories.join(","))
    ));
    html.push_str(&format!(
        "<tr><td>Regressions</td><td>{}</td></tr>",
        report.handoff_readiness.regression_count
    ));
    html.push_str("</table>");

    html.push_str("<h2>Reducer Drift</h2><table><tr><th>Signal</th><th>Value</th></tr>");
    html.push_str(&format!(
        "<tr><td>Latest status</td><td>{}</td></tr>",
        escape_html(&report.reducer_drift.latest_status)
    ));
    html.push_str(&format!(
        "<tr><td>Latest issues</td><td>{}</td></tr>",
        report.reducer_drift.latest_issue_count
    ));
    html.push_str(&format!(
        "<tr><td>Failing families</td><td><code>{}</code></td></tr>",
        escape_html(&report.reducer_drift.latest_failing_families.join(","))
    ));
    html.push_str(&format!(
        "<tr><td>Recurring issues</td><td><code>{}</code></td></tr>",
        escape_html(&report.reducer_drift.recurring_issue_kinds.join(","))
    ));
    html.push_str("</table>");

    html.push_str("<h2>Memory Lint</h2><table><tr><th>Signal</th><th>Value</th></tr>");
    html.push_str(&format!(
        "<tr><td>Latest status</td><td>{}</td></tr>",
        escape_html(&report.memory_lint.latest_status)
    ));
    html.push_str(&format!(
        "<tr><td>Latest issues</td><td>{}</td></tr>",
        report.memory_lint.latest_issue_count
    ));
    html.push_str(&format!(
        "<tr><td>Latest issue kinds</td><td><code>{}</code></td></tr>",
        escape_html(&report.memory_lint.latest_issue_kinds.join(","))
    ));
    html.push_str(&format!(
        "<tr><td>Recurring issues</td><td><code>{}</code></td></tr>",
        escape_html(&report.memory_lint.recurring_issue_kinds.join(","))
    ));
    html.push_str("</table>");

    html.push_str("<h2>Context Anomalies</h2><table><tr><th>Signal</th><th>Value</th></tr>");
    html.push_str(&format!(
        "<tr><td>Latest status</td><td>{}</td></tr>",
        escape_html(&report.context_anomalies.latest_status)
    ));
    html.push_str(&format!(
        "<tr><td>Latest anomalies</td><td>{}</td></tr>",
        report.context_anomalies.latest_anomaly_count
    ));
    html.push_str(&format!(
        "<tr><td>Latest high</td><td>{}</td></tr>",
        report.context_anomalies.latest_high_count
    ));
    html.push_str(&format!(
        "<tr><td>Latest age ms</td><td>{}</td></tr>",
        report.context_anomalies.latest_age_ms
    ));
    html.push_str(&format!(
        "<tr><td>Oldest recurring hidden age ms</td><td>{}</td></tr>",
        report.context_anomalies.oldest_recurring_hidden_age_ms
    ));
    html.push_str(&format!(
        "<tr><td>Latest hidden</td><td><code>{}</code></td></tr>",
        escape_html(&report.context_anomalies.latest_hidden_categories.join(","))
    ));
    html.push_str(&format!(
        "<tr><td>Recurring hidden</td><td><code>{}</code></td></tr>",
        escape_html(
            &report
                .context_anomalies
                .recurring_hidden_categories
                .join(",")
        )
    ));
    html.push_str(&format!(
        "<tr><td>Recurring hidden samples</td><td><code>{}</code></td></tr>",
        escape_html(&context_hidden_sample_summary(
            &report.context_anomalies.recurring_hidden_samples
        ))
    ));
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

pub(super) fn context_hidden_sample_summary(samples: &[ContextHiddenSample]) -> String {
    samples
        .iter()
        .map(|sample| {
            format!(
                "{}={}",
                escape_context_hidden_summary_segment(&sample.category, true),
                escape_context_hidden_summary_segment(&sample.signal, false)
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn escape_context_hidden_summary_segment(value: &str, escape_equals: bool) -> String {
    let escaped = value
        .replace('%', "%25")
        .replace('\n', "%0A")
        .replace(';', "%3B");
    if escape_equals {
        escaped.replace('=', "%3D")
    } else {
        escaped
    }
}
