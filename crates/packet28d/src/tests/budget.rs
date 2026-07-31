use super::*;

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

fn clone_elision_benchmark_sections() -> Vec<BrokerSection> {
    (0..32)
        .map(|index| BrokerSection {
            id: if index == 0 {
                "task_objective".to_string()
            } else {
                format!("optional_{index}")
            },
            title: format!("Section {index}"),
            body: format!("section-{index}:{}", "x".repeat(32 * 1024)),
            priority: if index == 0 { 1 } else { 3 },
            source_kind: BrokerSourceKind::Derived,
        })
        .collect()
}

#[test]
#[ignore = "release-only PER-09 benchmark; run explicitly with --ignored --nocapture"]
fn benchmark_budget_pruning_clone_elision() {
    const ITERATIONS: u32 = 128;
    const BUDGET_TOKENS: u64 = 8_192;
    const BUDGET_BYTES: u64 = 32 * 1024;

    let parity_input = clone_elision_benchmark_sections();
    let reference = prune_sections_for_budget(
        BrokerAction::Plan,
        parity_input.clone(),
        BUDGET_TOKENS,
        BUDGET_BYTES,
        8,
    );
    let direct = prune_sections_for_budget(
        BrokerAction::Plan,
        parity_input,
        BUDGET_TOKENS,
        BUDGET_BYTES,
        8,
    );
    assert_eq!(reference.0, direct.0);
    assert_eq!(
        reference
            .1
            .iter()
            .map(|candidate| (&candidate.section_id, &candidate.reason))
            .collect::<Vec<_>>(),
        direct
            .1
            .iter()
            .map(|candidate| (&candidate.section_id, &candidate.reason))
            .collect::<Vec<_>>()
    );

    let mut cloned_path_nanos = 0_u128;
    for _ in 0..ITERATIONS {
        let sections = clone_elision_benchmark_sections();
        let started = std::time::Instant::now();
        let result = prune_sections_for_budget(
            BrokerAction::Plan,
            std::hint::black_box(sections.clone()),
            BUDGET_TOKENS,
            BUDGET_BYTES,
            8,
        );
        cloned_path_nanos = cloned_path_nanos.saturating_add(started.elapsed().as_nanos());
        std::hint::black_box(result);
        std::hint::black_box(sections);
    }

    let mut moved_path_nanos = 0_u128;
    for _ in 0..ITERATIONS {
        let sections = clone_elision_benchmark_sections();
        let started = std::time::Instant::now();
        let result = prune_sections_for_budget(
            BrokerAction::Plan,
            std::hint::black_box(sections),
            BUDGET_TOKENS,
            BUDGET_BYTES,
            8,
        );
        moved_path_nanos = moved_path_nanos.saturating_add(started.elapsed().as_nanos());
        std::hint::black_box(result);
    }

    println!(
        "{{\"iterations\":{ITERATIONS},\"fixture_sections\":32,\"body_bytes_per_section\":32768,\"cloned_path_nanos\":{cloned_path_nanos},\"moved_path_nanos\":{moved_path_nanos}}}"
    );
}
