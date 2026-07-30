use super::support::*;
use super::*;

#[test]
fn authenticated_evidence_keeps_sparse_high_line_numbers_bounded() {
    let file = ReducerSearchFile {
        path: "src/high_line.rs".to_string(),
        definition_hits: 1,
        preview_matches: vec![(1_000_000, "pub fn authenticated_symbol() {}".to_string())],
        ..ReducerSearchFile::default()
    };

    let evidence = build_authenticated_search_evidence(&[file], 3);

    assert_eq!(
        evidence["src/high_line.rs"].rendered_lines,
        ["- src/high_line.rs:1000000 pub fn authenticated_symbol() {}"]
    );
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
            ("src/alpha.rs", "pub struct Alpha;\n"),
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
