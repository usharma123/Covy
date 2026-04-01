use super::*;
use sha2::{Digest, Sha256};

pub const DEFAULT_INSTRUCTION_SUMMARY_BUDGET_TOKENS: u64 = 512;
pub const INSTRUCTION_SUMMARY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct InstructionSummaryRequest {
    pub path: String,
    pub content: String,
    pub content_sha256: String,
    pub task_id: Option<String>,
    pub budget_tokens: Option<u64>,
    pub schema_version: u32,
    pub source_kind: Option<String>,
    pub backend_kind: Option<String>,
    pub agent_family: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct InstructionSummaryPayload {
    pub path: String,
    pub content_sha256: String,
    pub task_label: String,
    pub schema_version: u32,
    pub source_kind: String,
    pub backend_kind: String,
    pub agent_family: String,
    pub original_bytes: usize,
    pub rewritten_bytes: usize,
    pub matched_terms: Vec<String>,
    pub section_titles: Vec<String>,
    pub summary_text: String,
}

#[derive(Debug, Clone)]
struct MarkdownSection {
    heading: Option<String>,
    lines: Vec<String>,
}

pub(crate) fn run_packet28_instruction_summarize(
    ctx: &mut ExecutionContext,
    _input_packets: &[KernelPacket],
) -> Result<ReducerResult, KernelError> {
    let request: InstructionSummaryRequest = serde_json::from_value(ctx.reducer_input.clone())
        .map_err(|source| KernelError::ReducerFailed {
            target: ctx.target.clone(),
            detail: format!("invalid reducer input: {source}"),
        })?;
    if request.path.trim().is_empty() {
        return Err(KernelError::InvalidRequest {
            detail: "packet28.instruction.summarize requires reducer_input.path".to_string(),
        });
    }
    if request.content.trim().is_empty() {
        return Err(KernelError::InvalidRequest {
            detail: "packet28.instruction.summarize requires reducer_input.content".to_string(),
        });
    }

    let task_label = request
        .task_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("default")
        .to_string();
    let budget_tokens = request
        .budget_tokens
        .unwrap_or(DEFAULT_INSTRUCTION_SUMMARY_BUDGET_TOKENS)
        .max(96);
    let budget_bytes = (budget_tokens as usize).saturating_mul(4).max(384);
    let snapshot = if let Some(task_id) = request
        .task_id
        .as_deref()
        .filter(|task_id| !task_id.trim().is_empty())
    {
        Some(derive_agent_snapshot(&ctx.cache_entries()?, task_id))
    } else {
        None
    };
    let rendered = render_instruction_summary(
        &request.path,
        &request.content,
        &request.content_sha256,
        &task_label,
        snapshot.as_ref(),
        budget_bytes,
    );

    let payload = InstructionSummaryPayload {
        path: request.path.clone(),
        content_sha256: rendered.content_sha256.clone(),
        task_label: task_label.clone(),
        schema_version: if request.schema_version == 0 {
            INSTRUCTION_SUMMARY_SCHEMA_VERSION
        } else {
            request.schema_version
        },
        source_kind: request
            .source_kind
            .clone()
            .unwrap_or_else(|| "instruction_file".to_string()),
        backend_kind: request
            .backend_kind
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        agent_family: request
            .agent_family
            .clone()
            .unwrap_or_else(|| "generic".to_string()),
        original_bytes: request.content.len(),
        rewritten_bytes: rendered.summary_text.len(),
        matched_terms: rendered.matched_terms.clone(),
        section_titles: rendered.section_titles.clone(),
        summary_text: rendered.summary_text.clone(),
    };
    let payload_bytes = serde_json::to_vec(&payload).unwrap_or_default().len();
    let envelope = suite_packet_core::EnvelopeV1 {
        version: "1".to_string(),
        tool: "packet28".to_string(),
        kind: "instruction_summary".to_string(),
        hash: String::new(),
        summary: format!(
            "instruction summary path={} task={} sections={}",
            payload.path,
            payload.task_label,
            payload.section_titles.len()
        ),
        files: vec![suite_packet_core::FileRef {
            path: payload.path.clone(),
            relevance: Some(1.0),
            source: Some("packet28.instruction.summarize".to_string()),
        }],
        symbols: Vec::new(),
        risk: None,
        confidence: Some(if payload.rewritten_bytes < payload.original_bytes {
            0.88
        } else {
            0.55
        }),
        budget_cost: suite_packet_core::BudgetCost {
            est_tokens: 0,
            est_bytes: 0,
            runtime_ms: 0,
            tool_calls: 1,
            payload_est_tokens: Some((payload_bytes / 4) as u64),
            payload_est_bytes: Some(payload_bytes),
        },
        provenance: suite_packet_core::Provenance {
            inputs: vec![
                format!("task:{task_label}"),
                format!("instruction:{}", payload.path),
            ],
            git_base: None,
            git_head: None,
            generated_at_unix: now_unix(),
        },
        payload: payload.clone(),
    }
    .with_canonical_hash_and_real_budget();

    ctx.set_shared("task_id", Value::String(task_label.clone()));
    ctx.set_shared(
        "instruction_summary",
        json!({
            "path": payload.path,
            "rewritten_bytes": payload.rewritten_bytes,
            "matched_terms": payload.matched_terms,
            "source_kind": payload.source_kind,
            "backend_kind": payload.backend_kind,
            "agent_family": payload.agent_family,
        }),
    );

    let packet = KernelPacket {
        packet_id: Some(format!(
            "instruction-summary-{}",
            envelope.hash.chars().take(12).collect::<String>()
        )),
        format: default_packet_format(),
        body: serde_json::to_value(&envelope).map_err(|source| KernelError::ReducerFailed {
            target: ctx.target.clone(),
            detail: source.to_string(),
        })?,
        token_usage: Some(envelope.budget_cost.est_tokens),
        runtime_ms: Some(envelope.budget_cost.runtime_ms),
        metadata: json!({
            "tool": "packet28",
            "reducer": "packet28.instruction.summarize",
            "kind": "instruction_summary",
            "path": request.path,
            "task_id": task_label,
            "source_kind": payload.source_kind,
            "backend_kind": payload.backend_kind,
            "agent_family": payload.agent_family,
            "matched_terms": payload.matched_terms,
            "section_titles": payload.section_titles,
            "original_bytes": payload.original_bytes,
            "rewritten_bytes": payload.rewritten_bytes,
            "hash": envelope.hash,
        }),
    };

    Ok(ReducerResult {
        output_packets: vec![packet],
        metadata: json!({
            "reducer": "packet28.instruction.summarize",
            "path": payload.path,
            "task_id": payload.task_label,
            "source_kind": payload.source_kind,
            "backend_kind": payload.backend_kind,
            "agent_family": payload.agent_family,
            "matched_terms": payload.matched_terms,
            "original_bytes": payload.original_bytes,
            "rewritten_bytes": payload.rewritten_bytes,
        }),
    })
}

#[derive(Debug, Clone)]
struct RenderedInstructionSummary {
    content_sha256: String,
    matched_terms: Vec<String>,
    section_titles: Vec<String>,
    summary_text: String,
}

fn render_instruction_summary(
    path: &str,
    content: &str,
    content_sha256: &str,
    task_label: &str,
    snapshot: Option<&suite_packet_core::AgentSnapshotPayload>,
    budget_bytes: usize,
) -> RenderedInstructionSummary {
    let content_sha256 = if content_sha256.trim().is_empty() {
        format!("{:x}", Sha256::digest(content.as_bytes()))
    } else {
        content_sha256.trim().to_string()
    };
    let mut matched_terms = derive_focus_terms(path, task_label, snapshot)
        .into_iter()
        .collect::<Vec<_>>();
    matched_terms.sort();
    if matched_terms.len() > 12 {
        matched_terms.truncate(12);
    }

    let sections = parse_markdown_sections(content);
    let mut scored = sections
        .iter()
        .map(|section| {
            let score = score_section(section, &matched_terms);
            let rendered = render_section_excerpt(section, &matched_terms, 6);
            (section, score, rendered)
        })
        .filter(|(_, _, rendered)| !rendered.is_empty())
        .collect::<Vec<_>>();
    scored.sort_by(
        |(left_section, left_score, _), (right_section, right_score, _)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_section.heading.cmp(&right_section.heading))
        },
    );

    let mut section_titles = Vec::new();
    let mut body = String::new();
    if !matched_terms.is_empty() {
        body.push_str("focus: ");
        body.push_str(&matched_terms.join(", "));
        body.push_str("\n\n");
    }

    let mut appended = 0usize;
    for (section, score, rendered) in scored {
        if appended >= 4 {
            break;
        }
        let include = score > 0 || appended == 0;
        if !include {
            continue;
        }
        if let Some(heading) = section.heading.as_ref() {
            section_titles.push(heading.clone());
            body.push_str("## ");
            body.push_str(heading);
            body.push('\n');
        } else if appended == 0 {
            section_titles.push("Overview".to_string());
            body.push_str("## Overview\n");
        }
        for line in rendered {
            body.push_str(&line);
            body.push('\n');
        }
        body.push('\n');
        appended += 1;
    }

    if body.trim().is_empty() {
        body.push_str("## Overview\n");
        for line in compact_fallback_lines(content, 8) {
            body.push_str(&line);
            body.push('\n');
        }
        section_titles.push("Overview".to_string());
    }

    let header = format!(
        "# [p28:virtual] sha256:{} task:{}\n\n",
        short_sha(&content_sha256),
        task_label
    );
    let summary_text = truncate_markdown(&(header + body.trim_end()), budget_bytes);

    RenderedInstructionSummary {
        content_sha256,
        matched_terms,
        section_titles,
        summary_text,
    }
}

fn derive_focus_terms(
    path: &str,
    task_label: &str,
    snapshot: Option<&suite_packet_core::AgentSnapshotPayload>,
) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    collect_terms(path, &mut terms);
    collect_terms(task_label, &mut terms);
    if let Some(snapshot) = snapshot {
        for path in snapshot.focus_paths.iter().take(6) {
            collect_terms(path, &mut terms);
        }
        for symbol in snapshot.focus_symbols.iter().take(8) {
            collect_terms(symbol, &mut terms);
        }
        for question in snapshot.open_questions.iter().take(4) {
            collect_terms(&question.text, &mut terms);
        }
    }
    terms
}

fn collect_terms(text: &str, terms: &mut BTreeSet<String>) {
    for token in text
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.'))
        .map(|value| value.trim_matches('.').trim())
        .filter(|value| value.len() >= 3)
    {
        terms.insert(token.to_ascii_lowercase());
    }
}

fn parse_markdown_sections(content: &str) -> Vec<MarkdownSection> {
    let mut sections = Vec::new();
    let mut current = MarkdownSection {
        heading: None,
        lines: Vec::new(),
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            if !current.lines.is_empty() || current.heading.is_some() {
                sections.push(current);
            }
            current = MarkdownSection {
                heading: Some(trimmed.trim_start_matches('#').trim().to_string()),
                lines: Vec::new(),
            };
            continue;
        }
        current.lines.push(line.to_string());
    }

    if !current.lines.is_empty() || current.heading.is_some() {
        sections.push(current);
    }
    if sections.is_empty() {
        sections.push(MarkdownSection {
            heading: None,
            lines: content.lines().map(|line| line.to_string()).collect(),
        });
    }
    sections
}

fn score_section(section: &MarkdownSection, matched_terms: &[String]) -> usize {
    let heading = section
        .heading
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let joined = section.lines.join("\n").to_ascii_lowercase();
    matched_terms.iter().fold(0usize, |acc, term| {
        let mut score = acc;
        if heading.contains(term) {
            score += 4;
        }
        if joined.contains(term) {
            score += 1;
        }
        score
    })
}

fn render_section_excerpt(
    section: &MarkdownSection,
    matched_terms: &[String],
    max_lines: usize,
) -> Vec<String> {
    let mut exact = Vec::new();
    let mut fallback = Vec::new();
    for line in &section.lines {
        let compact = compact_line(line);
        if compact.is_empty() {
            continue;
        }
        if matched_terms
            .iter()
            .any(|term| compact.to_ascii_lowercase().contains(term))
        {
            exact.push(compact.clone());
        }
        fallback.push(compact);
    }

    let mut rendered = Vec::new();
    for line in exact.into_iter().chain(fallback.into_iter()) {
        if rendered.iter().any(|existing| existing == &line) {
            continue;
        }
        rendered.push(line);
        if rendered.len() >= max_lines {
            break;
        }
    }
    rendered
}

fn compact_fallback_lines(content: &str, max_lines: usize) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let compact = compact_line(line);
            (!compact.is_empty()).then_some(compact)
        })
        .take(max_lines)
        .collect()
}

fn compact_line(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("#")
        || trimmed
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_digit() && trimmed.contains('.'))
    {
        return truncate_line(trimmed, 220);
    }
    truncate_line(
        &trimmed
            .split_whitespace()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        220,
    )
}

fn truncate_line(line: &str, max_len: usize) -> String {
    if line.len() <= max_len {
        return line.to_string();
    }
    let mut cut = max_len.saturating_sub(3);
    while !line.is_char_boundary(cut) && cut > 0 {
        cut -= 1;
    }
    format!("{}...", &line[..cut])
}

fn truncate_markdown(text: &str, budget_bytes: usize) -> String {
    if text.len() <= budget_bytes {
        return text.to_string();
    }
    let mut cut = budget_bytes.saturating_sub(4);
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut truncated = text[..cut].trim_end().to_string();
    truncated.push_str("\n...");
    truncated
}

fn short_sha(value: &str) -> String {
    value.chars().take(8).collect::<String>()
}
