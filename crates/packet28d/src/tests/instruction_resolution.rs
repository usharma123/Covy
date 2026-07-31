use super::support::*;
use crate::instruction_files::{resolve_context, resolve_instruction_file};
use packet28_daemon_core::{
    ContextBackendKind, ContextResolveOutcome, ContextResolveRequest, ContextSourceKind,
    InstructionFileResolveOutcome, InstructionFileResolveRequest,
};

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
            content: "# Coverage\n\n- Prefer reducer outputs that stay bounded.\n- Avoid redundant tool chatter.\n\n## Auth\nTouch src/auth.rs carefully and preserve task state.\n\n## Testing\nRun targeted checks before widening scope.\n".to_string(),
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
            assert!(content.starts_with("# [p28:virtual] sha256:"));
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
            source_content: "# Coverage\n\n- Prefer reducer outputs that stay bounded.\n- Avoid redundant tool chatter.\n\n## Auth\nTouch src/auth.rs carefully and preserve task state.\n\n## Testing\nRun targeted checks before widening scope.\n".to_string(),
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
            assert!(content.starts_with("# [p28:virtual] sha256:"));
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
            content: "# Coverage\n\n- Prefer reducer outputs that stay bounded.\n- Avoid redundant tool chatter.\n\n## Auth\nTouch src/auth.rs carefully and preserve task state.\n".to_string(),
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
            source_content: "# Coverage\n\n- Prefer reducer outputs that stay bounded.\n- Avoid redundant tool chatter.\n\n## Auth\nTouch src/auth.rs carefully and preserve task state.\n".to_string(),
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
