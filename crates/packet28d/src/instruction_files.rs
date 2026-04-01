use super::*;
use packet28_daemon_core::{
    active_task_path, ActiveTaskRecord, ContextBackendKind, ContextResolveOutcome,
    ContextResolveRequest, ContextResolveResponse, ContextSourceKind,
    InstructionFileResolveOutcome, InstructionFileResolveRequest, InstructionFileResolveResponse,
};

pub(crate) fn resolve_context(
    state: Arc<Mutex<DaemonState>>,
    request: ContextResolveRequest,
) -> Result<ContextResolveResponse> {
    let source_path = request
        .source_path
        .clone()
        .filter(|value| !value.trim().is_empty());
    let display_path = source_path
        .clone()
        .unwrap_or_else(|| format!("{:?}", request.source_kind).to_ascii_lowercase());
    let source_content = request.source_content.clone();

    if display_path.trim().is_empty() {
        return Ok(passthrough_response(
            request.source_kind,
            None,
            "empty_path",
            request.source_sha256,
            request.task_label.or(request.task_id),
            Some(source_content.len()),
        ));
    }
    if source_content.trim().is_empty() {
        return Ok(passthrough_response(
            request.source_kind,
            source_path,
            "empty_content",
            request.source_sha256,
            request.task_label.or(request.task_id),
            Some(source_content.len()),
        ));
    }

    let root = state.lock().map_err(lock_err)?.root.clone();
    let task_id = request
        .task_id
        .clone()
        .filter(|value: &String| !value.trim().is_empty())
        .or_else(|| load_active_task_id(&root));
    let kernel = state.lock().map_err(lock_err)?.kernel.clone();
    let response = kernel.execute(KernelRequest {
        target: "packet28.instruction.summarize".to_string(),
        reducer_input: serde_json::to_value(context_kernel_core::InstructionSummaryRequest {
            path: display_path.clone(),
            content: source_content.clone(),
            content_sha256: request.source_sha256.clone(),
            task_id: task_id.clone(),
            budget_tokens: request.budget_tokens,
            schema_version: if request.schema_version == 0 {
                context_kernel_core::INSTRUCTION_SUMMARY_SCHEMA_VERSION
            } else {
                request.schema_version
            },
            source_kind: Some(format_source_kind(request.source_kind).to_string()),
            backend_kind: Some(format_backend_kind(request.backend_kind).to_string()),
            agent_family: request.agent_family.clone(),
        })?,
        policy_context: json!({
            "task_id": task_id,
            "source_kind": format_source_kind(request.source_kind),
            "backend_kind": format_backend_kind(request.backend_kind),
            "agent_family": request.agent_family,
        }),
        ..KernelRequest::default()
    });

    let response = match response {
        Ok(response) => response,
        Err(err) => {
            return Ok(passthrough_response(
                request.source_kind,
                source_path,
                &format!("summarization_failed:{err}"),
                request.source_sha256,
                request.task_label.or(request.task_id),
                Some(source_content.len()),
            ));
        }
    };
    let cache_hit = response
        .metadata
        .get("cache")
        .and_then(|value| value.get("hit"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let packet = match response.output_packets.first() {
        Some(packet) => packet,
        None => {
            return Ok(passthrough_response(
                request.source_kind,
                source_path,
                "missing_summary_packet",
                request.source_sha256,
                request.task_label.or(request.task_id),
                Some(source_content.len()),
            ));
        }
    };
    let envelope: suite_packet_core::EnvelopeV1<context_kernel_core::InstructionSummaryPayload> =
        match serde_json::from_value(packet.body.clone()) {
            Ok(envelope) => envelope,
            Err(err) => {
                return Ok(passthrough_response(
                    request.source_kind,
                    source_path,
                    &format!("invalid_summary_packet:{err}"),
                    request.source_sha256,
                    request.task_label.or(request.task_id),
                    Some(source_content.len()),
                ));
            }
        };

    let payload = envelope.payload;
    if payload.summary_text.len() >= source_content.len() {
        return Ok(passthrough_response(
            request.source_kind,
            source_path,
            "not_smaller_than_original",
            Some(payload.content_sha256),
            Some(payload.task_label),
            Some(payload.original_bytes),
        ));
    }

    Ok(ContextResolveResponse {
        source_kind: request.source_kind,
        source_path,
        outcome: ContextResolveOutcome::Rewrite {
            content: payload.summary_text,
            content_sha256: payload.content_sha256,
            task_label: payload.task_label,
            original_bytes: payload.original_bytes,
            rewritten_bytes: payload.rewritten_bytes,
            cache_hit,
            matched_terms: payload.matched_terms,
            section_titles: payload.section_titles,
            schema_version: payload.schema_version,
        },
    })
}

pub(crate) fn resolve_instruction_file(
    state: Arc<Mutex<DaemonState>>,
    request: InstructionFileResolveRequest,
) -> Result<InstructionFileResolveResponse> {
    let path = request.path.clone();
    let response = resolve_context(
        state,
        ContextResolveRequest {
            workspace_root: request.workspace_root,
            source_kind: ContextSourceKind::InstructionFile,
            source_path: Some(path.clone()),
            source_sha256: request.content_sha256,
            source_content: request.content,
            task_id: request.task_id,
            task_label: None,
            budget_tokens: request.budget_tokens,
            schema_version: request.schema_version,
            agent_family: None,
            backend_kind: ContextBackendKind::Unknown,
        },
    )?;

    Ok(InstructionFileResolveResponse {
        path: path.clone(),
        outcome: match response.outcome {
            ContextResolveOutcome::Rewrite {
                content,
                content_sha256,
                task_label,
                original_bytes,
                rewritten_bytes,
                cache_hit,
                matched_terms,
                section_titles,
                ..
            } => InstructionFileResolveOutcome::Rewrite {
                content,
                content_sha256,
                task_label,
                original_bytes,
                rewritten_bytes,
                cache_hit,
                matched_terms,
                section_titles,
            },
            ContextResolveOutcome::Passthrough {
                reason,
                content_sha256,
                task_label,
                original_bytes,
            } => InstructionFileResolveOutcome::Passthrough {
                reason,
                content_sha256,
                task_label,
                original_bytes,
            },
        },
    })
}

fn load_active_task_id(root: &Path) -> Option<String> {
    let path = active_task_path(root);
    let raw = fs::read_to_string(path).ok()?;
    let record = serde_json::from_str::<ActiveTaskRecord>(&raw).ok()?;
    let task_id = record.task_id.trim();
    (!task_id.is_empty()).then(|| task_id.to_string())
}

fn passthrough_response(
    source_kind: ContextSourceKind,
    path: Option<String>,
    reason: &str,
    content_sha256: impl Into<Option<String>>,
    task_label: impl Into<Option<String>>,
    original_bytes: Option<usize>,
) -> ContextResolveResponse {
    ContextResolveResponse {
        source_kind,
        source_path: path,
        outcome: ContextResolveOutcome::Passthrough {
            reason: reason.to_string(),
            content_sha256: content_sha256.into(),
            task_label: task_label.into(),
            original_bytes,
        },
    }
}

fn format_source_kind(kind: ContextSourceKind) -> &'static str {
    match kind {
        ContextSourceKind::InstructionFile => "instruction_file",
        ContextSourceKind::SystemPromptFragment => "system_prompt_fragment",
    }
}

fn format_backend_kind(kind: ContextBackendKind) -> &'static str {
    match kind {
        ContextBackendKind::LinuxPreload => "linux_preload",
        ContextBackendKind::LinuxOci => "linux_oci",
        ContextBackendKind::MacosFuse => "macos_fuse",
        ContextBackendKind::WindowsFuse => "windows_fuse",
        ContextBackendKind::ProxyOnly => "proxy_only",
        ContextBackendKind::Unknown => "unknown",
    }
}
