use super::*;
use crate::instruction_files::resolve_context;
use packet28_daemon_core::{
    ContextBackendKind, ContextResolveOutcome, ContextResolveRequest, ContextSourceKind,
    InstructionFileResolveOutcome, InstructionFileResolveRequest,
};
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn daemon_test_state() -> Arc<Mutex<DaemonState>> {
    let root = std::env::temp_dir().join(format!(
        "packet28-broker-test-{}-{}-{}",
        now_unix_millis(),
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    ensure_daemon_dir(&root).unwrap();
    let kernel = Arc::new(Kernel::with_v1_reducers_and_persistence(
        PersistConfig::new(root.clone()),
    ));
    let (index_tx, index_rx) = mpsc::channel();
    thread::spawn(move || while index_rx.recv().is_ok() {});
    Arc::new(Mutex::new(DaemonState {
        root,
        kernel,
        runtime: DaemonRuntimeInfo::default(),
        tasks: TaskRegistry::default(),
        agent_snapshots: BTreeMap::new(),
        watches: WatchRegistry::default(),
        watcher_handles: HashMap::new(),
        subscribers: HashMap::new(),
        source_file_cache: BTreeMap::new(),
        interactive_index: InteractiveIndexRuntime::default(),
        index_tx,
        shutting_down: false,
    }))
}

fn daemon_test_root(state: &Arc<Mutex<DaemonState>>) -> PathBuf {
    state.lock().unwrap().root.clone()
}

fn broker_evidence_confidence_body(
    state: &Arc<Mutex<DaemonState>>,
    snapshot: suite_packet_core::AgentSnapshotPayload,
) -> String {
    let root = daemon_test_root(state);
    build_broker_sections(
        &root,
        state,
        &BrokerGetContextRequest {
            task_id: "task-confidence".to_string(),
            action: Some(BrokerAction::Inspect),
            include_sections: vec!["evidence_confidence".to_string()],
            ..BrokerGetContextRequest::default()
        },
        &snapshot,
        None,
        None,
    )
    .into_iter()
    .find(|section| section.id == "evidence_confidence")
    .expect("evidence confidence should render")
    .body
}

fn broker_confidence_reason_line(body: &str) -> &str {
    body.lines()
        .find(|line| line.starts_with("- confidence_reason:"))
        .expect("confidence reason line should render")
}

fn broker_context_debt_body(
    state: &Arc<Mutex<DaemonState>>,
    snapshot: suite_packet_core::AgentSnapshotPayload,
) -> Option<String> {
    let root = daemon_test_root(state);
    build_broker_sections(
        &root,
        state,
        &BrokerGetContextRequest {
            task_id: "task-context-debt".to_string(),
            action: Some(BrokerAction::Edit),
            include_sections: vec!["context_debt".to_string()],
            ..BrokerGetContextRequest::default()
        },
        &snapshot,
        None,
        None,
    )
    .into_iter()
    .find(|section| section.id == "context_debt")
    .map(|section| section.body)
}

fn write_test_coverage_state(root: &Path, path: &str, covered: bool) {
    let mut coverage = suite_packet_core::CoverageData::new();
    let mut file = suite_packet_core::FileCoverage::new();
    file.lines_instrumented.insert(1);
    if covered {
        file.lines_covered.insert(1);
    }
    coverage.files.insert(path.to_string(), file);
    let bytes = suite_foundation_core::cache::serialize_coverage(&coverage).unwrap();
    let state_dir = root.join(".covy").join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("latest.bin"), bytes).unwrap();
}

fn write_testmap_state(root: &Path, path: &str, tests: &[&str]) {
    write_testmap_state_with_generated_at(root, path, tests, current_test_unix_seconds());
}

fn write_testmap_state_with_generated_at(
    root: &Path,
    path: &str,
    tests: &[&str],
    generated_at: u64,
) {
    let mut index = suite_packet_core::TestMapIndex::default();
    index.metadata.generated_at = generated_at;
    index.file_to_tests.insert(
        path.to_string(),
        tests.iter().map(|test| (*test).to_string()).collect(),
    );
    let state_dir = root.join(".covy").join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    testy_core::pipeline_testmap::write_testmap(&state_dir.join("testmap.bin"), &index).unwrap();
}

fn current_test_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[test]
fn explicit_limits_override_verbosity_alias() {
    let mut section_limits = BTreeMap::new();
    section_limits.insert("relevant_context".to_string(), 2);
    let limits = resolve_effective_limits(
        BrokerAction::Plan,
        Some(BrokerVerbosity::Rich),
        Some(3),
        Some(5),
        &section_limits,
    );
    assert_eq!(limits.max_sections, 3);
    assert_eq!(limits.default_max_items_per_section, 5);
    assert_eq!(limits.section_item_limits["relevant_context"], 2);
}

#[test]
fn omitted_explicit_limits_use_deterministic_action_defaults() {
    let plan_limits =
        resolve_effective_limits(BrokerAction::Plan, None, None, None, &BTreeMap::new());
    let choose_tool_limits =
        resolve_effective_limits(BrokerAction::ChooseTool, None, None, None, &BTreeMap::new());
    assert_eq!(plan_limits.max_sections, 8);
    assert_eq!(plan_limits.default_max_items_per_section, 8);
    assert_eq!(plan_limits.section_item_limits["code_evidence"], 6);
    assert_eq!(choose_tool_limits.max_sections, 6);
    assert_eq!(choose_tool_limits.default_max_items_per_section, 5);
}

#[test]
fn brief_always_starts_with_supersession_header() {
    let brief = render_brief(
        "task-123",
        "7",
        &[BrokerSection {
            id: "task_objective".to_string(),
            title: "Task Objective".to_string(),
            body: "Investigate auth flow".to_string(),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        }],
    );
    assert!(brief.starts_with("[Packet28 Context v7"));
    assert!(brief.contains("supersedes all prior Packet28 context"));
}

#[test]
fn normalize_plan_steps_trims_and_assigns_missing_ids() {
    let normalized = normalize_plan_steps(&[BrokerPlanStep {
        id: " ".to_string(),
        action: " Edit ".to_string(),
        description: Some(" touch auth ".to_string()),
        paths: vec!["src/auth.rs".to_string(), "src/auth.rs".to_string()],
        symbols: vec![" Login ".to_string()],
        depends_on: vec![" prev ".to_string(), "prev".to_string()],
    }]);
    assert_eq!(normalized[0].id, "step-1");
    assert_eq!(normalized[0].action, "edit");
    assert_eq!(normalized[0].description.as_deref(), Some("touch auth"));
    assert_eq!(normalized[0].paths, vec!["src/auth.rs".to_string()]);
    assert_eq!(normalized[0].symbols, vec!["Login".to_string()]);
    assert_eq!(normalized[0].depends_on, vec!["prev".to_string()]);
}

#[test]
fn validate_plan_requires_testmap_mapped_gate_for_uncovered_edits() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(root.join("src/alpha.rs"), "pub fn alpha() -> i32 { 1 }\n").unwrap();
    std::fs::write(
        root.join("tests/alpha_test.rs"),
        "#[test]\nfn alpha_test() {}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/beta_test.rs"),
        "#[test]\nfn beta_test() {}\n",
    )
    .unwrap();
    write_test_coverage_state(&root, "src/alpha.rs", false);
    write_testmap_state(&root, "src/alpha.rs", &["tests/alpha_test.rs"]);

    let response = broker_validate_plan(
        state,
        BrokerValidatePlanRequest {
            task_id: "task-testmap-gate".to_string(),
            require_read_before_edit: Some(false),
            steps: vec![
                BrokerPlanStep {
                    id: "edit-alpha".to_string(),
                    action: "edit".to_string(),
                    paths: vec!["src/alpha.rs".to_string()],
                    ..BrokerPlanStep::default()
                },
                BrokerPlanStep {
                    id: "test-beta".to_string(),
                    action: "test".to_string(),
                    paths: vec!["tests/beta_test.rs".to_string()],
                    ..BrokerPlanStep::default()
                },
            ],
            ..BrokerValidatePlanRequest::default()
        },
    )
    .unwrap();

    assert!(!response.valid);
    let violation = response
        .violations
        .iter()
        .find(|violation| violation.rule == "missing_test_gate")
        .expect("mapped test gate violation should be reported");
    assert!(violation.message.contains("tests/alpha_test.rs"));
    assert!(violation
        .related_paths
        .contains(&"tests/alpha_test.rs".to_string()));
    assert_eq!(response.test_gate_score, Some(40));
}

#[test]
fn validate_plan_accepts_testmap_mapped_or_generic_test_gate() {
    for (name, paths, expect_broad_warning, expected_score) in [
        (
            "mapped",
            vec!["tests/alpha_test.rs".to_string()],
            false,
            100,
        ),
        ("generic", Vec::new(), true, 80),
    ] {
        let state = daemon_test_state();
        let root = daemon_test_root(&state);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(root.join("src/alpha.rs"), "pub fn alpha() -> i32 { 1 }\n").unwrap();
        std::fs::write(
            root.join("tests/alpha_test.rs"),
            "#[test]\nfn alpha_test() {}\n",
        )
        .unwrap();
        write_test_coverage_state(&root, "src/alpha.rs", false);
        write_testmap_state(&root, "src/alpha.rs", &["tests/alpha_test.rs"]);

        let response = broker_validate_plan(
            state,
            BrokerValidatePlanRequest {
                task_id: format!("task-testmap-gate-{name}"),
                require_read_before_edit: Some(false),
                steps: vec![
                    BrokerPlanStep {
                        id: "edit-alpha".to_string(),
                        action: "edit".to_string(),
                        paths: vec!["src/alpha.rs".to_string()],
                        ..BrokerPlanStep::default()
                    },
                    BrokerPlanStep {
                        id: "test-alpha".to_string(),
                        action: "test".to_string(),
                        paths,
                        ..BrokerPlanStep::default()
                    },
                ],
                ..BrokerValidatePlanRequest::default()
            },
        )
        .unwrap();

        assert!(
            response
                .violations
                .iter()
                .all(|violation| violation.rule != "missing_test_gate"),
            "{name} test gate should satisfy mapped coverage requirement: {:?}",
            response.violations
        );
        assert_eq!(
            response
                .warnings
                .iter()
                .any(|warning| warning.rule == "broad_test_gate"),
            expect_broad_warning,
            "{name} test gate broad warning state should match"
        );
        assert_eq!(response.test_gate_score, Some(expected_score));
    }
}

#[test]
fn validate_plan_warns_when_testmap_has_no_mapping_for_uncovered_edit() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(root.join("src/alpha.rs"), "pub fn alpha() -> i32 { 1 }\n").unwrap();
    std::fs::write(root.join("src/beta.rs"), "pub fn beta() -> i32 { 2 }\n").unwrap();
    std::fs::write(
        root.join("tests/alpha_test.rs"),
        "#[test]\nfn alpha_test() {}\n",
    )
    .unwrap();
    write_test_coverage_state(&root, "src/alpha.rs", false);
    write_testmap_state(&root, "src/beta.rs", &["tests/beta_test.rs"]);

    let response = broker_validate_plan(
        state,
        BrokerValidatePlanRequest {
            task_id: "task-testmap-missing-mapping".to_string(),
            require_read_before_edit: Some(false),
            steps: vec![
                BrokerPlanStep {
                    id: "edit-alpha".to_string(),
                    action: "edit".to_string(),
                    paths: vec!["src/alpha.rs".to_string()],
                    ..BrokerPlanStep::default()
                },
                BrokerPlanStep {
                    id: "test-suite".to_string(),
                    action: "test".to_string(),
                    paths: Vec::new(),
                    ..BrokerPlanStep::default()
                },
            ],
            ..BrokerValidatePlanRequest::default()
        },
    )
    .unwrap();

    assert!(
        response
            .violations
            .iter()
            .all(|violation| violation.rule != "missing_test_gate"),
        "generic test gate should remain accepted when no mapping exists: {:?}",
        response.violations
    );
    let warning = response
        .warnings
        .iter()
        .find(|warning| warning.rule == "missing_testmap_mapping")
        .expect("missing testmap mapping warning should be reported");
    assert!(warning.message.contains("src/alpha.rs"));
    assert_eq!(warning.related_paths, vec!["src/alpha.rs".to_string()]);
    assert_eq!(response.test_gate_score, Some(85));
}

#[test]
fn validate_plan_warns_when_cached_testmap_is_stale() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(root.join("src/alpha.rs"), "pub fn alpha() -> i32 { 1 }\n").unwrap();
    std::fs::write(
        root.join("tests/alpha_test.rs"),
        "#[test]\nfn alpha_test() {}\n",
    )
    .unwrap();
    write_test_coverage_state(&root, "src/alpha.rs", false);
    write_testmap_state_with_generated_at(&root, "src/alpha.rs", &["tests/alpha_test.rs"], 1);

    let response = broker_validate_plan(
        state,
        BrokerValidatePlanRequest {
            task_id: "task-stale-testmap".to_string(),
            require_read_before_edit: Some(false),
            steps: vec![
                BrokerPlanStep {
                    id: "edit-alpha".to_string(),
                    action: "edit".to_string(),
                    paths: vec!["src/alpha.rs".to_string()],
                    ..BrokerPlanStep::default()
                },
                BrokerPlanStep {
                    id: "test-alpha".to_string(),
                    action: "test".to_string(),
                    paths: vec!["tests/alpha_test.rs".to_string()],
                    ..BrokerPlanStep::default()
                },
            ],
            ..BrokerValidatePlanRequest::default()
        },
    )
    .unwrap();

    assert!(
        response.valid,
        "stale testmap is advisory: {:?}",
        response.violations
    );
    let warning = response
        .warnings
        .iter()
        .find(|warning| warning.rule == "stale_testmap")
        .expect("stale testmap warning should be reported");
    assert_eq!(warning.step_id, "testmap");
    assert!(warning.message.contains("lower confidence"));
    assert_eq!(response.test_gate_score, Some(90));
}

#[test]
fn validate_plan_warns_when_edit_relies_on_stale_evidence_after_checkpoint() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/alpha.rs"), "pub fn alpha() -> i32 { 1 }\n").unwrap();
    state.lock().unwrap().agent_snapshots.insert(
        "task-stale-evidence".to_string(),
        suite_packet_core::AgentSnapshotPayload {
            task_id: "task-stale-evidence".to_string(),
            changed_paths_since_checkpoint: vec!["src/alpha.rs".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    let response = broker_validate_plan(
        state,
        BrokerValidatePlanRequest {
            task_id: "task-stale-evidence".to_string(),
            require_read_before_edit: Some(false),
            require_test_gate: Some(false),
            steps: vec![BrokerPlanStep {
                id: "edit-alpha".to_string(),
                action: "edit".to_string(),
                paths: vec!["src/alpha.rs".to_string()],
                ..BrokerPlanStep::default()
            }],
            ..BrokerValidatePlanRequest::default()
        },
    )
    .unwrap();

    assert!(response.valid);
    let warning = response
        .warnings
        .iter()
        .find(|warning| warning.rule == "stale_evidence_after_checkpoint")
        .expect("stale evidence warning should be reported");
    assert_eq!(warning.step_id, "edit-alpha");
    assert_eq!(warning.related_paths, vec!["src/alpha.rs".to_string()]);
    assert!(warning
        .message
        .contains("changed since the latest checkpoint"));
}

#[test]
fn infer_scope_paths_prefers_explicit_paths() {
    let inferred = infer_scope_paths(
        "refactor auth module",
        &mapy_core::RepoMapPayloadRich {
            files_ranked: vec![
                mapy_core::RankedFileRich {
                    path: "src/auth.rs".to_string(),
                    score: 1.0,
                    symbol_count: 1,
                    import_count: 0,
                },
                mapy_core::RankedFileRich {
                    path: "src/session.rs".to_string(),
                    score: 0.8,
                    symbol_count: 1,
                    import_count: 0,
                },
            ],
            ..Default::default()
        },
        &["src/session.rs".to_string()],
        &[],
    );
    assert_eq!(inferred, vec!["src/session.rs".to_string()]);
}

#[test]
fn derive_query_focus_extracts_symbol_terms() {
    let focus = derive_query_focus(Some(
        "What does StringUtils.abbreviate() do in src/main/java/StringUtils.java?",
    ));
    assert!(focus
        .full_symbol_terms
        .contains(&"StringUtils.abbreviate".to_string()));
    assert!(focus.symbol_terms.iter().any(|item| item == "StringUtils"));
    assert!(focus.symbol_terms.iter().any(|item| item == "abbreviate"));
    assert!(focus
        .path_terms
        .iter()
        .any(|item| item.contains("StringUtils.java")));
}

#[test]
fn derive_query_focus_filters_stopwords_but_keeps_symbols() {
    let focus = derive_query_focus(Some(
        "Where is StringUtils.isBlank defined and used across the codebase?",
    ));
    assert!(!focus.text_tokens.iter().any(|item| item == "where"));
    assert!(!focus.text_tokens.iter().any(|item| item == "defined"));
    assert!(!focus.text_tokens.iter().any(|item| item == "used"));
    assert!(focus
        .full_symbol_terms
        .contains(&"StringUtils.isBlank".to_string()));
    assert!(focus
        .symbol_terms
        .iter()
        .any(|item| item.eq_ignore_ascii_case("isBlank")));
}

#[test]
fn expand_scope_paths_pulls_adjacent_role_files() {
    let expanded = expand_scope_paths(
        "explain what diffy does",
        &mapy_core::RepoMapPayloadRich {
            files_ranked: vec![
                mapy_core::RankedFileRich {
                    path: "crates/diffy-core/src/lib.rs".to_string(),
                    score: 1.0,
                    symbol_count: 2,
                    import_count: 1,
                },
                mapy_core::RankedFileRich {
                    path: "crates/diffy-core/src/report.rs".to_string(),
                    score: 0.7,
                    symbol_count: 2,
                    import_count: 0,
                },
                mapy_core::RankedFileRich {
                    path: "crates/diffy-cli/src/cmd_analyze.rs".to_string(),
                    score: 0.65,
                    symbol_count: 2,
                    import_count: 1,
                },
                mapy_core::RankedFileRich {
                    path: "crates/testy-core/src/lib.rs".to_string(),
                    score: 0.6,
                    symbol_count: 2,
                    import_count: 0,
                },
            ],
            symbols_ranked: vec![
                mapy_core::RankedSymbolRich {
                    name: "analyze".to_string(),
                    file: "crates/diffy-cli/src/cmd_analyze.rs".to_string(),
                    kind: "function".to_string(),
                    score: 0.9,
                },
                mapy_core::RankedSymbolRich {
                    name: "render_report".to_string(),
                    file: "crates/diffy-core/src/report.rs".to_string(),
                    kind: "function".to_string(),
                    score: 0.8,
                },
            ],
            edges: vec![
                mapy_core::RepoEdgeRich {
                    from: "crates/diffy-cli/src/cmd_analyze.rs".to_string(),
                    to: "crates/diffy-core/src/lib.rs".to_string(),
                    kind: "import".to_string(),
                },
                mapy_core::RepoEdgeRich {
                    from: "crates/diffy-core/src/report.rs".to_string(),
                    to: "crates/diffy-core/src/lib.rs".to_string(),
                    kind: "import".to_string(),
                },
            ],
            ..Default::default()
        },
        &["crates/diffy-core/src/lib.rs".to_string()],
        &["diffy".to_string()],
        6,
    );
    assert!(expanded.contains(&"crates/diffy-core/src/report.rs".to_string()));
    assert!(expanded.contains(&"crates/diffy-cli/src/cmd_analyze.rs".to_string()));
}

fn write_search_fixture(root: &std::path::Path, files: &[(&str, &str)]) {
    for (relative_path, contents) in files {
        let path = root.join(relative_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}

fn run_search_execution_for_query(
    root: &std::path::Path,
    query: &str,
    action: BrokerAction,
) -> SearchExecution {
    let snapshot = suite_packet_core::AgentSnapshotPayload::default();
    run_search_execution_for_query_with_snapshot(root, query, action, &snapshot)
}

fn run_search_execution_for_query_with_snapshot(
    root: &std::path::Path,
    query: &str,
    action: BrokerAction,
    snapshot: &suite_packet_core::AgentSnapshotPayload,
) -> SearchExecution {
    let request = BrokerGetContextRequest {
        task_id: "task-search".to_string(),
        action: Some(action),
        query: Some(query.to_string()),
        ..BrokerGetContextRequest::default()
    };
    let query_focus = derive_query_focus(Some(query));
    build_reducer_search_execution(SearchExecutionArgs {
        state: None,
        root,
        snapshot,
        request: &request,
        query_focus: &query_focus,
        action,
        max_files: 8,
        max_evidence_lines: 8,
    })
}

#[test]
fn exact_symbol_query_returns_definition_first_without_fallback() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path();
    write_search_fixture(
        root,
        &[
            (
                "src/alpha.rs",
                "pub struct Alpha;\nimpl Alpha { pub fn build() {} }\n",
            ),
            (
                "src/mentions.rs",
                "fn helper() { let _ = Alpha::build(); }\n",
            ),
        ],
    );

    let execution =
        run_search_execution_for_query(root, "Where is Alpha defined?", BrokerAction::Inspect);
    assert!(!execution.used_fallback);
    assert_eq!(
        execution.files.first().map(|file| file.path.as_str()),
        Some("src/alpha.rs")
    );
    assert!(execution.files[0].definition_hits > 0);
}

#[test]
fn vague_query_triggers_fallback_only_after_weak_first_pass() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path();
    write_search_fixture(
        root,
        &[
            ("src/alpha.rs", "pub struct AlphaService;\n"),
            (
                "src/alpha_update.rs",
                "pub fn update_state_for_alpha_service() {}\n",
            ),
        ],
    );

    let execution = run_search_execution_for_query(
        root,
        "How is AlphaService.updateState updated?",
        BrokerAction::Inspect,
    );
    assert!(execution.used_fallback);
    assert!(execution
        .files
        .iter()
        .any(|file| file.path == "src/alpha_update.rs"));
    assert!(execution
        .evidence_by_file
        .get("src/alpha_update.rs")
        .is_some_and(|summary| summary
            .rendered_lines
            .iter()
            .any(|line| line.contains("update_state_for_alpha_service"))));
}

#[test]
fn definition_hits_outrank_bulk_references() {
    let root = std::env::temp_dir().join(format!(
        "packet28d-search-definition-rank-{}",
        std::process::id()
    ));
    write_search_fixture(
            &root,
            &[
                (
                    "src/alpha.rs",
                    "pub struct Alpha;\n",
                ),
                (
                    "src/references.rs",
                    "fn one() { let _ = Alpha; }\nfn two() { let _ = Alpha; }\nfn three() { let _ = Alpha; }\nfn four() { let _ = Alpha; }\n",
                ),
            ],
        );

    let execution = run_search_execution_for_query(&root, "Alpha", BrokerAction::Inspect);
    assert_eq!(
        execution.files.first().map(|file| file.path.as_str()),
        Some("src/alpha.rs")
    );
    assert!(execution.files[0].definition_hits >= execution.files[1].definition_hits);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn broad_generic_tokens_do_not_outrank_exact_symbol_hits() {
    let root = std::env::temp_dir().join(format!(
        "packet28d-search-generic-rank-{}",
        std::process::id()
    ));
    write_search_fixture(
            &root,
            &[
                (
                    "src/request.rs",
                    "pub struct BrokerWriteStateRequest {\n    pub task_id: String,\n}\n",
                ),
                (
                    "src/noise.rs",
                    "pub fn a(task_id: &str) {}\npub fn b(task_id: &str) {}\npub fn c(task_id: &str) {}\npub fn d(task_id: &str) {}\n",
                ),
            ],
        );

    let execution = run_search_execution_for_query(
        &root,
        "How does BrokerWriteStateRequest use task_id?",
        BrokerAction::Inspect,
    );
    assert_eq!(
        execution.files.first().map(|file| file.path.as_str()),
        Some("src/request.rs")
    );
    assert!(execution.files[0].exact_symbol_hits > 0);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn search_ranking_prefers_fresh_changed_path_over_stale_changed_path() {
    let root = std::env::temp_dir().join(format!(
        "packet28d-search-freshness-rank-{}",
        std::process::id()
    ));
    write_search_fixture(
        &root,
        &[
            ("src/a_stale.rs", "fn shared_term() {}\n"),
            ("src/z_fresh.rs", "fn shared_term() {}\n"),
        ],
    );
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        files_read: vec!["src/z_fresh.rs".to_string()],
        changed_paths_since_checkpoint: vec![
            "src/a_stale.rs".to_string(),
            "src/z_fresh.rs".to_string(),
        ],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };

    let execution = run_search_execution_for_query_with_snapshot(
        &root,
        "shared_term",
        BrokerAction::Inspect,
        &snapshot,
    );
    assert_eq!(
        execution.files.first().map(|file| file.path.as_str()),
        Some("src/z_fresh.rs")
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn choose_tool_uses_the_same_staged_search_planner() {
    let root = std::env::temp_dir().join(format!(
        "packet28d-search-choose-tool-{}",
        std::process::id()
    ));
    write_search_fixture(
        &root,
        &[
            ("src/alpha.rs", "pub struct AlphaService;\n"),
            (
                "src/alpha_update.rs",
                "pub fn update_state_for_alpha_service() {}\n",
            ),
        ],
    );

    let execution = run_search_execution_for_query(
        &root,
        "How is AlphaService.updateState updated?",
        BrokerAction::ChooseTool,
    );
    assert!(execution.used_fallback);
    assert!(execution
        .files
        .iter()
        .any(|file| file.path == "src/alpha_update.rs"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn choose_tool_action_critic_flags_missing_intent_and_risky_commands() {
    let missing = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(missing
        .iter()
        .any(|line| line.contains("missing_tool_intent")));

    let risky = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            query: Some("run rm -rf target/tmp after checking".to_string()),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(risky
        .iter()
        .any(|line| line.contains("destructive_command")));

    let scoped_search = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            query: Some("rg AlphaService".to_string()),
            focus_paths: vec!["src/alpha.rs".to_string()],
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(!scoped_search
        .iter()
        .any(|line| line.contains("broad_search")));

    let broad_search = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            query: Some("rg AlphaService".to_string()),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(broad_search
        .iter()
        .any(|line| line.contains("broad_search")));
}

#[test]
fn choose_tool_action_critic_flags_finalization_without_recent_verification() {
    let missing_verification = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            query: Some("commit and push this change".to_string()),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(missing_verification
        .iter()
        .any(|line| line.contains("verification_gap")));

    let verified = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::ChooseTool),
            query: Some("commit and push this change".to_string()),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload {
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-1".to_string(),
                sequence: 1,
                tool_name: "cargo test".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        },
        &[],
    );
    assert!(!verified
        .iter()
        .any(|line| line.contains("verification_gap")));
}

#[test]
fn edit_action_critic_flags_missing_scope_and_unread_paths() {
    let missing_scope = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::Edit),
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        &[],
    );
    assert!(missing_scope
        .iter()
        .any(|line| line.contains("missing_edit_scope")));

    let unread = build_action_critic_lines(
        &BrokerGetContextRequest {
            action: Some(BrokerAction::Edit),
            focus_paths: vec![
                "src/read.rs".to_string(),
                "src/unread.rs".to_string(),
                "./src/tool-read.rs".to_string(),
            ],
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload {
            files_read: vec!["src/read.rs".to_string()],
            read_paths_by_tool: vec![suite_packet_core::ToolPathSummary {
                tool_name: "rg".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Read,
                paths: vec!["src/tool-read.rs".to_string()],
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
        &[],
    );
    assert!(unread
        .iter()
        .any(|line| line.contains("read_before_edit") && line.contains("src/unread.rs")));
    assert!(!unread
        .iter()
        .any(|line| line.contains("src/read.rs") || line.contains("src/tool-read.rs")));
}

#[test]
fn extract_code_evidence_prefers_query_hits_and_context() {
    let root = std::env::temp_dir().join(format!("packet28d-code-evidence-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let path = root.join("src/lib.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            "use std::fmt;\n\npub struct Diffy;\nimpl Diffy {\n    pub fn analyze() {}\n    pub fn summarize() {}\n}\n",
        )
        .unwrap();

    let evidence = extract_code_evidence(
        &root,
        "src/lib.rs",
        &derive_query_focus(Some("Diffy.analyze")),
        &[],
        3,
        6,
    );
    assert!(evidence
        .primary_match_symbol
        .as_deref()
        .is_some_and(|value| value == "analyze" || value == "Diffy"));
    assert!(evidence
        .rendered_lines
        .iter()
        .any(|line| line.contains("pub fn analyze")));
    assert!(evidence
        .rendered_lines
        .iter()
        .all(|line| !line.contains("use std::fmt")));
    assert!(evidence
        .rendered_lines
        .iter()
        .any(|line| line.contains("impl Diffy") || line.contains("pub struct Diffy")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extract_code_evidence_ignores_license_headers_and_prefers_focus_symbols() {
    let root = std::env::temp_dir().join(format!(
        "packet28d-code-evidence-java-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let path = root.join("src/StringUtils.java");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            "/*\n * Licensed to the Apache Software Foundation (ASF)\n */\npackage org.example;\n\npublic class StringUtils {\n    public static boolean isBlank(final CharSequence cs) {\n        return cs == null || cs.length() == 0;\n    }\n}\n",
        )
        .unwrap();

    let mut focus = derive_query_focus(Some(
        "Where is StringUtils.isBlank defined and used across the codebase?",
    ));
    focus.full_symbol_terms.clear();
    focus.symbol_terms.clear();
    let focus = merge_query_focus_with_symbols(focus, &["isBlank".to_string()]);
    let evidence = extract_code_evidence(&root, "src/StringUtils.java", &focus, &[], 3, 6);
    assert!(evidence
        .rendered_lines
        .iter()
        .any(|line| line.contains("isBlank(final CharSequence cs)")));
    assert!(evidence
        .rendered_lines
        .iter()
        .all(|line| !line.contains("Licensed to the Apache")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extract_code_evidence_prefers_symbol_definitions_over_comment_mentions() {
    let root = std::env::temp_dir().join(format!(
        "packet28d-code-evidence-priority-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let path = root.join("src/StringUtils.java");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            "package org.example;\n\npublic final class StringUtils {\n    /** Mention isBlank in docs before the definition. */\n    public static boolean isBlank(final CharSequence cs) {\n        return cs == null || cs.length() == 0;\n    }\n}\n",
        )
        .unwrap();

    let mut focus = derive_query_focus(Some(
        "Where is StringUtils.isBlank defined and used across the codebase?",
    ));
    focus.full_symbol_terms.clear();
    focus.symbol_terms.clear();
    let focus = merge_query_focus_with_symbols(focus, &["isBlank".to_string()]);
    let evidence = extract_code_evidence(&root, "src/StringUtils.java", &focus, &[], 1, 3);
    assert!(evidence
        .rendered_lines
        .iter()
        .any(|line| line.contains("isBlank(final CharSequence cs)")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extract_code_evidence_prefers_region_hints_when_present() {
    let root = std::env::temp_dir().join(format!(
        "packet28d-code-evidence-region-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let path = root.join("src/StringUtils.java");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            "package org.example;\n\npublic final class StringUtils {\n    /** Mention isBlank in docs before the definition. */\n    public static String describe() { return \"isBlank docs\"; }\n\n    public static boolean isBlank(final CharSequence cs) {\n        return cs == null || cs.length() == 0;\n    }\n}\n",
        )
        .unwrap();

    let mut focus = derive_query_focus(Some(
        "Where is StringUtils.isBlank defined and used across the codebase?",
    ));
    focus.full_symbol_terms.clear();
    focus.symbol_terms.clear();
    let focus = merge_query_focus_with_symbols(focus, &["isBlank".to_string()]);
    let provenance = vec![ToolResultProvenance {
        regions: vec!["src/StringUtils.java:7-8".to_string()],
    }];
    let evidence = extract_code_evidence(&root, "src/StringUtils.java", &focus, &provenance, 1, 3);
    assert!(evidence.from_region_hint);
    assert_eq!(
        evidence.primary_match_kind,
        Some(EvidenceMatchKind::DefinesSymbol)
    );
    assert!(evidence
        .rendered_lines
        .iter()
        .any(|line| line.contains("isBlank(final CharSequence cs)")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extract_code_evidence_skips_unrelated_signatures_when_symbol_focus_exists() {
    let root = std::env::temp_dir().join(format!(
        "packet28d-code-evidence-unrelated-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let path = root.join("src/ArrayUtils.java");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            "package org.example;\n\npublic class ArrayUtils {\n    public static void shuffle() {}\n}\n",
        )
        .unwrap();

    let mut focus = derive_query_focus(Some(
        "Where is StringUtils.isBlank defined and used across the codebase?",
    ));
    focus.full_symbol_terms.clear();
    focus.symbol_terms.clear();
    let focus = merge_query_focus_with_symbols(focus, &["isBlank".to_string()]);
    let evidence = extract_code_evidence(&root, "src/ArrayUtils.java", &focus, &[], 3, 6);
    assert!(evidence.rendered_lines.is_empty());
    assert!(evidence.primary_match_symbol.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extract_code_evidence_prefers_method_match_over_class_declaration() {
    let root = std::env::temp_dir().join(format!(
        "packet28d-code-evidence-method-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let path = root.join("src/ArrayUtils.java");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            "package org.example;\n\npublic class ArrayUtils {\n    public static void shuffle() {}\n}\n",
        )
        .unwrap();

    let mut focus = derive_query_focus(Some(
        "Add deterministic seeded shuffle overloads to ArrayUtils",
    ));
    focus.full_symbol_terms.clear();
    focus.symbol_terms.clear();
    let focus =
        merge_query_focus_with_symbols(focus, &["ArrayUtils".to_string(), "shuffle".to_string()]);
    let evidence = extract_code_evidence(&root, "src/ArrayUtils.java", &focus, &[], 3, 6);
    assert!(evidence
        .rendered_lines
        .iter()
        .any(|line| line.contains("public static void shuffle")));
    assert!(evidence
        .rendered_lines
        .iter()
        .all(|line| !line.contains("public class ArrayUtils")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn build_budget_notes_section_is_empty_without_budget_pruning() {
    let limits =
        resolve_effective_limits(BrokerAction::Inspect, None, None, None, &BTreeMap::new());
    assert!(build_budget_notes_section(&[], &limits).is_none());
    assert!(build_budget_notes_section(
        &[BrokerEvictionCandidate {
            section_id: "search_evidence".to_string(),
            reason: "search evidence can be regenerated".to_string(),
            est_tokens: 12,
        }],
        &limits
    )
    .is_none());
}

#[test]
fn budget_preflight_warns_on_low_budget_broad_context_without_focus() {
    let limits =
        resolve_effective_limits(BrokerAction::Inspect, None, None, None, &BTreeMap::new());
    let allowed_sections = filter_requested_section_ids(
        BrokerAction::Inspect,
        &["budget_notes".to_string(), "search_evidence".to_string()],
        &[],
    );
    let request = BrokerGetContextRequest {
        action: Some(BrokerAction::Inspect),
        budget_tokens: Some(128),
        include_sections: vec!["budget_notes".to_string(), "search_evidence".to_string()],
        ..BrokerGetContextRequest::default()
    };
    let snapshot = suite_packet_core::AgentSnapshotPayload::default();
    let focus_symbols = Vec::<String>::new();

    let section = build_budget_preflight_section(
        &request,
        &snapshot,
        &focus_symbols,
        &allowed_sections,
        &limits,
    )
    .expect("low broad request should produce a budget preflight warning");
    assert!(section.body.contains("budget_preflight"));
    assert!(section.body.contains("add focus_paths or focus_symbols"));

    let scoped_request = BrokerGetContextRequest {
        focus_paths: vec!["src/lib.rs".to_string()],
        ..request.clone()
    };
    assert!(build_budget_preflight_section(
        &scoped_request,
        &snapshot,
        &focus_symbols,
        &allowed_sections,
        &limits,
    )
    .is_none());

    let roomy_request = BrokerGetContextRequest {
        budget_tokens: Some(broker_default_budget_tokens()),
        ..request
    };
    assert!(build_budget_preflight_section(
        &roomy_request,
        &snapshot,
        &focus_symbols,
        &allowed_sections,
        &limits,
    )
    .is_none());
}

#[test]
fn postprocess_selected_sections_adds_budget_notes_and_compacts_tool_activity() {
    let limits =
        resolve_effective_limits(BrokerAction::Inspect, None, None, None, &BTreeMap::new());
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "tool-1".to_string(),
            sequence: 7,
            tool_name: "grep".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Search,
            request_summary: Some("search for isBlank".to_string()),
            result_summary: Some("Validate.java:806 calls isBlank".to_string()),
            paths: vec!["src/Validate.java".to_string()],
            regions: vec!["src/Validate.java:806-806".to_string()],
            symbols: vec!["isBlank".to_string()],
            duration_ms: Some(12),
            ..Default::default()
        }],
        ..Default::default()
    };
    let sections = vec![
        BrokerSection {
            id: "task_objective".to_string(),
            title: "Task Objective".to_string(),
            body: "Where is StringUtils.isBlank defined and used?".to_string(),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        },
        BrokerSection {
            id: "recent_tool_activity".to_string(),
            title: "Recent Tool Activity".to_string(),
            body: "- #7 grep [search] search for isBlank -> Validate.java:806 calls isBlank"
                .to_string(),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        },
        BrokerSection {
            id: "code_evidence".to_string(),
            title: "Code Evidence".to_string(),
            body: "- src/Validate.java:806 if (StringUtils.isBlank(chars))".to_string(),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        },
    ];
    let pruned = vec![BrokerEvictionCandidate {
        section_id: "search_evidence".to_string(),
        reason: "budget_pruned".to_string(),
        est_tokens: 491,
    }];

    let processed = postprocess_selected_sections(sections, &pruned, &snapshot, &limits);
    let budget_notes = processed
        .iter()
        .find(|section| section.id == "budget_notes")
        .expect("budget notes should be inserted");
    assert!(budget_notes
        .body
        .contains("search_evidence omitted due to budget"));
    assert!(budget_notes.body.contains("491"));
    let tool_activity = processed
        .iter()
        .find(|section| section.id == "recent_tool_activity")
        .expect("tool activity should remain");
    assert!(tool_activity.body.contains("paths=1"));
    assert!(tool_activity.body.contains("regions=1"));
    assert!(tool_activity.body.contains("duration=12ms"));
    assert!(!tool_activity.body.contains("->"));
}

#[test]
fn budget_pruning_drops_optional_sections_before_critical_ones() {
    let sections = vec![
        BrokerSection {
            id: "task_objective".to_string(),
            title: "Task Objective".to_string(),
            body: "Investigate Alpha".to_string(),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        },
        BrokerSection {
            id: "code_evidence".to_string(),
            title: "Code Evidence".to_string(),
            body: [
                "- src/alpha.rs:1 fn alpha() {}",
                "- src/alpha.rs:2 struct Alpha;",
            ]
            .join("\n"),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        },
        BrokerSection {
            id: "search_evidence".to_string(),
            title: "Relevant Files".to_string(),
            body: "- src/alpha.rs:1 [matches=2] — direct reducer hit for Alpha".to_string(),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        },
        BrokerSection {
            id: "recent_tool_activity".to_string(),
            title: "Recent Tool Activity".to_string(),
            body: [
                "- #1 read [read] alpha -> found Alpha",
                "- #2 grep [search] alpha -> found alpha()",
            ]
            .join("\n"),
            priority: 2,
            source_kind: BrokerSourceKind::Derived,
        },
    ];
    let rendered = render_brief("task-a", "v1", &sections[..3]);
    let (budget_tokens, budget_bytes) = estimate_text_cost(&rendered);
    let (selected, evicted) = prune_sections_for_budget(
        BrokerAction::Inspect,
        sections,
        budget_tokens + 2,
        budget_bytes + 8,
        8,
    );
    assert!(selected.iter().any(|section| section.id == "code_evidence"));
    assert!(selected
        .iter()
        .any(|section| section.id == "search_evidence"));
    assert!(!selected
        .iter()
        .any(|section| section.id == "recent_tool_activity"));
    assert!(evicted.iter().any(|candidate| {
        candidate.section_id == "recent_tool_activity" && candidate.reason == "budget_pruned"
    }));
}

#[test]
fn relevant_context_renders_human_summaries_without_debug_ids() {
    let request = BrokerGetContextRequest {
        task_id: "task-summary".to_string(),
        include_sections: vec!["relevant_context".to_string()],
        ..BrokerGetContextRequest::default()
    };
    let snapshot = suite_packet_core::AgentSnapshotPayload::default();
    let manage = suite_packet_core::ContextManagePayload {
        working_set: vec![
            suite_packet_core::ContextManagePacketRef {
                cache_key: "cache-1".to_string(),
                target: "packet28.broker_memory.write".to_string(),
                score: 9.0,
                summary: Some(
                    "Checkpoint handoff for task-summary: inspect Alpha before editing it"
                        .to_string(),
                ),
                reason: Some("curated_memory".to_string()),
                source_tier: Some(suite_packet_core::MemorySourceTier::CuratedMemory),
                memory_kind: Some(suite_packet_core::MemoryKind::Handoff),
                packet_types: vec!["suite.packet28.broker_memory.v1".to_string()],
                est_tokens: 24,
                est_bytes: 96,
                runtime_ms: 1,
            },
            suite_packet_core::ContextManagePacketRef {
                cache_key: "cache-2".to_string(),
                target: "contextq.manage".to_string(),
                score: 7.0,
                summary: Some(
                    "task memory for task-summary: 2 relevant packet(s), 1 recommended action(s)"
                        .to_string(),
                ),
                reason: Some("curated_memory".to_string()),
                source_tier: Some(suite_packet_core::MemorySourceTier::CuratedMemory),
                memory_kind: Some(suite_packet_core::MemoryKind::Brief),
                packet_types: vec!["suite.context.manage.v1".to_string()],
                est_tokens: 18,
                est_bytes: 72,
                runtime_ms: 1,
            },
        ],
        ..suite_packet_core::ContextManagePayload::default()
    };

    let sections = build_broker_sections(
        Path::new("."),
        &daemon_test_state(),
        &request,
        &snapshot,
        Some(&manage),
        None,
    );
    let relevant_context = sections
        .iter()
        .find(|section| section.id == "relevant_context")
        .expect("relevant_context section should exist");
    assert!(relevant_context
        .body
        .contains("Checkpoint handoff for task-summary"));
    assert!(!relevant_context.body.contains("cache-1"));
    assert!(!relevant_context
        .body
        .contains("packet28.broker_memory.write"));
}

#[test]
fn relevant_context_marks_and_downranks_stale_changed_path_context() {
    let request = BrokerGetContextRequest {
        task_id: "task-stale-context".to_string(),
        include_sections: vec!["relevant_context".to_string()],
        ..BrokerGetContextRequest::default()
    };
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_paths_since_checkpoint: vec!["src/stale.rs".to_string()],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let manage = suite_packet_core::ContextManagePayload {
        working_set: vec![
            suite_packet_core::ContextManagePacketRef {
                cache_key: "cache-stale".to_string(),
                target: "contextq.manage".to_string(),
                score: 9.0,
                summary: Some("cached notes for src/stale.rs before the edit".to_string()),
                reason: Some("curated_memory".to_string()),
                source_tier: Some(suite_packet_core::MemorySourceTier::CuratedMemory),
                memory_kind: Some(suite_packet_core::MemoryKind::Evidence),
                packet_types: vec!["suite.context.manage.v1".to_string()],
                est_tokens: 18,
                est_bytes: 72,
                runtime_ms: 1,
            },
            suite_packet_core::ContextManagePacketRef {
                cache_key: "cache-fresh".to_string(),
                target: "contextq.manage".to_string(),
                score: 7.0,
                summary: Some("general implementation notes".to_string()),
                reason: Some("curated_memory".to_string()),
                source_tier: Some(suite_packet_core::MemorySourceTier::CuratedMemory),
                memory_kind: Some(suite_packet_core::MemoryKind::Brief),
                packet_types: vec!["suite.context.manage.v1".to_string()],
                est_tokens: 18,
                est_bytes: 72,
                runtime_ms: 1,
            },
        ],
        ..suite_packet_core::ContextManagePayload::default()
    };

    let sections = build_broker_sections(
        Path::new("."),
        &daemon_test_state(),
        &request,
        &snapshot,
        Some(&manage),
        None,
    );
    let relevant_context = sections
        .iter()
        .find(|section| section.id == "relevant_context")
        .expect("relevant_context section should exist");
    assert!(relevant_context
        .body
        .contains("[stale_after_change: refresh src/stale.rs]"));
    let fresh_idx = relevant_context
        .body
        .find("general implementation notes")
        .expect("fresh context should render");
    let stale_idx = relevant_context
        .body
        .find("cached notes for src/stale.rs")
        .expect("stale context should render");
    assert!(
        fresh_idx < stale_idx,
        "stale changed-path context should render after fresh context"
    );
}

#[test]
fn active_decisions_render_related_paths_and_symbols() {
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        active_decisions: vec![suite_packet_core::AgentDecision {
            id: "hypothesis:auth-cache".to_string(),
            text: "hypothesis active: Auth cache invalidation is suspect".to_string(),
            related_paths: vec!["src/auth.rs".to_string()],
            related_symbols: vec!["AuthCache".to_string()],
            related_artifact_ids: vec!["artifact-auth-cache".to_string()],
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let request = BrokerGetContextRequest {
        task_id: "task-hypothesis-evidence".to_string(),
        action: Some(BrokerAction::Inspect),
        ..BrokerGetContextRequest::default()
    };
    let sections = build_broker_sections(
        Path::new("."),
        &daemon_test_state(),
        &request,
        &snapshot,
        None,
        None,
    );
    let active_decisions = sections
        .iter()
        .find(|section| section.id == "active_decisions")
        .expect("active_decisions section should exist");
    assert!(active_decisions.body.contains("paths=src/auth.rs"));
    assert!(active_decisions.body.contains("symbols=AuthCache"));
    assert!(active_decisions
        .body
        .contains("artifacts=artifact-auth-cache"));
}

#[test]
fn broker_context_surfaces_failure_advice_from_run_savings() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    std::fs::create_dir_all(root.join(".packet28")).unwrap();
    std::fs::write(
        root.join(".packet28").join("run-savings.jsonl"),
        [
            serde_json::json!({
                "command": "cargo test failing_case",
                "cwd": root.display().to_string(),
                "family": "rust",
                "canonical_kind": "cargo_test",
                "exit_code": 101,
                "raw_est_tokens": 100,
                "reduced_est_tokens": 20,
                "savings_percent": 80.0,
                "fallback_reason": null,
                "failure_fingerprint": "failure:v1:abc",
                "changed_paths": [],
                "timestamp_unix_ms": 1
            })
            .to_string(),
            serde_json::json!({
                "command": "cargo test fixed_case",
                "cwd": root.display().to_string(),
                "family": "rust",
                "canonical_kind": "cargo_test",
                "exit_code": 0,
                "raw_est_tokens": 100,
                "reduced_est_tokens": 20,
                "savings_percent": 80.0,
                "fallback_reason": null,
                "failure_fingerprint": null,
                "changed_paths": ["src/fix.rs"],
                "timestamp_unix_ms": 2
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    let sections = build_broker_sections(
        &root,
        &state,
        &BrokerGetContextRequest {
            task_id: "task-failure-advice".to_string(),
            action: Some(BrokerAction::Plan),
            include_sections: vec!["failure_advice".to_string()],
            ..BrokerGetContextRequest::default()
        },
        &suite_packet_core::AgentSnapshotPayload::default(),
        None,
        None,
    );

    let advice = sections
        .iter()
        .find(|section| section.id == "failure_advice")
        .expect("failure advice section should render from run savings");
    assert!(advice.body.contains("failure:v1:abc"));
    assert!(advice.body.contains("cargo test fixed_case"));
    assert!(advice.body.contains("paths=src/fix.rs"));
}

#[test]
fn budget_pruning_shrinks_critical_sections_before_dropping_them() {
    let code_evidence_body = (1..=8)
        .map(|idx| format!("- src/alpha.rs:{idx} evidence line {idx}"))
        .collect::<Vec<_>>()
        .join("\n");
    let sections = vec![
        BrokerSection {
            id: "task_objective".to_string(),
            title: "Task Objective".to_string(),
            body: "Edit Alpha".to_string(),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        },
        BrokerSection {
            id: "code_evidence".to_string(),
            title: "Code Evidence".to_string(),
            body: code_evidence_body.clone(),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        },
        BrokerSection {
            id: "search_evidence".to_string(),
            title: "Relevant Files".to_string(),
            body: "- src/alpha.rs:1 [matches=2] — direct reducer hit for Alpha".to_string(),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        },
    ];
    let partial_sections = vec![
        sections[0].clone(),
        BrokerSection {
            id: "code_evidence".to_string(),
            title: "Code Evidence".to_string(),
            body: code_evidence_body
                .lines()
                .take(3)
                .collect::<Vec<_>>()
                .join("\n"),
            priority: 1,
            source_kind: BrokerSourceKind::Derived,
        },
    ];
    let partial_brief = render_brief("task-a", "v1", &partial_sections);
    let (budget_tokens, budget_bytes) = estimate_text_cost(&partial_brief);
    let (selected, _) = prune_sections_for_budget(
        BrokerAction::Inspect,
        sections,
        budget_tokens + 2,
        budget_bytes + 8,
        8,
    );
    let code_evidence = selected
        .iter()
        .find(|section| section.id == "code_evidence")
        .expect("code_evidence should be retained");
    assert!(code_evidence.body.len() < code_evidence_body.len());
    assert!(code_evidence.body.contains("src/alpha.rs:1"));
}

#[test]
fn inherit_broker_request_defaults_reuses_previous_follow_up_shape() {
    let previous = BrokerGetContextRequest {
        task_id: "task-a".to_string(),
        action: Some(BrokerAction::Inspect),
        budget_tokens: Some(700),
        budget_bytes: Some(2800),
        focus_paths: vec!["src/alpha.rs".to_string()],
        focus_symbols: vec!["Alpha".to_string()],
        query: Some("Where is Alpha defined?".to_string()),
        include_sections: vec!["task_objective".to_string(), "code_evidence".to_string()],
        verbosity: Some(BrokerVerbosity::Rich),
        response_mode: Some(BrokerResponseMode::Delta),
        max_sections: Some(5),
        default_max_items_per_section: Some(3),
        section_item_limits: BTreeMap::from([("code_evidence".to_string(), 2)]),
        persist_artifacts: Some(true),
        ..BrokerGetContextRequest::default()
    };
    let mut current = BrokerGetContextRequest {
        task_id: "task-a".to_string(),
        ..BrokerGetContextRequest::default()
    };

    inherit_broker_request_defaults(&mut current, Some(&previous));

    assert_eq!(current.action, Some(BrokerAction::Inspect));
    assert_eq!(current.query.as_deref(), Some("Where is Alpha defined?"));
    assert_eq!(current.focus_paths, vec!["src/alpha.rs"]);
    assert_eq!(current.focus_symbols, vec!["Alpha"]);
    assert_eq!(
        current.include_sections,
        vec!["task_objective".to_string(), "code_evidence".to_string()]
    );
    assert_eq!(current.response_mode, Some(BrokerResponseMode::Delta));
    assert_eq!(current.section_item_limits["code_evidence"], 2);
}

#[test]
fn reducer_search_only_runs_when_evidence_sections_are_allowed() {
    let only_summary = HashSet::from(["task_objective".to_string(), "progress".to_string()]);
    assert!(!should_run_reducer_search(&only_summary));

    let with_search = HashSet::from(["search_evidence".to_string()]);
    assert!(should_run_reducer_search(&with_search));

    let with_code = HashSet::from(["code_evidence".to_string()]);
    assert!(should_run_reducer_search(&with_code));
}

#[test]
fn broker_edit_context_surfaces_evidence_freshness_for_changed_paths() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        files_read: vec!["src/fresh.rs".to_string()],
        changed_paths_since_checkpoint: vec![
            "src/fresh.rs".to_string(),
            "src/stale.rs".to_string(),
        ],
        changed_symbols_since_checkpoint: vec!["StaleSymbol".to_string()],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let sections = build_broker_sections(
        &root,
        &state,
        &BrokerGetContextRequest {
            task_id: "task-freshness".to_string(),
            action: Some(BrokerAction::Edit),
            include_sections: vec!["evidence_freshness".to_string()],
            ..BrokerGetContextRequest::default()
        },
        &snapshot,
        None,
        None,
    );

    let freshness = sections
        .iter()
        .find(|section| section.id == "evidence_freshness")
        .expect("changed paths should produce evidence freshness section");
    assert!(freshness.body.contains(
        "freshness_score: 1/2 changed path(s) have fresh reads; 1 path(s) and 1 symbol(s) need refresh"
    ));
    assert!(freshness
        .body
        .contains("src/fresh.rs (fresh read recorded)"));
    assert!(freshness
        .body
        .contains("src/stale.rs (refresh read/search before relying on cached evidence)"));
    assert!(freshness.body.contains("StaleSymbol"));
}

#[test]
fn broker_context_debt_clears_after_reads_questions_and_verification() {
    let state = daemon_test_state();
    let debt_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_paths_since_checkpoint: vec!["src/stale.rs".to_string()],
        files_edited: vec!["src/stale.rs".to_string()],
        open_questions: vec![suite_packet_core::AgentQuestion {
            id: "q-auth".to_string(),
            text: "Which auth path owns this?".to_string(),
        }],
        active_decisions: vec![suite_packet_core::AgentDecision {
            id: "hypothesis:auth-cache".to_string(),
            text: "hypothesis active: Auth cache owns stale reads".to_string(),
            related_paths: vec!["src/stale.rs".to_string()],
            related_symbols: Vec::new(),
            related_artifact_ids: Vec::new(),
        }],
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "tool-refute".to_string(),
            sequence: 8,
            tool_name: "cargo test".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Generic,
            result_summary: Some("refuted auth-cache after reading src/stale.rs".to_string()),
            ..suite_packet_core::ToolInvocationSummary::default()
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let debt = broker_context_debt_body(&state, debt_snapshot)
        .expect("debt section should render when debts exist");
    assert!(debt.contains(
        "debt_summary: stale_paths=1 open_questions=1 unverified_edits=1 contradictions=1"
    ));
    assert!(debt.contains("payoff stale_path"));
    assert!(serde_json::to_string(&debt).unwrap().len() < 1024);

    let clear_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_paths_since_checkpoint: vec!["src/stale.rs".to_string()],
        files_read: vec!["src/stale.rs".to_string()],
        files_edited: vec!["src/stale.rs".to_string()],
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "tool-test".to_string(),
            sequence: 9,
            tool_name: "cargo test".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Test,
            result_summary: Some("tests passed".to_string()),
            ..suite_packet_core::ToolInvocationSummary::default()
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    assert!(broker_context_debt_body(&state, clear_snapshot).is_none());
}

#[test]
fn broker_context_debt_surfaces_symbol_payoff_without_stale_path() {
    let state = daemon_test_state();
    let debt = broker_context_debt_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    )
    .expect("symbol-only stale evidence should render debt");

    assert!(debt.contains(
        "debt_summary: stale_paths=0 open_questions=0 unverified_edits=1 contradictions=0"
    ));
    assert!(debt.contains(
        "payoff stale_symbol: inspect/search AuthCache before relying on cached evidence"
    ));
    assert!(debt.lines().count() <= 3);
    assert!(debt.lines().all(|line| line.len() <= 140));
    assert!(serde_json::to_string(&debt).unwrap().len() < 512);
}

#[test]
fn broker_context_debt_orders_symbol_payoff_after_path_payoff() {
    let state = daemon_test_state();
    let debt = broker_context_debt_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_paths_since_checkpoint: vec!["src/auth.rs".to_string()],
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            open_questions: vec![suite_packet_core::AgentQuestion {
                id: "q-auth".to_string(),
                text: "Which auth cache path owns this?".to_string(),
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    )
    .expect("mixed stale evidence should render debt");
    let stale_path_index = debt
        .find("payoff stale_path")
        .expect("stale path payoff should render");
    let stale_symbol_index = debt
        .find("payoff stale_symbol")
        .expect("stale symbol payoff should render");
    let open_questions_index = debt
        .find("payoff open_questions")
        .expect("open question payoff should render");
    let unverified_edits_index = debt
        .find("payoff unverified_edits")
        .expect("unverified edit payoff should render");

    assert!(stale_path_index < stale_symbol_index);
    assert!(stale_symbol_index < open_questions_index);
    assert!(open_questions_index < unverified_edits_index);
    assert!(debt.lines().count() <= 5);
    assert!(debt.lines().all(|line| line.len() <= 140));
    assert!(serde_json::to_string(&debt).unwrap().len() < 768);
}

#[test]
fn broker_context_debt_clears_symbol_only_after_verification() {
    let state = daemon_test_state();
    let debt = broker_context_debt_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "tool-test".to_string(),
                sequence: 9,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(debt.is_none());
}

#[test]
fn broker_symbol_verification_clears_debt_but_preserves_confidence_staleness() {
    let state = daemon_test_state();
    let verified_symbol_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
        evidence_artifact_ids: vec!["artifact-test".to_string()],
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "tool-test".to_string(),
            sequence: 9,
            tool_name: "cargo test auth_cache".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Test,
            result_summary: Some("tests passed".to_string()),
            artifact_id: Some("artifact-test".to_string()),
            ..suite_packet_core::ToolInvocationSummary::default()
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let debt = broker_context_debt_body(&state, verified_symbol_snapshot.clone());
    let confidence = broker_evidence_confidence_body(&state, verified_symbol_snapshot);

    assert!(debt.is_none());
    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("confidence: high"));
    assert!(confidence.contains("backing=artifact"));
}

#[test]
fn broker_symbol_labels_distinguish_confidence_from_debt_payoff() {
    let state = daemon_test_state();
    let symbol_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };

    let debt = broker_context_debt_body(&state, symbol_snapshot.clone())
        .expect("expected context debt for unverified changed symbol");
    let confidence = broker_evidence_confidence_body(&state, symbol_snapshot);

    assert!(confidence.contains("changed_symbols=1"));
    assert!(!confidence.contains("stale_symbols"));
    assert!(debt.contains("payoff stale_symbol"));
    assert!(!debt.contains("changed_symbols="));
}

#[test]
fn broker_confidence_distinguishes_stale_paths_from_changed_symbols() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_paths_since_checkpoint: vec!["src/auth.rs".to_string()],
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("stale_paths=1"));
    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("confidence: medium"));
    assert!(confidence.contains("risk=freshness_mixed"));
    assert!(confidence.contains("payoff=refresh stale_paths+changed_symbols"));
}

#[test]
fn broker_confidence_payoff_priority_orders_repair_actions() {
    assert_eq!(confidence_payoff(100, 1, 1, 1, 1, 1), "evidence usable");
    assert_eq!(
        confidence_payoff(55, 1, 1, 1, 1, 1),
        "rerun failing evidence"
    );
    assert_eq!(
        confidence_payoff(60, 1, 1, 1, 0, 1),
        "refresh stale_paths+changed_symbols"
    );
    assert_eq!(
        confidence_payoff(75, 0, 1, 0, 0, 1),
        "capture artifact-backed symbol evidence"
    );
    assert_eq!(
        confidence_payoff(80, 0, 1, 0, 0, 0),
        "refresh changed_symbols"
    );
    assert_eq!(
        confidence_payoff(80, 0, 0, 1, 0, 0),
        "replace fallback_records"
    );
    assert_eq!(
        confidence_payoff(70, 0, 0, 1, 0, 1),
        "replace fallback_records"
    );
    assert_eq!(
        confidence_payoff(80, 0, 0, 0, 0, 1),
        "capture artifact-backed evidence"
    );
}

#[test]
fn broker_confidence_risk_priority_matches_repair_actions() {
    assert_eq!(confidence_risk(100, 1, 1, 1, 1, 1), "none");
    assert_eq!(confidence_risk(55, 1, 1, 1, 1, 1), "failures");
    assert_eq!(confidence_risk(60, 1, 1, 1, 0, 1), "freshness_mixed");
    assert_eq!(confidence_risk(75, 1, 0, 1, 0, 1), "stale_paths");
    assert_eq!(confidence_risk(75, 0, 1, 0, 0, 1), "missing_backing");
    assert_eq!(confidence_risk(80, 0, 1, 0, 0, 0), "changed_symbols");
    assert_eq!(confidence_risk(80, 0, 0, 1, 0, 0), "fallback_records");
    assert_eq!(confidence_risk(70, 0, 0, 1, 0, 1), "fallback_records");
}

#[test]
fn broker_evidence_confidence_reason_lines_stay_stable() {
    let state = daemon_test_state();
    let backed_success = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            evidence_artifact_ids: vec!["artifact-test".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-backed".to_string(),
                sequence: 1,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                artifact_id: Some("artifact-test".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let unbacked_symbol = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-unbacked-symbol".to_string(),
                sequence: 2,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let mixed_freshness = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_paths_since_checkpoint: vec!["src/auth.rs".to_string()],
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert_eq!(
        broker_confidence_reason_line(&backed_success),
        "- confidence_reason: source=local_tool_state verification=fresh artifacts=1 backing=artifact risk=none payoff=evidence usable"
    );
    assert_eq!(
        broker_confidence_reason_line(&unbacked_symbol),
        "- confidence_reason: source=local_tool_state verification=fresh artifacts=0 backing=missing risk=missing_backing payoff=capture artifact-backed symbol evidence"
    );
    assert_eq!(
        broker_confidence_reason_line(&mixed_freshness),
        "- confidence_reason: source=local_tool_state verification=missing artifacts=0 backing=missing risk=freshness_mixed payoff=refresh stale_paths+changed_symbols"
    );
    for reason_line in [
        broker_confidence_reason_line(&backed_success),
        broker_confidence_reason_line(&unbacked_symbol),
        broker_confidence_reason_line(&mixed_freshness),
    ] {
        assert!(
            reason_line.len() <= 180,
            "reason line too wide: {reason_line}"
        );
    }
    for body in [&backed_success, &unbacked_symbol, &mixed_freshness] {
        assert!(body.len() <= 512, "confidence body too large: {body}");
    }
}

#[test]
fn broker_evidence_confidence_scores_stale_or_fallback_below_fresh_success() {
    let state = daemon_test_state();
    let root = daemon_test_root(&state);
    std::fs::create_dir_all(root.join(".packet28")).unwrap();
    std::fs::write(
        root.join(".packet28/run-savings.jsonl"),
        serde_json::json!({
            "command": "Packet28 run -- rg stale",
            "cwd": root.display().to_string(),
            "family": "search",
            "canonical_kind": "rg",
            "exit_code": 0,
            "raw_est_tokens": 500,
            "reduced_est_tokens": 100,
            "savings_percent": 80.0,
            "fallback_reason": "fff auto preferred backend failed: launch error",
            "failure_fingerprint": null,
            "changed_paths": [],
            "timestamp_unix_ms": 10
        })
        .to_string(),
    )
    .unwrap();
    let stale_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_paths_since_checkpoint: vec!["src/stale.rs".to_string()],
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "search-1".to_string(),
            sequence: 1,
            tool_name: "rg".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Search,
            result_summary: Some("fallback search result".to_string()),
            ..suite_packet_core::ToolInvocationSummary::default()
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let fresh_snapshot = suite_packet_core::AgentSnapshotPayload {
        changed_paths_since_checkpoint: vec!["src/stale.rs".to_string()],
        files_read: vec!["src/stale.rs".to_string()],
        evidence_artifact_ids: vec!["artifact-test".to_string()],
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "test-1".to_string(),
            sequence: 2,
            tool_name: "cargo test".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Test,
            result_summary: Some("tests passed".to_string()),
            artifact_id: Some("artifact-test".to_string()),
            ..suite_packet_core::ToolInvocationSummary::default()
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let stale = broker_evidence_confidence_body(&state, stale_snapshot);
    let fresh = broker_evidence_confidence_body(&state, fresh_snapshot);

    assert!(stale.contains("stale_paths=1"));
    assert!(stale.contains("fallback_records=1"));
    assert!(stale.contains("confidence: low"));
    assert!(stale.contains("risk=stale_paths"));
    assert!(stale.contains("payoff=refresh stale_paths"));
    assert!(fresh.contains("confidence: high"));
    assert!(fresh.contains("verification=fresh"));
    assert!(fresh.contains("risk=none"));
    assert!(fresh.contains("payoff=evidence usable"));
}

#[test]
fn broker_evidence_confidence_penalizes_symbol_only_staleness() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("stale_paths=0"));
    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("confidence: medium"));
    assert!(confidence.contains("risk=changed_symbols"));
    assert!(confidence.contains("payoff=refresh changed_symbols"));
}

#[test]
fn broker_evidence_confidence_keeps_symbol_staleness_visible_after_verification() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            evidence_artifact_ids: vec!["artifact-test".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-1".to_string(),
                sequence: 2,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                artifact_id: Some("artifact-test".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("confidence: high"));
    assert!(confidence.contains("verification=fresh"));
}

#[test]
fn broker_evidence_confidence_scores_failed_symbol_verification_low() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-1".to_string(),
                sequence: 2,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests failed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("failures=1"));
    assert!(confidence.contains("confidence: low"));
    assert!(confidence.contains("verification=missing"));
    assert!(confidence.contains("risk=failures"));
    assert!(confidence.contains("payoff=rerun failing evidence"));
}

#[test]
fn broker_evidence_confidence_scores_unbacked_symbol_verification_medium() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-1".to_string(),
                sequence: 2,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("changed_symbols=1"));
    assert!(confidence.contains("artifact_gaps=1"));
    assert!(confidence.contains("backing=missing"));
    assert!(confidence.contains("confidence: medium"));
    assert!(confidence.contains("verification=fresh"));
    assert!(confidence.contains("artifacts=0 backing=missing risk=missing_backing"));
    assert!(confidence.contains("payoff=capture artifact-backed symbol evidence"));
}

#[test]
fn broker_evidence_confidence_scores_repeated_artifact_gaps_medium() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            recent_tool_invocations: vec![
                suite_packet_core::ToolInvocationSummary {
                    invocation_id: "search-1".to_string(),
                    sequence: 1,
                    tool_name: "rg AuthCache".to_string(),
                    operation_kind: suite_packet_core::ToolOperationKind::Search,
                    result_summary: Some("matched AuthCache".to_string()),
                    ..suite_packet_core::ToolInvocationSummary::default()
                },
                suite_packet_core::ToolInvocationSummary {
                    invocation_id: "read-1".to_string(),
                    sequence: 2,
                    tool_name: "read src/auth.rs".to_string(),
                    operation_kind: suite_packet_core::ToolOperationKind::Read,
                    result_summary: Some("read auth cache code".to_string()),
                    ..suite_packet_core::ToolInvocationSummary::default()
                },
            ],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("artifact_gaps=2"));
    assert!(confidence.contains("backing=missing"));
    assert!(confidence.contains("confidence: medium"));
    assert!(confidence.contains("risk=missing_backing"));
    assert!(confidence.contains("payoff=capture artifact-backed evidence"));
}

#[test]
fn broker_evidence_confidence_caps_missing_backing_below_high() {
    let state = daemon_test_state();
    let confidence = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-1".to_string(),
                sequence: 1,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(confidence.contains("artifact_gaps=1"));
    assert!(confidence.contains("backing=missing"));
    assert!(confidence.contains("score=84"));
    assert!(confidence.contains("confidence: medium"));
    assert!(confidence.contains("risk=missing_backing"));
    assert!(confidence.contains("payoff=capture artifact-backed evidence"));
}

#[test]
fn broker_evidence_confidence_missing_backing_keeps_score_spread() {
    let state = daemon_test_state();
    let symbol_only = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let unbacked_success = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-1".to_string(),
                sequence: 1,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let failed_symbol = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-2".to_string(),
                sequence: 2,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests failed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(symbol_only.contains("score=80"));
    assert!(symbol_only.contains("confidence: medium"));
    assert!(unbacked_success.contains("score=84"));
    assert!(unbacked_success.contains("confidence: medium"));
    assert!(failed_symbol.contains("score=35"));
    assert!(failed_symbol.contains("confidence: low"));
}

#[test]
fn broker_evidence_confidence_orders_symbol_evidence_tiers() {
    let state = daemon_test_state();
    let backed_verified = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            evidence_artifact_ids: vec!["artifact-test".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-backed".to_string(),
                sequence: 1,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                artifact_id: Some("artifact-test".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let unbacked_verified = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-unbacked".to_string(),
                sequence: 2,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let failed = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-failed".to_string(),
                sequence: 3,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests failed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    assert!(backed_verified.contains("confidence: high"));
    assert!(backed_verified.contains("artifact_gaps=0"));
    assert!(backed_verified.contains("backing=artifact"));
    assert!(backed_verified.contains("artifacts=1 backing=artifact"));
    assert!(unbacked_verified.contains("confidence: medium"));
    assert!(unbacked_verified.contains("artifact_gaps=1"));
    assert!(unbacked_verified.contains("backing=missing"));
    assert!(unbacked_verified.contains("artifacts=0 backing=missing"));
    assert!(failed.contains("confidence: low"));
    assert!(failed.contains("failures=1"));
    assert!(failed.contains("backing=missing"));
    assert!(failed.contains("artifacts=0 backing=missing"));
}

#[test]
fn broker_evidence_confidence_backing_labels_stay_compact() {
    let state = daemon_test_state();
    let backed_verified = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            evidence_artifact_ids: vec!["artifact-test".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-backed".to_string(),
                sequence: 1,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                artifact_id: Some("artifact-test".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );
    let unbacked_verified = broker_evidence_confidence_body(
        &state,
        suite_packet_core::AgentSnapshotPayload {
            changed_symbols_since_checkpoint: vec!["AuthCache".to_string()],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "test-unbacked".to_string(),
                sequence: 2,
                tool_name: "cargo test auth_cache".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some("tests passed".to_string()),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    for body in [backed_verified, unbacked_verified] {
        let lines = body.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.len() <= 180));
        assert!(body.contains("backing="));
    }
}

#[test]
fn render_task_memory_lines_surfaces_recent_state() {
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        files_read: vec!["src/alpha.rs".to_string()],
        latest_intention: Some(suite_packet_core::AgentIntention {
            text: "Inspect Alpha before editing".to_string(),
            note: Some("Need a clean handoff breadcrumb".to_string()),
            step_id: Some("investigating".to_string()),
            paths: vec!["src/alpha.rs".to_string()],
            occurred_at_unix: 1,
            ..suite_packet_core::AgentIntention::default()
        }),
        latest_checkpoint_id: Some("cp-1".to_string()),
        checkpoint_note: Some("Validated shuffle scope".to_string()),
        checkpoint_focus_paths: vec!["src/alpha.rs".to_string()],
        checkpoint_focus_symbols: vec!["Alpha".to_string()],
        changed_paths_since_checkpoint: vec!["src/beta.rs".to_string()],
        changed_symbols_since_checkpoint: vec!["Beta".to_string()],
        evidence_artifact_ids: vec!["artifact-1".to_string()],
        recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
            invocation_id: "tool-1".to_string(),
            sequence: 7,
            tool_name: "manual.read".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Read,
            request_summary: Some("Read alpha".to_string()),
            result_summary: Some("Found Alpha".to_string()),
            paths: vec!["src/alpha.rs".to_string()],
            symbols: vec!["Alpha".to_string()],
            occurred_at_unix: 1,
            ..suite_packet_core::ToolInvocationSummary::default()
        }],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };

    let rendered = render_task_memory_lines(&snapshot);

    assert!(rendered.iter().any(
        |line| line.contains("latest intention [investigating]: Inspect Alpha before editing")
    ));
    assert!(rendered
        .iter()
        .any(|line| line.contains("latest intention note: Need a clean handoff breadcrumb")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("latest tool: manual.read")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("recently read: src/alpha.rs")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("latest checkpoint: cp-1")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("checkpoint note: Validated shuffle scope")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("checkpoint focus path: src/alpha.rs")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("checkpoint focus symbol: Alpha")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("changed since checkpoint: src/beta.rs")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("changed symbol since checkpoint: Beta")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("evidence artifact: artifact-1")));
}

#[test]
fn compute_handoff_state_requires_checkpoint_and_tracks_newer_intentions() {
    let empty_snapshot = suite_packet_core::AgentSnapshotPayload::default();
    let (ready_without_checkpoint, _) = compute_handoff_state(None, &empty_snapshot);
    assert!(!ready_without_checkpoint);

    let snapshot = suite_packet_core::AgentSnapshotPayload {
        latest_checkpoint_id: Some("cp-1".to_string()),
        latest_intention: Some(suite_packet_core::AgentIntention {
            text: "Resume editing beta".to_string(),
            occurred_at_unix: 20,
            ..suite_packet_core::AgentIntention::default()
        }),
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let (ready_initial, _) = compute_handoff_state(None, &snapshot);
    assert!(ready_initial);

    let task = TaskRecord {
        task_id: "task-a".to_string(),
        latest_handoff_generated_at_unix: Some(10),
        latest_handoff_checkpoint_id: Some("cp-1".to_string()),
        ..TaskRecord::default()
    };
    let (ready_newer_intention, _) = compute_handoff_state(Some(&task), &snapshot);
    assert!(ready_newer_intention);

    let stale_snapshot = suite_packet_core::AgentSnapshotPayload {
        latest_checkpoint_id: Some("cp-1".to_string()),
        latest_intention: Some(suite_packet_core::AgentIntention {
            text: "Resume editing beta".to_string(),
            occurred_at_unix: 5,
            ..suite_packet_core::AgentIntention::default()
        }),
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let (ready_stale, _) = compute_handoff_state(Some(&task), &stale_snapshot);
    assert!(!ready_stale);
}

#[test]
fn compute_handoff_state_accepts_newer_hook_boundaries_with_legacy_second_timestamps() {
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        latest_intention: Some(suite_packet_core::AgentIntention {
            text: "Resume editing beta".to_string(),
            occurred_at_unix: 1_700_000_001_500,
            ..suite_packet_core::AgentIntention::default()
        }),
        ..suite_packet_core::AgentSnapshotPayload::default()
    };
    let task = TaskRecord {
        task_id: "task-legacy-boundary".to_string(),
        latest_handoff_generated_at_unix: Some(1_700_000_000_250),
        latest_hook_boundary_at_unix: Some(1_700_000_001),
        latest_hook_boundary_kind: Some("stop".to_string()),
        ..TaskRecord::default()
    };

    let (ready, reason) = compute_handoff_state(Some(&task), &snapshot);

    assert!(ready);
    assert!(reason.contains("stop"));
}

#[test]
fn checkpoint_context_lines_surface_saved_focus() {
    let snapshot = suite_packet_core::AgentSnapshotPayload {
        latest_checkpoint_id: Some("cp-42".to_string()),
        checkpoint_note: Some("Seeded shuffle plan".to_string()),
        checkpoint_focus_paths: vec![
            "apache/src/main/java/org/apache/commons/lang3/ArrayUtils.java".to_string(),
        ],
        checkpoint_focus_symbols: vec!["shuffle".to_string()],
        ..suite_packet_core::AgentSnapshotPayload::default()
    };

    let rendered = render_checkpoint_context_lines(&snapshot);

    assert!(rendered
        .iter()
        .any(|line| line.contains("checkpoint: cp-42")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("note: Seeded shuffle plan")));
    assert!(rendered.iter().any(|line| line
        .contains("focus path: apache/src/main/java/org/apache/commons/lang3/ArrayUtils.java")));
    assert!(rendered
        .iter()
        .any(|line| line.contains("focus symbol: shuffle")));
}

#[test]
fn prepare_handoff_only_resumes_recorded_handoff_artifacts() {
    let state = daemon_test_state();
    let root = state.lock().unwrap().root.clone();
    let context = BrokerGetContextResponse {
        context_version: "ctx-1".to_string(),
        response_mode: BrokerResponseMode::Full,
        artifact_id: Some("ctx-1".to_string()),
        brief: "context".to_string(),
        ..BrokerGetContextResponse::default()
    };
    let version_path = task_version_json_path(&root, "task-resume-guard", "ctx-1");
    std::fs::create_dir_all(version_path.parent().unwrap()).unwrap();
    std::fs::write(&version_path, serde_json::to_vec_pretty(&context).unwrap()).unwrap();

    {
        let mut guard = state.lock().unwrap();
        let task = ensure_task_record_mut(&mut guard.tasks, "task-resume-guard");
        task.task_id = "task-resume-guard".to_string();
        task.latest_context_version = Some("ctx-1".to_string());
        task.latest_handoff_artifact_id = Some("handoff-1".to_string());
        persist_state(&guard).unwrap();
    }

    let response = broker_prepare_handoff(
        state,
        BrokerPrepareHandoffRequest {
            task_id: "task-resume-guard".to_string(),
            query: None,
            response_mode: Some(BrokerResponseMode::Slim),
            include_debug_memory: false,
        },
    )
    .unwrap();

    assert!(!response.handoff_ready);
    assert!(response.context.is_none());
    assert_eq!(
        response.latest_handoff_artifact_id.as_deref(),
        Some("handoff-1")
    );
}

#[test]
fn prepare_handoff_warns_when_tool_evidence_contradicts_active_hypothesis() {
    let state = daemon_test_state();
    state.lock().unwrap().agent_snapshots.insert(
        "task-contradiction".to_string(),
        suite_packet_core::AgentSnapshotPayload {
            task_id: "task-contradiction".to_string(),
            latest_checkpoint_id: Some("checkpoint-1".to_string()),
            active_decisions: vec![suite_packet_core::AgentDecision {
                id: "hypothesis:auth-cache".to_string(),
                text: "hypothesis active: Auth cache invalidation is suspect".to_string(),
                related_paths: vec!["src/auth.rs".to_string()],
                related_symbols: vec!["AuthCache".to_string()],
                related_artifact_ids: Vec::new(),
            }],
            recent_tool_invocations: vec![suite_packet_core::ToolInvocationSummary {
                invocation_id: "tool-contradict".to_string(),
                sequence: 42,
                tool_name: "cargo test".to_string(),
                operation_kind: suite_packet_core::ToolOperationKind::Test,
                result_summary: Some(
                    "refuted hypothesis auth-cache while testing src/auth.rs".to_string(),
                ),
                ..suite_packet_core::ToolInvocationSummary::default()
            }],
            ..suite_packet_core::AgentSnapshotPayload::default()
        },
    );

    let response = broker_prepare_handoff(
        state,
        BrokerPrepareHandoffRequest {
            task_id: "task-contradiction".to_string(),
            query: None,
            response_mode: Some(BrokerResponseMode::Slim),
            include_debug_memory: false,
        },
    )
    .unwrap();

    assert!(response.handoff_ready);
    assert_eq!(response.warnings.len(), 1);
    assert!(response.warnings[0].contains("handoff_contradiction"));
    assert!(response.warnings[0].contains("hypothesis:auth-cache"));
    assert!(response.warnings[0].contains("tool #42"));
    assert_eq!(response.readiness.status, "caution");
    assert!(response.readiness.score < 85);
    assert!(response
        .readiness
        .reasons
        .iter()
        .any(|reason| reason == "contradictions=1"));
}

#[test]
fn prepare_handoff_readiness_score_rises_after_verification_evidence() {
    let state = daemon_test_state();
    for (task_id, with_verification) in [
        ("task-readiness-unverified", false),
        ("task-readiness-verified", true),
    ] {
        state.lock().unwrap().agent_snapshots.insert(
            task_id.to_string(),
            suite_packet_core::AgentSnapshotPayload {
                task_id: task_id.to_string(),
                latest_checkpoint_id: Some("checkpoint-1".to_string()),
                changed_paths_since_checkpoint: vec!["src/lib.rs".to_string()],
                latest_intention: Some(suite_packet_core::AgentIntention {
                    text: "Hand off after library edit".to_string(),
                    occurred_at_unix: 1,
                    ..suite_packet_core::AgentIntention::default()
                }),
                recent_tool_invocations: if with_verification {
                    vec![suite_packet_core::ToolInvocationSummary {
                        invocation_id: "test-1".to_string(),
                        sequence: 7,
                        tool_name: "cargo test".to_string(),
                        operation_kind: suite_packet_core::ToolOperationKind::Test,
                        result_summary: Some("tests passed".to_string()),
                        ..suite_packet_core::ToolInvocationSummary::default()
                    }]
                } else {
                    Vec::new()
                },
                ..suite_packet_core::AgentSnapshotPayload::default()
            },
        );
    }

    let unverified = broker_prepare_handoff(
        state.clone(),
        BrokerPrepareHandoffRequest {
            task_id: "task-readiness-unverified".to_string(),
            response_mode: Some(BrokerResponseMode::Slim),
            ..BrokerPrepareHandoffRequest::default()
        },
    )
    .unwrap();
    let verified = broker_prepare_handoff(
        state,
        BrokerPrepareHandoffRequest {
            task_id: "task-readiness-verified".to_string(),
            response_mode: Some(BrokerResponseMode::Slim),
            ..BrokerPrepareHandoffRequest::default()
        },
    )
    .unwrap();

    assert!(verified.readiness.score > unverified.readiness.score);
    assert!(unverified
        .readiness
        .reasons
        .iter()
        .any(|reason| reason == "missing_recent_verification"));
    assert!(!verified
        .readiness
        .reasons
        .iter()
        .any(|reason| reason == "missing_recent_verification"));
    assert!(serde_json::to_string(&verified.readiness).unwrap().len() < 512);
}

#[test]
fn instruction_file_resolution_rewrites_larger_markdown() {
    let state = daemon_test_state();
    let workspace_root = state.lock().unwrap().root.display().to_string();
    let response = resolve_instruction_file(
        state,
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
        state,
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
        state,
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
        state,
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
