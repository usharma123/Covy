use super::*;

#[test]
fn validates_new_tool_call_and_human_review_paths() {
    let cfg = r#"
version: 1
policy:
  tools:
    allowlist: ["diffy"]
  reducers:
    allowlist: ["analyze"]
  paths:
    include: ["src/**"]
    exclude: []
  token_budget:
    cap: 1200
  runtime_budget:
    cap_ms: 1200
  tool_call_budget:
    cap: 5
  redaction:
    forbidden_patterns: []
  human_review:
    required: false
    on_policy_violation: true
    on_budget_violation: true
    on_redaction_violation: true
    paths: ["src/critical/**"]
"#;

    let parsed = parse_context_strict(cfg).unwrap();
    assert_eq!(parsed.policy.effective_tool_call_cap(), Some(5));
}

#[test]
fn reducer_allowlist_matches_leaf() {
    let allowed = vec!["analyze".to_string()];
    assert_eq!(
        match_reducer_allowlist("diffy.analyze", &allowed),
        Some("analyze".to_string())
    );
}

#[test]
fn policy_parser_preserves_explicit_string_scalars() {
    let yaml = r#"
version: !!int 1
policy:
  tools:
    allowlist: [!!str true, !!str 001, on]
  reducers:
    allowlist: [!!str null]
  paths:
    include: [!!str src/**]
  token_budget:
    cap: !!int 300
  redaction:
    forbidden_patterns: [!!str "(?i)secret"]
"#;

    let config = parse_context_strict(yaml).unwrap();

    assert_eq!(
        config.policy.effective_allowed_tools(),
        ["true", "001", "on"].map(str::to_string)
    );
}

#[test]
fn policy_parser_rejects_malformed_yaml() {
    let result = validate_config_str("version: 1\npolicy:\n  tools:\n    allowlist: [diffy\n");

    assert!(!result.valid);
}
