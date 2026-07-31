use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};

use super::super::{FffMcpClient, McpSessionState};

pub(super) fn mcp_fff_auto_prefer_requested() -> bool {
    matches!(
        std::env::var("P28_FFF_AUTO").ok().as_deref(),
        Some("prefer" | "PREFER" | "first" | "FIRST" | "1" | "true" | "TRUE" | "on" | "ON")
    )
}

pub(super) fn execute_fff_search_with_session(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    request: &packet28_reducer_core::SearchRequest,
) -> Result<packet28_reducer_core::SearchResult> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for attempt in 0..2 {
        let text = {
            let mut guard = session
                .lock()
                .map_err(|err| anyhow!("failed to lock MCP session: {err}"))?;
            let needs_spawn = guard
                .fff_client
                .as_ref()
                .map(|client| client.root != canonical_root)
                .unwrap_or(true);
            if needs_spawn {
                guard.fff_client = Some(FffMcpClient::spawn(&canonical_root)?);
            }
            let result = guard
                .fff_client
                .as_mut()
                .ok_or_else(|| anyhow!("fff MCP backend was not initialized"))?
                .call_grep(request);
            match result {
                Ok(text) => text,
                Err(_) if attempt == 0 => {
                    guard.fff_client = None;
                    drop(guard);
                    continue;
                }
                Err(error) => return Err(error),
            }
        };
        return Ok(parse_fff_grep_text(&canonical_root, request, &text));
    }
    Err(anyhow!("fff MCP backend failed after restart"))
}

fn parse_fff_grep_text(
    root: &Path,
    request: &packet28_reducer_core::SearchRequest,
    text: &str,
) -> packet28_reducer_core::SearchResult {
    let mut groups =
        std::collections::BTreeMap::<String, Vec<packet28_reducer_core::SearchMatch>>::new();
    let mut current_path: Option<String> = None;
    let max_per_file = request.max_matches_per_file.unwrap_or(usize::MAX);
    let max_total = request.max_total_matches.unwrap_or(usize::MAX);
    let mut total_matches = 0usize;
    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if line.is_empty()
            || line.starts_with('→')
            || line.starts_with("cursor:")
            || line.contains(" matches shown")
            || line.starts_with("0 ")
        {
            continue;
        }
        if let Some((line_no, body)) = parse_fff_match_line(line) {
            if let Some(path) = current_path.clone() {
                if !fff_path_allowed(&path, &request.requested_paths) {
                    continue;
                }
                let file_matches = groups.entry(path.clone()).or_default();
                if file_matches.len() < max_per_file && total_matches < max_total {
                    file_matches.push(packet28_reducer_core::SearchMatch {
                        path,
                        line: line_no,
                        text: body.to_string(),
                    });
                    total_matches += 1;
                }
            }
            continue;
        }
        if root.join(line).exists() || line.contains('/') || line.contains('\\') {
            current_path = Some(line.to_string());
        }
    }

    let match_count = groups.values().map(Vec::len).sum::<usize>();
    let paths = groups.keys().cloned().collect::<Vec<_>>();
    let mut regions = Vec::new();
    let mut compact_preview = String::new();
    let groups = groups
        .into_iter()
        .map(|(path, matches)| {
            for item in &matches {
                regions.push(format!("{}:{}-{}", item.path, item.line, item.line));
                compact_preview.push_str(&format!(
                    "{}:{}:{}\n",
                    item.path,
                    item.line,
                    item.text.trim()
                ));
            }
            packet28_reducer_core::SearchGroup {
                path,
                match_count: matches.len(),
                displayed_match_count: matches.len(),
                truncated: false,
                matches,
            }
        })
        .collect::<Vec<_>>();
    let returned_match_count = regions.len();
    packet28_reducer_core::SearchResult {
        query: request.query.clone(),
        requested_paths: request.requested_paths.clone(),
        resolved_paths: paths.clone(),
        match_count,
        returned_match_count,
        truncated: match_count > returned_match_count,
        paths,
        regions,
        symbols: Vec::new(),
        groups,
        compact_preview,
        diagnostics: vec!["experimental persistent fff MCP backend".to_string()],
        engine: Some(packet28_reducer_core::SearchEngineStats {
            engine: "fff_mcp".to_string(),
            ..packet28_reducer_core::SearchEngineStats::default()
        }),
    }
}

fn parse_fff_match_line(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let (number, body) = trimmed.split_once(':')?;
    let line_no = number.trim().parse::<usize>().ok()?;
    Some((line_no, body.trim_start()))
}

fn fff_path_allowed(path: &str, requested_paths: &[String]) -> bool {
    if requested_paths.is_empty() {
        return true;
    }
    let normalized = normalize_fff_path(path);
    requested_paths.iter().any(|requested| {
        let requested = normalize_fff_path(requested);
        requested == "."
            || normalized == requested
            || normalized.starts_with(&format!("{requested}/"))
    })
}

fn normalize_fff_path(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_string()
}
