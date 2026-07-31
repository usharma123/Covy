use super::support::*;
use super::*;

#[test]
fn merged_unique_many_normalizes_all_groups_in_one_pass() {
    let first = vec![" beta ".to_string(), String::new(), "alpha".to_string()];
    let second = vec!["gamma".to_string(), "beta".to_string()];
    let third = vec![" delta ".to_string(), "alpha".to_string()];

    assert_eq!(
        merged_unique_many([first.as_slice(), second.as_slice(), third.as_slice()]),
        ["alpha", "beta", "delta", "gamma"]
    );
    assert_eq!(first, [" beta ", "", "alpha"]);
    assert_eq!(second, ["gamma", "beta"]);
    assert_eq!(third, [" delta ", "alpha"]);
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
    refresh_test_repo_runtime(&state);

    let response = broker_validate_plan(
        state.clone(),
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
        refresh_test_repo_runtime(&state);

        let response = broker_validate_plan(
            state.clone(),
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
    refresh_test_repo_runtime(&state);

    let response = broker_validate_plan(
        state.clone(),
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
    refresh_test_repo_runtime(&state);

    let response = broker_validate_plan(
        state.clone(),
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
    refresh_test_repo_runtime(&state);

    let response = broker_validate_plan(
        state.clone(),
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
