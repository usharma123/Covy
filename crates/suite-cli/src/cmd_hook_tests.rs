use super::*;

#[test]
fn pretool_rewrites_strict_git_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"git status --short src/lib.rs"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        Some("session-1"),
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();
    assert!(command.contains("hook reducer-runner"));
    assert!(command.contains("--family git"));
    assert!(command.contains("--kind git_status"));
}

#[test]
fn pretool_keeps_grep_on_native_compact_path_with_basic_alternation() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{
            "command": r"grep 'fn classify\|Mutation' crates/packet28-reducer-core/src/command.rs"
        }
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-grep",
        Some("session-1"),
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();

    assert!(command.contains(" compact grep "));
    assert!(!command.contains("hook reducer-runner"));
    assert!(command.contains("--basic-regexp"));
    assert!(command.contains("fn classify\\|Mutation"));
    assert!(command.contains("crates/packet28-reducer-core/src/command.rs"));
}

#[test]
fn pretool_hook_output_surfaces_action_critic_without_rewrite() {
    let body = render_hook_output(
        HookEventKind::PreToolUse,
        None,
        &packet28_daemon_core::HookIngestResponse::default(),
        None,
        &["destructive_command: inspect scope first".to_string()],
    )
    .unwrap()
    .unwrap();
    let payload: Value = serde_json::from_str(&body).unwrap();
    let output = &payload["hookSpecificOutput"];
    assert_eq!(output["hookEventName"], "PreToolUse");
    assert_eq!(output["permissionDecision"], "allow");
    assert!(output["permissionDecisionReason"]
        .as_str()
        .unwrap()
        .contains("Packet28 action critic"));
}

#[test]
fn grep_hook_packet_preserves_actionable_regions_and_preview() {
    let input = json!({
        "pattern": r"fn classify\|Mutation",
        "include": ["crates/packet28-reducer-core/src/command.rs"]
    });
    let response = json!({
        "output": "crates/packet28-reducer-core/src/command.rs:16:pub fn classify_command(command: &str) {}\ncrates/packet28-reducer-core/src/command.rs:34:pub fn classify_command_argv(command: &str) {}\n"
    });

    let packet = build_grep_packet(&input, &response).unwrap();

    assert_eq!(
        packet.search_query.as_deref(),
        Some(r"fn classify\|Mutation")
    );
    assert!(packet
        .regions
        .contains(&"crates/packet28-reducer-core/src/command.rs:16-16".to_string()));
    assert!(packet
        .regions
        .contains(&"crates/packet28-reducer-core/src/command.rs:34-34".to_string()));
    let preview = packet.compact_preview.unwrap();
    assert!(preview.contains("Grep found 2 matches"));
    assert!(preview.contains("crates/packet28-reducer-core/src/command.rs:16:"));
}

#[test]
fn bash_grep_post_capture_preserves_actionable_regions_without_pretool_rewrite() {
    let input = json!({
        "command": r"grep -n 'fn classify\|Mutation\|fn classify_command' crates/packet28-reducer-core/src/command.rs"
    });
    let response = json!({
        "stdout": "16:pub fn classify_command(command: &str) -> Option<CommandReducerSpec> {\n34:pub fn classify_command_argv(command: &str, argv: &[String]) -> Option<CommandReducerSpec> {\n"
    });

    let packet = build_bash_packet(&input, &response).unwrap();

    assert_eq!(packet.tool_name, "Bash");
    assert_eq!(packet.packet_type, "packet28.hook.bash.grep.v1");
    assert_eq!(
        packet.search_query.as_deref(),
        Some(r"fn classify\|Mutation\|fn classify_command")
    );
    assert!(packet
        .regions
        .contains(&"crates/packet28-reducer-core/src/command.rs:16-16".to_string()));
    assert!(packet
        .regions
        .contains(&"crates/packet28-reducer-core/src/command.rs:34-34".to_string()));
    let preview = packet.compact_preview.unwrap();
    assert!(preview.contains("Grep found 2 matches"));
    assert!(preview.contains("crates/packet28-reducer-core/src/command.rs:16:"));
}

#[test]
fn pretool_hook_output_preserves_rewrite_with_action_critic() {
    let body = render_hook_output(
        HookEventKind::PreToolUse,
        Some(json!({"command": "Packet28 hook reducer-runner -- git status"})),
        &packet28_daemon_core::HookIngestResponse::default(),
        None,
        &["broad_search: add focus_paths".to_string()],
    )
    .unwrap()
    .unwrap();
    let payload: Value = serde_json::from_str(&body).unwrap();
    let output = &payload["hookSpecificOutput"];
    assert!(output.get("permissionDecision").is_none());
    assert_eq!(
        output["updatedInput"]["command"],
        "Packet28 hook reducer-runner -- git status"
    );
    assert!(output["permissionDecisionReason"]
        .as_str()
        .unwrap()
        .contains("broad_search"));
}

#[test]
fn runtime_rewrite_outputs_do_not_auto_allow_permissions() {
    let updated_input = json!({"command": "Packet28 hook reducer-runner -- git status"});
    let copilot = render_runtime_hook_output(
        ExternalHookRuntime::Copilot,
        HookEventKind::PreToolUse,
        &json!({"tool_name": "runTerminalCommand"}),
        Some(updated_input.clone()),
    )
    .unwrap()
    .unwrap();
    let copilot: Value = serde_json::from_str(&copilot).unwrap();
    assert!(copilot["hookSpecificOutput"]
        .get("permissionDecision")
        .is_none());

    let cursor = render_runtime_hook_output(
        ExternalHookRuntime::Cursor,
        HookEventKind::PreToolUse,
        &json!({}),
        Some(updated_input.clone()),
    )
    .unwrap()
    .unwrap();
    let cursor: Value = serde_json::from_str(&cursor).unwrap();
    assert!(cursor.get("permission").is_none());

    let gemini = render_runtime_hook_output(
        ExternalHookRuntime::Gemini,
        HookEventKind::PreToolUse,
        &json!({}),
        Some(updated_input),
    )
    .unwrap()
    .unwrap();
    let gemini: Value = serde_json::from_str(&gemini).unwrap();
    assert!(gemini.get("decision").is_none());
}

#[test]
fn runtime_pretool_rewrites_use_shared_route_planner() {
    let root = PathBuf::from("/tmp/demo");
    let config = HookRuntimeConfig::default();

    let claude = build_pretool_rewrite(
        &config,
        &root,
        &json!({
            "tool_name": "Bash",
            "tool_input": {"command": "sudo git status --short"}
        }),
        HookEventKind::PreToolUse,
        "task-rtk",
        Some("session-rtk"),
    )
    .unwrap()
    .unwrap();
    let claude_command = claude["command"].as_str().unwrap();
    assert!(claude_command.contains("hook reducer-runner"));
    assert!(claude_command.contains("--kind git_status"));
    assert!(claude_command.ends_with(" -- git status --short"));

    let cursor = build_runtime_pretool_rewrite(
        ExternalHookRuntime::Cursor,
        &config,
        &root,
        &json!({
            "command": "env RUST_BACKTRACE=1 cargo test",
            "cwd": "/tmp/demo"
        }),
        HookEventKind::PreToolUse,
        "task-rtk",
        Some("session-rtk"),
    )
    .unwrap()
    .unwrap();
    let cursor_command = cursor["command"].as_str().unwrap();
    assert!(cursor_command.contains("--kind rust_test"));
    assert!(cursor_command.contains("--env RUST_BACKTRACE=1"));
    assert!(cursor_command.ends_with(" -- cargo test"));

    let copilot = build_runtime_pretool_rewrite(
        ExternalHookRuntime::Copilot,
        &config,
        &root,
        &json!({
            "tool_name": "runTerminalCommand",
            "tool_input": {"command": "/usr/bin/git status --short"},
            "workspace_root": "/tmp/demo"
        }),
        HookEventKind::PreToolUse,
        "task-rtk",
        Some("session-rtk"),
    )
    .unwrap()
    .unwrap();
    let copilot_command = copilot["command"].as_str().unwrap();
    assert!(copilot_command.contains("--kind git_status"));
    assert!(copilot_command.ends_with(" -- git status --short"));

    let gemini = build_runtime_pretool_rewrite(
        ExternalHookRuntime::Gemini,
        &config,
        &root,
        &json!({
            "tool_name": "run_shell_command",
            "tool_input": {"command": "sudo git status --short"},
            "cwd": "/tmp/demo"
        }),
        HookEventKind::PreToolUse,
        "task-rtk",
        Some("session-rtk"),
    )
    .unwrap()
    .unwrap();
    let gemini_command = gemini["command"].as_str().unwrap();
    assert!(gemini_command.contains("--kind git_status"));
    assert!(gemini_command.ends_with(" -- git status --short"));
}

#[test]
fn rust_workspace_fingerprint_changes_for_out_of_band_source_edit() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"demo\"\n").unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn value() -> i32 { 1 }\n",
    )
    .unwrap();
    let spec = classify_command("cargo test --lib").unwrap();

    let before = workspace_cache_fingerprint(dir.path(), dir.path(), &spec);
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn value() -> i32 { 2 }\n",
    )
    .unwrap();
    let after = workspace_cache_fingerprint(dir.path(), dir.path(), &spec);

    assert_ne!(before, after);
}

#[cfg(unix)]
#[test]
fn rust_workspace_fingerprint_skips_symlink_cycles() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"demo\"\n").unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn value() -> i32 { 1 }\n",
    )
    .unwrap();
    symlink(dir.path(), dir.path().join("src/loop")).unwrap();
    let spec = classify_command("cargo test --lib").unwrap();

    let fingerprint = workspace_cache_fingerprint(dir.path(), dir.path(), &spec);
    assert!(!fingerprint.is_empty());
}

#[test]
fn pretool_declines_composed_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"cargo test 2>&1 | grep FAILED"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        None,
    )
    .unwrap();
    assert!(rewrite.is_none());
}

#[test]
fn pretool_rewrites_supported_compound_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"cargo test | grep FAIL && git status --short"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        None,
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();
    assert!(command.contains("| grep FAIL &&"));
    assert_eq!(command.matches("hook reducer-runner").count(), 1);
}

#[test]
fn pretool_rewrites_strict_fs_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"head -n 5 README.md"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        Some("session-1"),
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();
    assert!(command.contains("hook reducer-runner"));
    assert!(command.contains("--family fs"));
    assert!(command.contains("--kind fs_head"));
}

#[test]
fn pretool_rewrites_strict_rust_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"cargo test -p packet28-reducer-core"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        Some("session-1"),
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();
    assert!(command.contains("hook reducer-runner"));
    assert!(command.contains("--family rust"));
    assert!(command.contains("--kind rust_test"));
}

#[test]
fn pretool_declines_ambiguous_fs_sed_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"sed -i 1,4p README.md"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        None,
    )
    .unwrap();
    assert!(rewrite.is_none());
}

#[test]
fn pretool_rewrites_strict_github_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"gh pr list --limit 5"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        Some("session-1"),
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();
    assert!(command.contains("hook reducer-runner"));
    assert!(command.contains("--family github"));
    assert!(command.contains("--kind gh_pr_list"));
}

#[test]
fn pretool_declines_ambiguous_github_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"gh pr list --json title"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        None,
    )
    .unwrap();
    assert!(rewrite.is_none());
}

#[test]
fn pretool_rewrites_strict_python_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"python3 -m pytest tests"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        Some("session-1"),
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();
    assert!(command.contains("hook reducer-runner"));
    assert!(command.contains("--family python"));
    assert!(command.contains("--kind python_pytest"));
}

#[test]
fn pretool_declines_ambiguous_python_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"ruff check --output-format json src"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        None,
    )
    .unwrap();
    assert!(rewrite.is_none());
}

#[test]
fn pretool_rewrites_strict_javascript_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"npx tsc --noEmit"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        Some("session-1"),
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();
    assert!(command.contains("hook reducer-runner"));
    assert!(command.contains("--family javascript"));
    assert!(command.contains("--kind javascript_tsc"));
}

#[test]
fn pretool_declines_ambiguous_javascript_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"eslint --format json src"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        None,
    )
    .unwrap();
    assert!(rewrite.is_none());
}

#[test]
fn pretool_rewrites_strict_go_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"go test ./..."}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        Some("session-1"),
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();
    assert!(command.contains("hook reducer-runner"));
    assert!(command.contains("--family go"));
    assert!(command.contains("--kind go_test"));
}

#[test]
fn pretool_declines_ambiguous_go_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"go test -json ./..."}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        None,
    )
    .unwrap();
    assert!(rewrite.is_none());
}

#[test]
fn pretool_rewrites_strict_infra_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"kubectl get pods"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        Some("session-1"),
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();
    assert!(command.contains("hook reducer-runner"));
    assert!(command.contains("--family infra"));
    assert!(command.contains("--kind kubectl_get"));
}

#[test]
fn pretool_rewrites_strict_ruby_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"bundle exec rspec spec/models/user_spec.rb"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        Some("session-1"),
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();
    assert!(command.contains("hook reducer-runner"));
    assert!(command.contains("--family ruby"));
    assert!(command.contains("--kind ruby_rspec"));
}

#[test]
fn pretool_rewrites_strict_dotnet_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"dotnet test Packet28.Tests.csproj"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        Some("session-1"),
    )
    .unwrap()
    .unwrap();
    let command = rewrite["command"].as_str().unwrap();
    assert!(command.contains("hook reducer-runner"));
    assert!(command.contains("--family dotnet"));
    assert!(command.contains("--kind dotnet_test"));
}

#[test]
fn pretool_declines_ambiguous_infra_command() {
    let root = PathBuf::from("/tmp/demo");
    let payload = json!({
        "tool_name":"Bash",
        "tool_input":{"command":"curl -o out.txt https://example.com"}
    });
    let rewrite = build_pretool_rewrite(
        &HookRuntimeConfig::default(),
        &root,
        &payload,
        HookEventKind::PreToolUse,
        "task-123",
        None,
    )
    .unwrap();
    assert!(rewrite.is_none());
}

#[test]
fn post_tool_skips_reducer_runner_command() {
    let packet = build_reducer_packet(
        &HookRuntimeConfig::default(),
        &json!({
            "tool_name":"Bash",
            "tool_input":{"command":"Packet28 hook reducer-runner --root . -- task"},
            "tool_response":{"stdout":"done"}
        }),
        HookEventKind::PostToolUse,
    );
    assert!(packet.is_none());
}

#[test]
fn post_tool_failure_captures_failed_bash_packet() {
    let packet = build_reducer_packet(
        &HookRuntimeConfig::default(),
        &json!({
            "tool_name":"Bash",
            "tool_input":{"command":"git status --short src/lib.rs"},
            "error":"fatal: not a git repository"
        }),
        HookEventKind::PostToolUseFailure,
    )
    .unwrap();
    assert!(packet.failed);
    assert_eq!(packet.reducer_family.as_deref(), Some("git"));
    assert_eq!(packet.canonical_command_kind.as_deref(), Some("git_status"));
    assert!(packet.summary.contains("fatal: not a git repository"));
}

#[test]
fn read_reducer_marks_read_operation() {
    let packet = build_read_packet(
        &json!({"file_path":"src/lib.rs","offset":10,"limit":5}),
        &json!({"content":"demo"}),
    )
    .unwrap();
    assert_eq!(
        packet.operation_kind,
        suite_packet_core::ToolOperationKind::Read
    );
    assert_eq!(packet.paths, vec!["src/lib.rs".to_string()]);
    assert_eq!(
        packet.cache_fingerprint.as_deref(),
        Some("read:src/lib.rs:10:14")
    );
}
