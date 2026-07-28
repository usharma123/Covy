use serde_json::{json, Value};

use super::McpToolset;

fn cursor_safe_tool_name(name: &str) -> String {
    name.strip_prefix("packet28.")
        .map(|suffix| format!("packet28_{suffix}"))
        .unwrap_or_else(|| name.to_string())
}

pub(super) fn canonical_tool_name(name: &str) -> String {
    let dotted = name
        .strip_prefix("packet28_")
        .map(|suffix| format!("packet28.{suffix}"))
        .unwrap_or_else(|| name.to_string());
    match dotted.as_str() {
        "packet28.handoff" => "packet28.prepare_handoff".to_string(),
        _ => dotted,
    }
}

fn rewrite_tool_names_for_cursor(payload: &mut Value) {
    let Some(tools) = payload.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };

    for tool in tools {
        if let Some(name) = tool.get("name").and_then(Value::as_str) {
            let safe_name = cursor_safe_tool_name(name);
            tool["name"] = Value::String(safe_name);
        }
    }
}

fn filter_tools_for_toolset(payload: &mut Value, toolset: McpToolset) {
    if matches!(toolset, McpToolset::All) {
        return;
    }
    let Some(tools) = payload.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };
    tools.retain(|tool| {
        tool.get("name")
            .and_then(Value::as_str)
            .is_some_and(is_core_mcp_tool)
    });
}

fn is_core_mcp_tool(name: &str) -> bool {
    matches!(
        canonical_tool_name(name).as_str(),
        "packet28.search"
            | "packet28.search_fast"
            | "packet28.read_regions"
            | "packet28.glob"
            | "packet28.fetch_tool_result"
            | "packet28.fetch_raw_output"
            | "packet28.fetch_context"
            | "packet28.prepare_handoff"
            | "packet28.write_intention"
            | "packet28.task_status"
            | "packet28.capabilities"
    )
}
pub(super) fn tools_list_payload(toolset: McpToolset) -> Value {
    let mut tools = crate::cmd_mcp::native_tools::tool_descriptors();
    tools.extend(
        json!([
                {
                    "name": "packet28.verify_experiments",
                    "description": "Verify Packet28 experiment manifest evidence, artifacts, metric gates, runtime versions, and required workflow coverage.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "manifest": {"type":"string"},
                            "require_workflows": {"type":"array","items":{"type":"string"}}
                        }
                    }
                },
                {
                    "name": "packet28.reducer_drift",
                    "description": "Replay reducer golden raw-output fixtures and flag missing decisive markers from compact summaries or previews.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "fixture": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.hypothesis_add",
                    "description": "Record an active task hypothesis so it flows into Packet28 broker context and handoff state.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["text"],
                        "properties": {
                            "task_id": {"type":"string"},
                            "id": {"type":"string"},
                            "text": {"type":"string"},
                            "paths": {"type":"array","items":{"type":"string"}},
                            "symbols": {"type":"array","items":{"type":"string"}},
                            "artifact_id": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.hypothesis_list",
                    "description": "List active task hypotheses from the Packet28 broker snapshot.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.hypothesis_resolve",
                    "description": "Confirm or reject an active Packet28 task hypothesis.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["id", "status"],
                        "properties": {
                            "task_id": {"type":"string"},
                            "id": {"type":"string"},
                            "status": {"type":"string","enum":["confirmed","rejected"]},
                            "note": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.reduce",
                    "description": "Reduce command stdout/stderr into a compact Packet28 packet without executing the command.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["command"],
                        "properties": {
                            "command": {"type":"string"},
                            "stdout": {"type":"string"},
                            "stderr": {"type":"string"},
                            "exit_code": {"type":"integer"}
                        }
                    }
                },
                {
                    "name": "packet28.rewrite",
                    "description": "Plan the Packet28 reducer/native-tool/proxy rewrite for a shell command.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["command"],
                        "properties": {
                            "command": {"type":"string"},
                            "task_id": {"type":"string"},
                            "session_id": {"type":"string"},
                            "cwd": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.doctor",
                    "description": "Run Packet28 doctor and return its JSON health report.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "agent": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_store",
                    "description": "Store a local Packet28 memory in ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["content"],
                        "properties": {
                            "content": {"type":"string"},
                            "tags": {"type":"string"},
                            "topic": {"type":"string"},
                            "importance": {"type":"string"},
                            "keywords": {"type":"string"},
                            "project": {"type":"string"},
                            "source": {"type":"string"},
                            "raw_excerpt": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_recall",
                    "description": "Recall local Packet28 memories from ~/.packet28/packet28.db using keyword search.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["query"],
                        "properties": {
                            "query": {"type":"string"},
                            "limit": {"type":"integer","minimum":1},
                            "topic": {"type":"string"},
                            "project": {"type":"string"},
                            "tag": {"type":"string"},
                            "keyword": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_list",
                    "description": "List recent local Packet28 memories from ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1},
                            "topic": {"type":"string"},
                            "project": {"type":"string"},
                            "all": {"type":"boolean"},
                            "sort": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_update",
                    "description": "Update a local Packet28 memory by id.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["id"],
                        "properties": {
                            "id": {"type":"integer"},
                            "content": {"type":"string"},
                            "tags": {"type":"string"},
                            "topic": {"type":"string"},
                            "importance": {"type":"string"},
                            "keywords": {"type":"string"},
                            "project": {"type":"string"},
                            "source": {"type":"string"},
                            "raw_excerpt": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_forget",
                    "description": "Delete a local Packet28 memory by id, or delete memories in a topic.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {"type":"integer"},
                            "topic": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_topics",
                    "description": "List local Packet28 memory topics and counts.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.memory_stats",
                    "description": "Return local Packet28 memory and store statistics.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.memory_health",
                    "description": "Return local Packet28 memory topic health, staleness, and consolidation signals.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "topic": {"type":"string"},
                            "stale_after_days": {"type":"integer","minimum":0},
                            "consolidation_threshold": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.memory_lint",
                    "description": "Lint local Packet28 memories for runtime-specific advice, stale repo paths, and unsupported hook assumptions.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.context_anomalies",
                    "description": "Rank compact context anomalies from dashboard quality signals and return next-check commands.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.verify_context_anomalies",
                    "description": "Verify context anomaly thresholds using the same compact digest as Packet28 verify context-anomalies.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "max_anomalies": {"type":"integer","minimum":0},
                            "max_high": {"type":"integer","minimum":0},
                            "max_trend_age_ms": {"type":"integer","minimum":0}
                        }
                    }
                },
                {
                    "name": "packet28.memory_consolidate",
                    "description": "Consolidate local Packet28 memories for a topic into one deterministic summary memory.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "topic": {"type":"string"},
                            "keep_originals": {"type":"boolean"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_decay",
                    "description": "Apply local weight decay to non-critical Packet28 memories.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "factor": {"type":"number","minimum":0,"maximum":1}
                        }
                    }
                },
                {
                    "name": "packet28.memory_prune",
                    "description": "Delete low-weight non-critical Packet28 memories, or preview candidates with dry_run.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "threshold": {"type":"number","minimum":0,"maximum":1},
                            "dry_run": {"type":"boolean"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_embed",
                    "description": "Create local deterministic memory embeddings in ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {"type":"integer"},
                            "all": {"type":"boolean"},
                            "dimensions": {"type":"integer","minimum":8}
                        }
                    }
                },
                {
                    "name": "packet28.memory_extract_patterns",
                    "description": "Detect recurring local memory patterns in a topic and optionally create graph concepts.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["topic"],
                        "properties": {
                            "topic": {"type":"string"},
                            "memoir": {"type":"string"},
                            "min_cluster_size": {"type":"integer","minimum":2}
                        }
                    }
                },
                {
                    "name": "packet28.memory_pending_enqueue",
                    "description": "Queue raw local tool/session text for later Packet28 memory extraction.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["raw_output"],
                        "properties": {
                            "raw_output": {"type":"string"},
                            "project": {"type":"string"},
                            "tool_name": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_pending_list",
                    "description": "List queued Packet28 pending memory extractions.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.memory_pending_process",
                    "description": "Process queued Packet28 pending extractions into durable local memories.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1},
                            "dry_run": {"type":"boolean"}
                        }
                    }
                },
                {
                    "name": "packet28.memory_pending_delete",
                    "description": "Delete queued Packet28 pending extraction rows by id.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["ids"],
                        "properties": {
                            "ids": {"type":"array","items":{"type":"integer"}}
                        }
                    }
                },
                {
                    "name": "packet28.memory_pending_stats",
                    "description": "Return queued Packet28 pending extraction counts.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.feedback_record",
                    "description": "Record a local feedback correction in ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["subject", "correction"],
                        "properties": {
                            "subject": {"type":"string"},
                            "correction": {"type":"string"},
                            "topic": {"type":"string"},
                            "context": {"type":"string"},
                            "predicted": {"type":"string"},
                            "reason": {"type":"string"},
                            "source": {"type":"string"},
                            "project": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.feedback_search",
                    "description": "Search local feedback corrections in ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["query"],
                        "properties": {
                            "query": {"type":"string"},
                            "memoir": {"type":"string"},
                            "label": {"type":"string"},
                            "project": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.feedback_list",
                    "description": "List local feedback corrections from ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "topic": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.feedback_apply",
                    "description": "Increment the applied count for a feedback correction.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["id"],
                        "properties": {
                            "id": {"type":"integer"}
                        }
                    }
                },
                {
                    "name": "packet28.feedback_delete",
                    "description": "Delete a local feedback correction by id.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["id"],
                        "properties": {
                            "id": {"type":"integer"}
                        }
                    }
                },
                {
                    "name": "packet28.feedback_stats",
                    "description": "Return local feedback correction statistics.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.wakeup",
                    "description": "Build a compact local wake-up pack from memory, feedback, transcripts, graph, and stats.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {"type":"string"},
                            "project": {"type":"string"},
                            "paths": {"type":"array","items":{"type":"string"}},
                            "symbols": {"type":"array","items":{"type":"string"}},
                            "intent": {"type":"string"},
                            "limit": {"type":"integer","minimum":1},
                            "max_tokens": {"type":"integer","minimum":1},
                            "format": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.learn_project",
                    "description": "Scan a local project into Packet28 graph concepts and relations.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "directory": {"type":"string"},
                            "name": {"type":"string"},
                            "memoir": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_append",
                    "description": "Append a local transcript message to ~/.packet28/packet28.db.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["content"],
                        "properties": {
                            "content": {"type":"string"},
                            "session": {"type":"string"},
                            "agent": {"type":"string"},
                            "role": {"type":"string"},
                            "source": {"type":"string"},
                            "project": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_list",
                    "description": "List local transcript sessions.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_show",
                    "description": "Show local transcript messages for a session.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["session"],
                        "properties": {
                            "session": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_search",
                    "description": "Search local transcript messages with FTS and LIKE fallback.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["query"],
                        "properties": {
                            "query": {"type":"string"},
                            "memoir": {"type":"string"},
                            "label": {"type":"string"},
                            "project": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_stats",
                    "description": "Return local transcript session and message statistics.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.transcript_export",
                    "description": "Export local transcript messages as Packet28 transcript JSON.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "session": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.transcript_import",
                    "description": "Import Packet28 transcript JSON into the local transcript store.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["content"],
                        "properties": {
                            "content": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.graph_create",
                    "description": "Create or update a Packet28 memoir-style graph container.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": {"type":"string"},
                            "description": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.graph_list",
                    "description": "List Packet28 memoir-style graph containers.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.graph_show",
                    "description": "Show one Packet28 memoir-style graph container with concepts and relations.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.graph_add_concept",
                    "description": "Add or update a local Packet28 graph concept.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {
                            "name": {"type":"string"},
                            "description": {"type":"string"},
                            "memoir": {"type":"string"},
                            "labels": {"type":"array","items":{"type":"string"}},
                            "confidence": {"type":"number","minimum":0,"maximum":1},
                            "source_ids": {"type":"array","items":{"type":"string"}}
                        }
                    }
                },
                {
                    "name": "packet28.graph_refine",
                    "description": "Refine a local Packet28 graph concept definition.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["name", "description"],
                        "properties": {
                            "name": {"type":"string"},
                            "description": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.graph_link",
                    "description": "Create a typed relation between two local Packet28 graph concepts.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["source", "target"],
                        "properties": {
                            "source": {"type":"string"},
                            "target": {"type":"string"},
                            "relation": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.graph_search",
                    "description": "Search local Packet28 graph concepts.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["query"],
                        "properties": {
                            "query": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.graph_export",
                    "description": "Export the local Packet28 graph as json, dot, or ascii.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "format": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.graph_stats",
                    "description": "Return local Packet28 graph counts and relation type statistics.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "packet28.graph_delete",
                    "description": "Delete a local Packet28 graph concept and attached relations.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {
                            "name": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.graph_inspect",
                    "description": "Inspect local Packet28 graph concepts and relations.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.graph_inspect_concept",
                    "description": "Inspect one Packet28 graph concept and its relation neighborhood.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {
                            "name": {"type":"string"},
                            "memoir": {"type":"string"},
                            "depth": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.graph_distill",
                    "description": "Distill memories from a Packet28 topic into memoir-style graph concepts.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["from_topic"],
                        "properties": {
                            "from_topic": {"type":"string"},
                            "into": {"type":"string"},
                            "limit": {"type":"integer","minimum":1}
                        }
                    }
                },
                {
                    "name": "packet28.write_intention",
                    "description": "Persist the current task objective and worker intent into Packet28.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["text"],
                        "properties": {
                            "task_id": {"type":"string"},
                            "text": {"type":"string"},
                            "note": {"type":"string"},
                            "step_id": {"type":"string"},
                            "question_id": {"type":"string"},
                            "paths": {"type":"array","items":{"type":"string"}},
                            "symbols": {"type":"array","items":{"type":"string"}}
                        }
                    }
                },
                {
                    "name": "packet28.task_status",
                    "description": "Return current Packet28 task status and handoff state.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type":"string"}
                        }
                    }
                },
                {
                    "name": "packet28.capabilities",
                    "description": "Describe the active Packet28 hooks-first runtime contract.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                }
        ])
        .as_array()
        .into_iter()
        .flatten()
        .cloned(),
    );
    let mut payload = json!({"tools": tools});
    filter_tools_for_toolset(&mut payload, toolset);
    rewrite_tool_names_for_cursor(&mut payload);
    payload
}
