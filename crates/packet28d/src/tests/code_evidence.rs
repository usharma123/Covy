use super::*;

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
