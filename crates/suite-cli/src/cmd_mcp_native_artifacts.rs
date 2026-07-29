use super::*;
use crate::cmd_mcp::support::{load_raw_output_artifact, load_tool_result_artifact};

pub(crate) fn handle_packet28_fetch_tool_result(
    root: &Path,
    args: Packet28FetchToolResultArgs,
) -> Result<Value> {
    let task_id = args.task_id.as_str();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.fetch_tool_result requires task_id"));
    }
    let (artifact_id, mut payload) = load_tool_result_artifact(
        root,
        task_id,
        args.artifact_id.as_deref(),
        args.invocation_id.as_deref(),
    )?;
    if payload.get("artifact_id").is_none() {
        payload["artifact_id"] = json!(artifact_id.clone());
    }
    if payload.get("response_mode").is_none() {
        payload["response_mode"] = json!("full");
    }
    compact_fetched_tool_result_payload(&mut payload);
    Ok(payload)
}

pub(super) fn compact_fetched_tool_result_payload(payload: &mut Value) {
    if payload.get("groups").and_then(Value::as_array).is_some() {
        compact_fetched_search_payload(payload);
    }
}

fn compact_fetched_search_payload(payload: &mut Value) {
    let content = render_search_artifact_content(payload);
    if !content.is_empty() {
        payload["content"] = json!(content);
        payload["line_count"] = json!(payload["content"].as_str().unwrap_or("").lines().count());
        payload["content_format"] = json!("path:line:text");
    }
    if let Some(object) = payload.as_object_mut() {
        object.remove("groups");
    }
}

fn render_search_artifact_content(payload: &Value) -> String {
    let mut lines = Vec::new();
    let Some(groups) = payload.get("groups").and_then(Value::as_array) else {
        return String::new();
    };
    for group in groups {
        let Some(matches) = group.get("matches").and_then(Value::as_array) else {
            continue;
        };
        let group_path = group
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for item in matches {
            let path = item
                .get("path")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(group_path);
            let Some(line) = item.get("line").and_then(Value::as_u64) else {
                continue;
            };
            let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
            lines.push(format!("{path}:{line}:{text}"));
        }
    }
    lines.join("\n")
}

pub(crate) fn handle_packet28_fetch_raw_output(
    root: &Path,
    args: Packet28FetchRawOutputArgs,
) -> Result<Value> {
    let task_id = args.task_id.as_str();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.fetch_raw_output requires task_id"));
    }
    let (path, content) = load_raw_output_artifact(root, task_id, &args.handle)?;
    Ok(json!({
        "task_id": task_id,
        "handle": args.handle,
        "path": path,
        "content": content,
        "line_count": content.lines().count(),
    }))
}

pub(crate) fn handle_packet28_fetch_context(
    root: &Path,
    args: Packet28FetchContextArgs,
) -> Result<Value> {
    let task_id = args.task_id.as_str();
    if task_id.is_empty() {
        return Err(anyhow!("packet28.fetch_context requires task_id"));
    }
    let artifact_id = args
        .artifact_id
        .or(args.context_version)
        .ok_or_else(|| anyhow!("packet28.fetch_context requires artifact_id or context_version"))?;
    let (_path, bytes) = read_validated_context_artifact(root, task_id, &artifact_id)?;
    let mut payload: Value = serde_json::from_slice(&bytes)?;
    validate_context_artifact_identity(&payload, &artifact_id)?;
    // Honour response_mode: when slim is requested, strip heavy section
    // data and keep only the metadata the agent needs to decide next steps.
    if matches!(args.response_mode, Some(BrokerResponseMode::Slim)) {
        if let Some(obj) = payload.as_object_mut() {
            obj.remove("sections");
            obj.remove("delta");
            obj.remove("evidence_cache");
            obj.remove("search_evidence");
            obj.remove("code_evidence");
        }
        payload["response_mode"] = json!("slim");
    } else if payload.get("response_mode").is_none() {
        payload["response_mode"] = json!("full");
    }
    Ok(payload)
}
