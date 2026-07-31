use super::support::*;
use crate::instruction_files::{
    resolve_context, resolve_instruction_file, INSTRUCTION_EXPERIMENT_CONFIG_FILE,
};
use packet28_daemon_protocol::message::{
    ContextBackendKind, ContextResolveOutcome, ContextResolveRequest, ContextSourceKind,
    InstructionFileResolveOutcome, InstructionFileResolveRequest, InstructionRenderMode,
    InstructionStableConfig,
};

fn long_instruction_source() -> String {
    "# Repository instructions\n\n\
     General repository guidance remains intentionally verbose so the controlled renderer has a \
     meaningful reduction target. Preserve public behavior, validate failure paths, and keep each \
     change reviewable.\n\n\
     ## Auth\n\
     Authentication changes must preserve session boundaries, validate authorization before data \
     access, and exercise rejection paths in deterministic tests.\n\n\
     ## Cache\n\
     Cache changes must keep identities stable, invalidate changed state, and distinguish local \
     reuse from provider prompt-cache observations.\n\n\
     ## Release\n\
     Release work must use locked dependencies, strict documentation checks, and reproducible \
     packaging commands.\n"
        .to_string()
}

fn context_request(
    workspace_root: String,
    render_mode: Option<InstructionRenderMode>,
    stable_config: Option<InstructionStableConfig>,
) -> ContextResolveRequest {
    ContextResolveRequest {
        workspace_root,
        source_kind: ContextSourceKind::InstructionFile,
        source_path: Some("AGENTS.md".to_string()),
        source_sha256: "untrusted-source-hint".to_string(),
        source_content: long_instruction_source(),
        render_mode,
        stable_config,
        task_id: Some("task-a".to_string()),
        task_label: Some("mutable-label-a".to_string()),
        budget_tokens: Some(128),
        schema_version: 1,
        agent_family: Some("codex".to_string()),
        backend_kind: ContextBackendKind::LinuxPreload,
    }
}

#[test]
fn instruction_file_resolution_rewrites_larger_markdown() {
    let state = daemon_test_state();
    let workspace_root = state.lock().unwrap().root.display().to_string();
    let response = resolve_instruction_file(
        state.clone(),
        InstructionFileResolveRequest {
            workspace_root,
            path: "AGENTS.md".to_string(),
            content_sha256: String::new(),
            content: long_instruction_source(),
            render_mode: Some(InstructionRenderMode::Adaptive),
            stable_config: Some(InstructionStableConfig::default()),
            task_id: Some("task-virtualize".to_string()),
            budget_tokens: Some(128),
            schema_version: 1,
        },
    )
    .unwrap();

    match response.outcome {
        InstructionFileResolveOutcome::Rewrite {
            content,
            task_label,
            original_bytes,
            rewritten_bytes,
            ..
        } => {
            assert!(content.starts_with("# [p28:adaptive:v1]"));
            assert_eq!(task_label, "task-virtualize");
            assert!(rewritten_bytes < original_bytes);
        }
        other => panic!("expected rewrite response, got {other:?}"),
    }
}

#[test]
fn instruction_file_resolution_fails_open_when_summary_is_not_smaller() {
    let state = daemon_test_state();
    let response = resolve_instruction_file(
        state.clone(),
        InstructionFileResolveRequest {
            workspace_root: ".".to_string(),
            path: "AGENTS.md".to_string(),
            content_sha256: String::new(),
            content: "short".to_string(),
            render_mode: Some(InstructionRenderMode::Adaptive),
            stable_config: Some(InstructionStableConfig::default()),
            task_id: None,
            budget_tokens: Some(128),
            schema_version: 1,
        },
    )
    .unwrap();

    match response.outcome {
        InstructionFileResolveOutcome::Passthrough { reason, .. } => {
            assert_eq!(reason, "not_smaller_than_original");
        }
        other => panic!("expected passthrough response, got {other:?}"),
    }
}

#[test]
fn context_resolve_rewrites_instruction_file_and_preserves_metadata() {
    let state = daemon_test_state();
    let workspace_root = state.lock().unwrap().root.display().to_string();
    let response = resolve_context(
        state.clone(),
        ContextResolveRequest {
            workspace_root,
            source_kind: ContextSourceKind::InstructionFile,
            source_path: Some("AGENTS.md".to_string()),
            source_sha256: String::new(),
            source_content: long_instruction_source(),
            render_mode: Some(InstructionRenderMode::Adaptive),
            stable_config: Some(InstructionStableConfig::default()),
            task_id: Some("task-virtualize".to_string()),
            task_label: None,
            budget_tokens: Some(128),
            schema_version: 7,
            agent_family: Some("claude".to_string()),
            backend_kind: ContextBackendKind::LinuxPreload,
        },
    )
    .unwrap();

    assert_eq!(response.source_kind, ContextSourceKind::InstructionFile);
    assert_eq!(response.source_path.as_deref(), Some("AGENTS.md"));
    match response.outcome {
        ContextResolveOutcome::Rewrite {
            content,
            task_label,
            original_bytes,
            rewritten_bytes,
            schema_version,
            ..
        } => {
            assert!(content.starts_with("# [p28:adaptive:v1]"));
            assert_eq!(task_label, "task-virtualize");
            assert!(rewritten_bytes < original_bytes);
            assert_eq!(schema_version, 7);
        }
        other => panic!("expected rewrite response, got {other:?}"),
    }
}

#[test]
fn instruction_file_resolution_compatibility_matches_context_resolve_decision() {
    let state = daemon_test_state();
    let workspace_root = state.lock().unwrap().root.display().to_string();
    let legacy = resolve_instruction_file(
        state.clone(),
        InstructionFileResolveRequest {
            workspace_root: workspace_root.clone(),
            path: "AGENTS.md".to_string(),
            content_sha256: String::new(),
            content: long_instruction_source(),
            render_mode: Some(InstructionRenderMode::Adaptive),
            stable_config: Some(InstructionStableConfig::default()),
            task_id: Some("task-compat".to_string()),
            budget_tokens: Some(128),
            schema_version: 3,
        },
    )
    .unwrap();
    let generic = resolve_context(
        state.clone(),
        ContextResolveRequest {
            workspace_root,
            source_kind: ContextSourceKind::InstructionFile,
            source_path: Some("AGENTS.md".to_string()),
            source_sha256: String::new(),
            source_content: long_instruction_source(),
            render_mode: Some(InstructionRenderMode::Adaptive),
            stable_config: Some(InstructionStableConfig::default()),
            task_id: Some("task-compat".to_string()),
            task_label: None,
            budget_tokens: Some(128),
            schema_version: 3,
            agent_family: Some("generic".to_string()),
            backend_kind: ContextBackendKind::Unknown,
        },
    )
    .unwrap();

    match (legacy.outcome, generic.outcome) {
        (
            InstructionFileResolveOutcome::Rewrite {
                task_label: legacy_task,
                original_bytes: legacy_original,
                rewritten_bytes: legacy_rewritten,
                ..
            },
            ContextResolveOutcome::Rewrite {
                task_label: generic_task,
                original_bytes: generic_original,
                rewritten_bytes: generic_rewritten,
                ..
            },
        ) => {
            assert_eq!(legacy_task, generic_task);
            assert_eq!(legacy_original, generic_original);
            assert_eq!(legacy_rewritten, generic_rewritten);
        }
        other => panic!("expected matching rewrite decisions, got {other:?}"),
    }
}

#[test]
fn absent_instruction_mode_defaults_to_passthrough() {
    let state = daemon_test_state();
    let workspace_root = daemon_test_root(&state).display().to_string();
    let response =
        resolve_context(state.clone(), context_request(workspace_root, None, None)).unwrap();

    match response.outcome {
        ContextResolveOutcome::Passthrough {
            reason,
            original_bytes,
            ..
        } => {
            assert_eq!(reason, "mode_passthrough");
            assert_eq!(original_bytes, Some(long_instruction_source().len()));
        }
        other => panic!("expected default passthrough, got {other:?}"),
    }
}

#[test]
fn repository_config_explicitly_enables_stable_mode() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    std::fs::write(
        root.join(INSTRUCTION_EXPERIMENT_CONFIG_FILE),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "stable",
            "stable_config": {
                "focus_terms": ["auth"]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let response = resolve_context(
        state.clone(),
        context_request(root.display().to_string(), None, None),
    )
    .unwrap();

    match response.outcome {
        ContextResolveOutcome::Rewrite {
            content,
            render_mode,
            snapshot_sha256,
            task_label,
            ..
        } => {
            assert!(content.starts_with("# [p28:stable:v1]"));
            assert_eq!(render_mode, InstructionRenderMode::Stable);
            assert_eq!(snapshot_sha256, None);
            assert_eq!(task_label, "stable");
        }
        other => panic!("expected repository-gated stable rewrite, got {other:?}"),
    }
}

#[test]
fn explicit_passthrough_overrides_repository_experiment_mode() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    std::fs::write(
        root.join(INSTRUCTION_EXPERIMENT_CONFIG_FILE),
        br#"{"schema_version":1,"mode":"adaptive"}"#,
    )
    .unwrap();

    let response = resolve_context(
        state.clone(),
        context_request(
            root.display().to_string(),
            Some(InstructionRenderMode::Passthrough),
            None,
        ),
    )
    .unwrap();

    assert!(matches!(
        response.outcome,
        ContextResolveOutcome::Passthrough { ref reason, .. }
            if reason == "mode_passthrough"
    ));
}

#[test]
fn malformed_and_unsupported_repository_configs_fail_open() {
    let malformed_state = daemon_test_state();
    let malformed_root = daemon_test_root(&malformed_state);
    std::fs::write(
        malformed_root.join(INSTRUCTION_EXPERIMENT_CONFIG_FILE),
        b"{not-json",
    )
    .unwrap();
    let malformed = resolve_context(
        malformed_state.clone(),
        context_request(malformed_root.display().to_string(), None, None),
    )
    .unwrap();
    assert!(matches!(
        malformed.outcome,
        ContextResolveOutcome::Passthrough { ref reason, .. }
            if reason == "instruction_config_invalid"
    ));

    let unsupported_state = daemon_test_state();
    let unsupported_root = daemon_test_root(&unsupported_state);
    std::fs::write(
        unsupported_root.join(INSTRUCTION_EXPERIMENT_CONFIG_FILE),
        br#"{"schema_version":2,"mode":"stable"}"#,
    )
    .unwrap();
    let unsupported = resolve_context(
        unsupported_state.clone(),
        context_request(unsupported_root.display().to_string(), None, None),
    )
    .unwrap();
    assert!(matches!(
        unsupported.outcome,
        ContextResolveOutcome::Passthrough { ref reason, .. }
            if reason == "instruction_config_schema_unsupported"
    ));

    let unsupported_renderer_state = daemon_test_state();
    let unsupported_renderer_root = daemon_test_root(&unsupported_renderer_state);
    std::fs::write(
        unsupported_renderer_root.join(INSTRUCTION_EXPERIMENT_CONFIG_FILE),
        br#"{"schema_version":1,"mode":"stable","stable_config":{"renderer_version":2}}"#,
    )
    .unwrap();
    let unsupported_renderer = resolve_context(
        unsupported_renderer_state.clone(),
        context_request(unsupported_renderer_root.display().to_string(), None, None),
    )
    .unwrap();
    assert!(matches!(
        unsupported_renderer.outcome,
        ContextResolveOutcome::Passthrough { ref reason, .. }
            if reason == "instruction_renderer_version_unsupported"
    ));

    let unknown_nested_field_state = daemon_test_state();
    let unknown_nested_field_root = daemon_test_root(&unknown_nested_field_state);
    std::fs::write(
        unknown_nested_field_root.join(INSTRUCTION_EXPERIMENT_CONFIG_FILE),
        br#"{"schema_version":1,"mode":"stable","stable_config":{"max_section":4}}"#,
    )
    .unwrap();
    let unknown_nested_field = resolve_context(
        unknown_nested_field_state.clone(),
        context_request(unknown_nested_field_root.display().to_string(), None, None),
    )
    .unwrap();
    assert!(matches!(
        unknown_nested_field.outcome,
        ContextResolveOutcome::Passthrough { ref reason, .. }
            if reason == "instruction_config_invalid"
    ));

    let explicit_state = daemon_test_state();
    let explicit_root = daemon_test_root(&explicit_state);
    let explicit = resolve_context(
        explicit_state.clone(),
        context_request(
            explicit_root.display().to_string(),
            Some(InstructionRenderMode::Stable),
            Some(InstructionStableConfig {
                renderer_version: 2,
                ..InstructionStableConfig::default()
            }),
        ),
    )
    .unwrap();
    assert!(matches!(
        explicit.outcome,
        ContextResolveOutcome::Passthrough { ref reason, .. }
            if reason == "instruction_renderer_version_unsupported"
    ));

    let unreadable_state = daemon_test_state();
    let unreadable_root = daemon_test_root(&unreadable_state);
    std::fs::create_dir(unreadable_root.join(INSTRUCTION_EXPERIMENT_CONFIG_FILE)).unwrap();
    let unreadable = resolve_context(
        unreadable_state.clone(),
        context_request(unreadable_root.display().to_string(), None, None),
    )
    .unwrap();
    assert!(matches!(
        unreadable.outcome,
        ContextResolveOutcome::Passthrough { ref reason, .. }
            if reason == "instruction_config_unreadable"
    ));
}

#[test]
fn stable_rewrite_and_cache_identity_ignore_mutable_request_metadata() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    let stable_config = InstructionStableConfig {
        focus_terms: vec!["auth".to_string()],
        ..InstructionStableConfig::default()
    };
    let first = resolve_context(
        state.clone(),
        context_request(
            root.display().to_string(),
            Some(InstructionRenderMode::Stable),
            Some(stable_config.clone()),
        ),
    )
    .unwrap();
    let mut changed = context_request(
        root.display().to_string(),
        Some(InstructionRenderMode::Stable),
        Some(stable_config),
    );
    changed.task_id = Some("task-b".to_string());
    changed.task_label = Some("mutable-label-b".to_string());
    changed.agent_family = Some("claude".to_string());
    changed.backend_kind = ContextBackendKind::MacosSwap;
    changed.source_path = Some(root.join("AGENTS.md").display().to_string());
    let second = resolve_context(state.clone(), changed).unwrap();

    let (
        ContextResolveOutcome::Rewrite {
            content: first_content,
            rendered_sha256: first_sha,
            cache_hit: first_hit,
            ..
        },
        ContextResolveOutcome::Rewrite {
            content: second_content,
            rendered_sha256: second_sha,
            cache_hit: second_hit,
            ..
        },
    ) = (first.outcome, second.outcome)
    else {
        panic!("expected stable rewrites");
    };
    assert!(!first_hit);
    assert!(second_hit);
    assert_eq!(first_content, second_content);
    assert_eq!(first_sha, second_sha);
}

#[test]
fn instruction_path_outside_workspace_fails_open() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    let mut request = context_request(
        root.display().to_string(),
        Some(InstructionRenderMode::Stable),
        Some(InstructionStableConfig::default()),
    );
    request.source_path = Some(root.join("..").join("AGENTS.md").display().to_string());

    let response = resolve_context(state.clone(), request).unwrap();

    assert!(matches!(
        response.outcome,
        ContextResolveOutcome::Passthrough { ref reason, .. }
            if reason == "source_path_outside_workspace"
    ));
}

#[cfg(unix)]
#[test]
fn instruction_path_symlink_escape_fails_open() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("AGENTS.md"), long_instruction_source()).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("outside-link")).unwrap();
    let mut request = context_request(
        root.display().to_string(),
        Some(InstructionRenderMode::Stable),
        Some(InstructionStableConfig::default()),
    );
    request.source_path = Some("outside-link/AGENTS.md".to_string());

    let response = resolve_context(state.clone(), request).unwrap();

    assert!(matches!(
        response.outcome,
        ContextResolveOutcome::Passthrough { ref reason, .. }
            if reason == "source_path_outside_workspace"
    ));
}

#[cfg(unix)]
#[test]
fn repository_config_symlink_escape_fails_open() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    let outside = tempfile::tempdir().unwrap();
    let outside_config = outside.path().join("experiment.json");
    std::fs::write(&outside_config, br#"{"schema_version":1,"mode":"stable"}"#).unwrap();
    std::os::unix::fs::symlink(
        outside_config,
        root.join(INSTRUCTION_EXPERIMENT_CONFIG_FILE),
    )
    .unwrap();

    let response = resolve_context(
        state.clone(),
        context_request(root.display().to_string(), None, None),
    )
    .unwrap();

    assert!(matches!(
        response.outcome,
        ContextResolveOutcome::Passthrough { ref reason, .. }
            if reason == "instruction_config_outside_workspace"
    ));
}
