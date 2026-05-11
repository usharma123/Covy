use anyhow::Result;
use clap::Args;
use serde::Serialize;

use crate::memory_store::{
    inspect_graph, list_memories_filtered, local_store_stats, recall_memories_filtered,
    search_feedback_filtered, search_transcripts_filtered, FeedbackRecord, GraphInspect,
    LocalStoreStats, MemoryListQuery, MemoryRecallQuery, MemoryRecord, TranscriptMessage,
};

#[derive(Args)]
pub struct WakeupArgs {
    /// Optional focus query for recalled memories and feedback
    #[arg(long)]
    pub query: Option<String>,

    /// Optional project filter for recalled memories, feedback, and transcripts
    #[arg(long)]
    pub project: Option<String>,

    /// Maximum memories, feedback records, concepts, and relations to include
    #[arg(long, default_value_t = 5)]
    pub limit: usize,

    /// Approximate token budget for the rendered wake-up pack
    #[arg(long, default_value_t = 500)]
    pub max_tokens: usize,

    /// Rendered pack format: markdown or plain
    #[arg(long, default_value = "markdown")]
    pub format: String,

    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub pretty: bool,
}

#[derive(Serialize)]
pub(crate) struct WakeupReport {
    kind: &'static str,
    query: Option<String>,
    project: Option<String>,
    format: String,
    max_tokens: usize,
    estimated_tokens: usize,
    truncated: bool,
    pack: String,
    included_items: Vec<WakeupIncludedItem>,
    stats: LocalStoreStats,
    memories: Vec<MemoryRecord>,
    feedback: Vec<FeedbackRecord>,
    transcripts: Vec<TranscriptMessage>,
    graph: GraphInspect,
}

#[derive(Clone, Serialize)]
pub(crate) struct WakeupIncludedItem {
    source: &'static str,
    id: String,
    section: &'static str,
    score: f64,
    estimated_tokens: usize,
    text: String,
}

#[derive(Clone)]
struct WakeupCandidate {
    source: &'static str,
    id: String,
    section: &'static str,
    score: f64,
    text: String,
}

pub(crate) fn build_wakeup_report_with_options(
    query: Option<&str>,
    project: Option<&str>,
    limit: usize,
    max_tokens: usize,
    format: &str,
) -> Result<WakeupReport> {
    let limit = limit.max(1);
    let max_tokens = max_tokens.max(1);
    let format = normalize_wakeup_format(format)?;
    let query = query.map(str::trim).filter(|q| !q.is_empty());
    let project = project.map(str::trim).filter(|q| !q.is_empty());
    let memories = match query {
        Some(query) => recall_memories_filtered(MemoryRecallQuery {
            query,
            limit,
            topic: None,
            project,
            tag: None,
            keyword: None,
        })?,
        None => list_memories_filtered(MemoryListQuery {
            limit,
            topic: None,
            project,
            all: false,
            sort: "recent",
        })?,
    };
    let feedback = search_feedback_filtered(query.unwrap_or_default(), project, limit)?;
    let transcripts = search_transcripts_filtered(query.unwrap_or_default(), project, limit)?;
    let graph = inspect_graph(limit)?;
    let stats = local_store_stats()?;
    let (pack, included_items, estimated_tokens, truncated) = render_budgeted_pack(
        &memories,
        &feedback,
        &transcripts,
        &graph,
        query,
        max_tokens,
        &format,
    );
    Ok(WakeupReport {
        kind: "packet28.wakeup.v1",
        query: query.map(ToOwned::to_owned),
        project: project.map(ToOwned::to_owned),
        format,
        max_tokens,
        estimated_tokens,
        truncated,
        pack,
        included_items,
        stats,
        memories,
        feedback,
        transcripts,
        graph,
    })
}

pub(crate) fn build_wakeup_pack_for_injection(
    query: Option<&str>,
    project: Option<&str>,
    limit: usize,
    max_tokens: usize,
) -> Result<Option<String>> {
    let report = build_wakeup_report_with_options(query, project, limit, max_tokens, "markdown")?;
    if report.included_items.is_empty() {
        return Ok(None);
    }
    Ok(Some(report.pack))
}

pub fn run(args: WakeupArgs) -> Result<i32> {
    let report = build_wakeup_report_with_options(
        args.query.as_deref(),
        args.project.as_deref(),
        args.limit,
        args.max_tokens,
        &args.format,
    )?;
    if args.json {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else {
        print!("{}", report.pack);
    }
    Ok(0)
}

fn normalize_wakeup_format(format: &str) -> Result<String> {
    let format = format.trim().to_ascii_lowercase();
    match format.as_str() {
        "" | "markdown" | "md" => Ok("markdown".to_string()),
        "plain" | "text" => Ok("plain".to_string()),
        other => anyhow::bail!("unsupported wakeup format '{other}'"),
    }
}

fn render_budgeted_pack(
    memories: &[MemoryRecord],
    feedback: &[FeedbackRecord],
    transcripts: &[TranscriptMessage],
    graph: &GraphInspect,
    query: Option<&str>,
    max_tokens: usize,
    format: &str,
) -> (String, Vec<WakeupIncludedItem>, usize, bool) {
    let mut candidates = wakeup_candidates(memories, feedback, transcripts, graph, query);
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.source.cmp(b.source))
            .then_with(|| a.id.cmp(&b.id))
    });
    let max_chars = max_tokens.saturating_mul(4);
    let mut used_chars = 0usize;
    let mut included = Vec::new();
    let mut truncated = false;
    for candidate in candidates {
        let line_chars = candidate.text.len() + candidate.section.len() + 8;
        if used_chars.saturating_add(line_chars) > max_chars && !included.is_empty() {
            truncated = true;
            continue;
        }
        used_chars = used_chars.saturating_add(line_chars);
        included.push(WakeupIncludedItem {
            source: candidate.source,
            id: candidate.id,
            section: candidate.section,
            score: (candidate.score * 100.0).round() / 100.0,
            estimated_tokens: estimate_tokens(&candidate.text),
            text: candidate.text,
        });
        if used_chars >= max_chars {
            truncated = true;
            break;
        }
    }
    let estimated_tokens = estimate_tokens_for_items(&included);
    let pack = render_pack(&included, max_tokens, estimated_tokens, truncated, format);
    (pack, included, estimated_tokens, truncated)
}

fn wakeup_candidates(
    memories: &[MemoryRecord],
    feedback: &[FeedbackRecord],
    transcripts: &[TranscriptMessage],
    graph: &GraphInspect,
    query: Option<&str>,
) -> Vec<WakeupCandidate> {
    let mut candidates = Vec::new();
    for memory in memories {
        candidates.push(WakeupCandidate {
            source: "memory",
            id: memory.id.to_string(),
            section: memory_section(&memory.importance),
            score: memory_score(memory, query),
            text: format!(
                "{} [{}{}]",
                one_line(&memory.content),
                memory.topic,
                memory
                    .project
                    .as_deref()
                    .map(|project| format!(", project={project}"))
                    .unwrap_or_default()
            ),
        });
    }
    for item in feedback {
        candidates.push(WakeupCandidate {
            source: "feedback",
            id: item.id.to_string(),
            section: "Corrections",
            score: 35.0 + query_bonus(&item.correction, query) + query_bonus(&item.subject, query),
            text: format!(
                "{} -> {}{}",
                one_line(&item.subject),
                one_line(&item.correction),
                item.project
                    .as_deref()
                    .map(|project| format!(" [project={project}]"))
                    .unwrap_or_default()
            ),
        });
    }
    for transcript in transcripts {
        candidates.push(WakeupCandidate {
            source: "transcript",
            id: transcript.id.to_string(),
            section: "Recent transcript context",
            score: 25.0 + query_bonus(&transcript.content, query),
            text: format!(
                "{} {}: {}{}",
                one_line(&transcript.session_key),
                one_line(&transcript.role),
                one_line(&transcript.content),
                transcript
                    .project
                    .as_deref()
                    .map(|project| format!(" [project={project}]"))
                    .unwrap_or_default()
            ),
        });
    }
    for concept in &graph.concepts {
        candidates.push(WakeupCandidate {
            source: "graph",
            id: concept.id.to_string(),
            section: "Graph concepts",
            score: 20.0 + concept.confidence * 10.0 + query_bonus(&concept.name, query),
            text: format!(
                "{}: {}",
                one_line(&concept.name),
                one_line(concept.description.as_deref().unwrap_or(""))
            ),
        });
    }
    candidates
}

fn memory_score(memory: &MemoryRecord, query: Option<&str>) -> f64 {
    let importance = match memory.importance.as_str() {
        "critical" => 100.0,
        "high" => 75.0,
        "medium" => 45.0,
        "low" => 20.0,
        _ => 30.0,
    };
    importance
        + memory.weight.max(0.0) * 5.0
        + memory.recall_score.unwrap_or(0.0).max(0.0) * 10.0
        + query_bonus(&memory.content, query)
        + query_bonus(&memory.topic, query)
}

fn memory_section(importance: &str) -> &'static str {
    match importance {
        "critical" => "Critical memories",
        "high" => "Important memories",
        _ => "Project context",
    }
}

fn query_bonus(text: &str, query: Option<&str>) -> f64 {
    let Some(query) = query else {
        return 0.0;
    };
    let query = query.to_ascii_lowercase();
    if query.is_empty() {
        return 0.0;
    }
    let text = text.to_ascii_lowercase();
    if text.contains(&query) {
        12.0
    } else {
        0.0
    }
}

fn render_pack(
    items: &[WakeupIncludedItem],
    max_tokens: usize,
    estimated_tokens: usize,
    truncated: bool,
    format: &str,
) -> String {
    if items.is_empty() {
        return "(no Packet28 wake-up context matched)\n".to_string();
    }
    if format == "plain" {
        let mut out = String::new();
        for item in items {
            out.push_str(&format!("- [{}:{}] {}\n", item.source, item.id, item.text));
        }
        if truncated {
            out.push_str(&format!(
                "- truncated to approximately {estimated_tokens}/{max_tokens} tokens\n"
            ));
        }
        return out;
    }
    let mut out = format!(
        "## Packet28 Wake-Up Pack\n\n_budget: {estimated_tokens}/{max_tokens} tokens{}_\n",
        if truncated { ", truncated" } else { "" }
    );
    let mut last_section = "";
    for item in items {
        if item.section != last_section {
            out.push_str(&format!("\n### {}\n", item.section));
            last_section = item.section;
        }
        out.push_str(&format!(
            "- [{}:{} score={:.2}] {}\n",
            item.source, item.id, item.score, item.text
        ));
    }
    out
}

fn estimate_tokens_for_items(items: &[WakeupIncludedItem]) -> usize {
    estimate_tokens(
        &items
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn estimate_tokens(text: &str) -> usize {
    (text.len().saturating_add(3) / 4).max(1)
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
