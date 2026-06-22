use std::collections::BTreeSet;
use std::path::Path;

use crate::memory_store_types::{HookEventRecord, MemoryLintIssue, MemoryLintReport, MemoryRecord};

pub(crate) fn lint_memory_records(
    root: &Path,
    memories: &[MemoryRecord],
    hook_events: &[HookEventRecord],
) -> MemoryLintReport {
    let hook_runtimes = hook_events
        .iter()
        .map(|event| event.runtime.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mut issues = Vec::new();
    for memory in memories {
        let lint_text = memory_lint_text(memory);
        let lower = lint_text.to_ascii_lowercase();
        let runtimes = mentioned_agent_runtimes(&lower);
        if !runtimes.is_empty() && looks_like_runtime_specific_advice(&lower) {
            issues.push(MemoryLintIssue {
                memory_id: memory.id,
                kind: "runtime_specific_memory".to_string(),
                detail: format!("mentions {}", runtimes.join(",")),
            });
        }
        for path in referenced_repo_paths(&lint_text) {
            if !root.join(&path).exists() {
                issues.push(MemoryLintIssue {
                    memory_id: memory.id,
                    kind: "stale_path".to_string(),
                    detail: path,
                });
            }
        }
        if looks_like_hook_support_claim(&lower) {
            for runtime in runtimes {
                if !hook_runtimes.contains(runtime) {
                    issues.push(MemoryLintIssue {
                        memory_id: memory.id,
                        kind: "unsupported_runtime_assumption".to_string(),
                        detail: format!("no hook evidence for {runtime}"),
                    });
                }
            }
        }
    }
    let issue_count = issues.len();
    MemoryLintReport {
        memory_count: memories.len(),
        hook_runtime_count: hook_runtimes.len(),
        issue_count,
        issues,
        summary: format!(
            "memory_lint memories={} issues={issue_count} hook_runtimes={}",
            memories.len(),
            hook_runtimes.len()
        ),
    }
}

fn memory_lint_text(memory: &MemoryRecord) -> String {
    [
        Some(memory.content.as_str()),
        memory.tags.as_deref(),
        memory.keywords.as_deref(),
        memory.source.as_deref(),
        memory.raw_excerpt.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n")
}

fn mentioned_agent_runtimes(lower: &str) -> Vec<&'static str> {
    let mut runtimes = Vec::new();
    for (runtime, needles) in [
        ("claude", &["claude code", "claude"] as &[&str]),
        ("cursor", &["cursor"]),
        ("gemini", &["gemini"]),
        ("windsurf", &["windsurf"]),
        ("copilot", &["copilot"]),
        ("opencode", &["opencode", "open code"]),
        ("codex", &["codex"]),
    ] {
        if needles.iter().any(|needle| lower.contains(needle)) {
            runtimes.push(runtime);
        }
    }
    runtimes
}

fn looks_like_runtime_specific_advice(lower: &str) -> bool {
    contains_any_text(
        lower,
        &[
            " only ",
            " always ",
            " never ",
            " must ",
            "hook",
            "pretooluse",
            "transparent rewrite",
            "updatedinput",
            "agent-specific",
            "runtime-specific",
        ],
    )
}

fn looks_like_hook_support_claim(lower: &str) -> bool {
    contains_any_text(
        lower,
        &[
            "transparent rewrite",
            "hook support",
            "supports hooks",
            "pretooluse",
            "updatedinput",
        ],
    )
}

fn contains_any_text(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn referenced_repo_paths(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut paths = Vec::new();
    for token in text.split_whitespace() {
        let candidate = token.trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\'' | '`' | ',' | ';' | ':' | ')' | '(' | '[' | ']' | '{' | '}'
            )
        });
        let candidate = candidate.trim_start_matches("./");
        if !looks_like_repo_path(candidate) {
            continue;
        }
        if seen.insert(candidate.to_string()) {
            paths.push(candidate.to_string());
        }
    }
    paths
}

fn looks_like_repo_path(candidate: &str) -> bool {
    if candidate.starts_with('/')
        || candidate.starts_with("http://")
        || candidate.starts_with("https://")
        || candidate.contains('$')
        || candidate.contains("..")
        || !candidate.contains('/')
    {
        return false;
    }
    candidate.ends_with(".rs")
        || candidate.ends_with(".md")
        || candidate.ends_with(".toml")
        || candidate.ends_with(".json")
        || candidate.ends_with(".yml")
        || candidate.ends_with(".yaml")
        || candidate.ends_with(".ts")
        || candidate.ends_with(".tsx")
        || candidate.ends_with(".js")
        || candidate.ends_with(".py")
        || candidate.ends_with(".sh")
}
