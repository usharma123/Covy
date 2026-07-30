use super::*;
use packet28_daemon_protocol::hooks::ActiveTaskRecord;
use packet28_daemon_protocol::message::{
    ContextBackendKind, ContextResolveOutcome, ContextResolveRequest, ContextResolveResponse,
    ContextSourceKind, InstructionFileResolveOutcome, InstructionFileResolveRequest,
    InstructionFileResolveResponse,
};
use packet28_daemon_protocol::paths::active_task_path;

pub(crate) const INSTRUCTION_EXPERIMENT_CONFIG_FILE: &str = "packet28-instruction.json";

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct InstructionExperimentConfigFile {
    schema_version: u32,
    mode: suite_packet_core::InstructionRenderMode,
    #[serde(default)]
    stable_config: InstructionExperimentStableConfigFile,
}

#[derive(Debug, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct InstructionExperimentStableConfigFile {
    renderer_version: u32,
    max_sections: usize,
    max_lines_per_section: usize,
    max_focus_terms: usize,
    focus_terms: Vec<String>,
}

impl Default for InstructionExperimentStableConfigFile {
    fn default() -> Self {
        let config = suite_packet_core::InstructionStableConfig::default();
        Self {
            renderer_version: config.renderer_version,
            max_sections: config.max_sections,
            max_lines_per_section: config.max_lines_per_section,
            max_focus_terms: config.max_focus_terms,
            focus_terms: config.focus_terms,
        }
    }
}

impl From<InstructionExperimentStableConfigFile> for suite_packet_core::InstructionStableConfig {
    fn from(config: InstructionExperimentStableConfigFile) -> Self {
        Self {
            renderer_version: config.renderer_version,
            max_sections: config.max_sections,
            max_lines_per_section: config.max_lines_per_section,
            max_focus_terms: config.max_focus_terms,
            focus_terms: config.focus_terms,
        }
    }
}

pub(crate) fn resolve_context(
    state: Arc<Mutex<DaemonState>>,
    request: ContextResolveRequest,
) -> Result<ContextResolveResponse> {
    let source_path = request
        .source_path
        .clone()
        .filter(|value| !value.trim().is_empty());
    let source_content = request.source_content.clone();
    let root = state.lock().map_err(lock_err)?.root.clone();
    let display_path =
        match canonical_instruction_path(&root, source_path.as_deref(), request.source_kind) {
            Ok(path) => path,
            Err(reason) => {
                return Ok(passthrough_response(
                    request.source_kind,
                    source_path,
                    reason,
                    request.source_sha256,
                    request.task_label.or(request.task_id),
                    Some(source_content.len()),
                ));
            }
        };
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

    let (render_mode, stable_config) =
        match resolve_instruction_config(&root, request.render_mode, request.stable_config.clone())
        {
            Ok(config) => config,
            Err(reason) => {
                return Ok(passthrough_response(
                    request.source_kind,
                    source_path,
                    &reason,
                    request.source_sha256,
                    request.task_label.or(request.task_id),
                    Some(source_content.len()),
                ));
            }
        };
    if render_mode == suite_packet_core::InstructionRenderMode::Passthrough {
        return Ok(passthrough_response(
            request.source_kind,
            source_path,
            "mode_passthrough",
            request.source_sha256,
            request.task_label.or(request.task_id),
            Some(source_content.len()),
        ));
    }
    let task_id = if render_mode == suite_packet_core::InstructionRenderMode::Adaptive {
        request
            .task_id
            .clone()
            .filter(|value: &String| !value.trim().is_empty())
            .or_else(|| load_active_task_id(&root))
    } else {
        None
    };
    let kernel = state.lock().map_err(lock_err)?.kernel.clone();
    let response = kernel.execute(KernelRequest {
        target: "packet28.instruction.summarize".to_string(),
        reducer_input: serde_json::to_value(context_kernel_core::InstructionSummaryRequest {
            path: display_path,
            content: source_content.clone(),
            content_sha256: request.source_sha256.clone(),
            mode: render_mode,
            stable_config,
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
            "instruction_mode": render_mode,
            "task_id": if render_mode == suite_packet_core::InstructionRenderMode::Adaptive {
                task_id
            } else {
                None
            },
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
            render_mode: payload.mode,
            stable_config_sha256: payload.stable_config_sha256,
            snapshot_sha256: payload.snapshot_sha256,
            rendered_sha256: payload.rendered_sha256,
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
            render_mode: request.render_mode,
            stable_config: request.stable_config,
            task_id: request.task_id,
            task_label: None,
            budget_tokens: request.budget_tokens,
            schema_version: request.schema_version,
            agent_family: None,
            backend_kind: ContextBackendKind::Unknown,
        },
    )?;

    Ok(InstructionFileResolveResponse {
        path,
        outcome: match response.outcome {
            ContextResolveOutcome::Rewrite {
                content,
                content_sha256,
                render_mode,
                stable_config_sha256,
                snapshot_sha256,
                rendered_sha256,
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
                render_mode,
                stable_config_sha256,
                snapshot_sha256,
                rendered_sha256,
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

fn resolve_instruction_config(
    root: &Path,
    requested_mode: Option<suite_packet_core::InstructionRenderMode>,
    requested_config: Option<suite_packet_core::InstructionStableConfig>,
) -> std::result::Result<
    (
        suite_packet_core::InstructionRenderMode,
        suite_packet_core::InstructionStableConfig,
    ),
    String,
> {
    if let Some(mode) = requested_mode {
        let config = requested_config.unwrap_or_default();
        if !config.has_supported_renderer_version() {
            return Err("instruction_renderer_version_unsupported".to_string());
        }
        return Ok((mode, config.normalized()));
    }

    let path = root.join(INSTRUCTION_EXPERIMENT_CONFIG_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok((
                suite_packet_core::InstructionRenderMode::Passthrough,
                suite_packet_core::InstructionStableConfig::default(),
            ));
        }
        Err(_) => return Err("instruction_config_unreadable".to_string()),
    };
    let canonical_root =
        fs::canonicalize(root).map_err(|_| "workspace_root_unavailable".to_string())?;
    let canonical_path =
        fs::canonicalize(path).map_err(|_| "instruction_config_unreadable".to_string())?;
    if !canonical_path.starts_with(canonical_root) {
        return Err("instruction_config_outside_workspace".to_string());
    }
    let raw = fs::read_to_string(canonical_path)
        .map_err(|_| "instruction_config_unreadable".to_string())?;
    let config = serde_json::from_str::<InstructionExperimentConfigFile>(&raw)
        .map_err(|_| "instruction_config_invalid".to_string())?;
    if config.schema_version != 1 {
        return Err("instruction_config_schema_unsupported".to_string());
    }
    let stable_config = suite_packet_core::InstructionStableConfig::from(config.stable_config);
    if !stable_config.has_supported_renderer_version() {
        return Err("instruction_renderer_version_unsupported".to_string());
    }
    Ok((config.mode, stable_config.normalized()))
}

fn load_active_task_id(root: &Path) -> Option<String> {
    let path = active_task_path(root);
    let raw = fs::read_to_string(path).ok()?;
    let record = serde_json::from_str::<ActiveTaskRecord>(&raw).ok()?;
    let task_id = record.task_id.trim();
    (!task_id.is_empty()).then(|| task_id.to_string())
}

fn canonical_instruction_path(
    root: &Path,
    source_path: Option<&str>,
    source_kind: ContextSourceKind,
) -> std::result::Result<String, &'static str> {
    let Some(source_path) = source_path else {
        return Ok(format_source_kind(source_kind).to_string());
    };
    let candidate = Path::new(source_path.trim());
    let canonical_root = fs::canonicalize(root).map_err(|_| "workspace_root_unavailable")?;
    let relative = if candidate.is_absolute() {
        candidate
            .strip_prefix(root)
            .or_else(|_| candidate.strip_prefix(&canonical_root))
            .map_err(|_| "source_path_outside_workspace")?
    } else {
        candidate
    };
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => {
                parts.push(part.to_string_lossy().into_owned());
            }
            std::path::Component::ParentDir => return Err("source_path_outside_workspace"),
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err("source_path_outside_workspace");
            }
        }
    }
    if parts.is_empty() {
        return Err("empty_path");
    }

    let mut resolved = canonical_root.clone();
    for (index, part) in parts.iter().enumerate() {
        let next = resolved.join(part);
        match fs::symlink_metadata(&next) {
            Ok(_) => {
                resolved = fs::canonicalize(&next).map_err(|_| "source_path_unreadable")?;
                if !resolved.starts_with(&canonical_root) {
                    return Err("source_path_outside_workspace");
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                resolved.extend(&parts[index..]);
                break;
            }
            Err(_) => return Err("source_path_unreadable"),
        }
    }

    resolved
        .strip_prefix(&canonical_root)
        .map_err(|_| "source_path_outside_workspace")?
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .reduce(|mut path, component| {
            path.push('/');
            path.push_str(&component);
            path
        })
        .filter(|path| !path.is_empty())
        .ok_or("empty_path")
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
        ContextBackendKind::MacosSwap => "macos_swap",
        ContextBackendKind::MacosFuse => "macos_fuse",
        ContextBackendKind::WindowsFuse => "windows_fuse",
        ContextBackendKind::ProxyOnly => "proxy_only",
        ContextBackendKind::Unknown => "unknown",
    }
}
