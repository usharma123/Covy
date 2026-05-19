use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_verify_filters_runs_inline_toml_tests() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(root.path().join(".packet28")).unwrap();
    fs::write(
        root.path().join(".packet28").join("filters.toml"),
        r#"
schema_version = 1

[filters.demo]
match_command = "^demo-tool\\b"
strip_lines_matching = ["^debug:"]
on_empty = "demo-tool: ok"

[[tests.demo]]
name = "drops debug noise"
input = """
debug: first
useful
"""
expected = "useful"
"#,
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", &home)
        .args([
            "verify",
            "filters",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "--require-all",
            "--trust",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"ok\":true"))
        .stdout(predicate::str::contains("\"passed\":1"))
        .stdout(predicate::str::contains("\"trusted_filters\""))
        .stdout(predicate::str::contains("drops debug noise"));
}

#[test]
fn test_verify_experiments_checks_manifest_evidence() {
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs/experiments/runtime-live")).unwrap();
    fs::write(
        root.path().join("docs/experiments/runtime-live/SMOKE.md"),
        "raw smoke evidence\nsaved_tokens: 32\nunsupported fallback\n",
    )
    .unwrap();
    let manifest = root.path().join("experiments.json");
    fs::write(
        &manifest,
        r#"{
  "experiments": [
    {
      "id": "claude-runtime-smoke",
      "workflow": "Claude Code hook smoke",
      "commands": ["Packet28 doctor --json"],
      "artifacts": ["docs/experiments/runtime-live/SMOKE.md"]
    },
    {
      "id": "missing-fallback",
      "workflow": "",
      "commands": [""],
      "artifacts": ["docs/experiments/runtime-live/MISSING.md"],
      "metrics": [
        {"name": "saved_tokens", "value": 5, "min": 10},
        {"name": "latency_ms", "value": 120, "max": 100},
        {"name": "artifact_backed_metric", "value": 1, "min": 1, "evidence": ["not present in artifact"]},
        {"name": "missing-value"}
      ],
      "runtime_versions": [
        {"name": "claude-code", "version": ""}
      ],
      "fallback_reasons": ["unsupported"]
    },
    {
      "id": "missing-script-command",
      "workflow": "Missing local script command",
      "commands": ["docs/experiments/missing-script.sh --flag"],
      "artifacts": ["docs/experiments/runtime-live/SMOKE.md"]
    }
  ]
}"#,
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "verify",
            "experiments",
            "--root",
            root.path().to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
            "--json",
            "--require-workflow",
            "Claude Code hook smoke",
            "--require-workflow",
            "missing required workflow",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"ok\":false"))
        .stdout(predicate::str::contains(
            "\"kind\":\"missing_required_workflow\"",
        ))
        .stdout(predicate::str::contains("\"kind\":\"uncovered_workflow\""))
        .stdout(predicate::str::contains(
            "\"kind\":\"missing_command_evidence\"",
        ))
        .stdout(predicate::str::contains(
            "\"kind\":\"missing_command_path\"",
        ))
        .stdout(predicate::str::contains("\"kind\":\"missing_artifact\""))
        .stdout(predicate::str::contains("\"kind\":\"metric_below_min\""))
        .stdout(predicate::str::contains("\"kind\":\"metric_above_max\""))
        .stdout(predicate::str::contains(
            "\"kind\":\"missing_metric_evidence\"",
        ))
        .stdout(predicate::str::contains(
            "\"kind\":\"missing_metric_artifact_evidence\"",
        ))
        .stdout(predicate::str::contains(
            "\"kind\":\"missing_metric_value\"",
        ))
        .stdout(predicate::str::contains(
            "\"kind\":\"missing_runtime_version\"",
        ))
        .stdout(predicate::str::contains("\"kind\":\"unexpected_fallback\""));

    fs::write(
        &manifest,
        r#"{
  "experiments": [
    {
      "id": "bad-allowed-fallback",
      "workflow": "Claude Code hook smoke",
      "commands": ["Packet28 doctor --json"],
      "artifacts": ["docs/experiments/runtime-live/SMOKE.md"],
      "metrics": [
        {"name": "saved_tokens", "value": 32, "min": 10, "evidence": ["saved_tokens: 32"]}
      ],
      "fallback_reasons": ["not present in artifact"],
      "allow_fallbacks": true,
      "runtime_versions": [
        {"name": "claude-code", "version": "2.1.139"}
      ]
    }
  ]
}"#,
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "verify",
            "experiments",
            "--root",
            root.path().to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "\"kind\":\"missing_fallback_artifact_evidence\"",
        ));

    fs::write(
        &manifest,
        r#"{
  "experiments": [
    {
      "id": "claude-runtime-smoke",
      "workflow": "Claude Code hook smoke",
      "commands": ["Packet28 doctor --json"],
      "artifacts": ["docs/experiments/runtime-live/SMOKE.md"],
      "metrics": [
        {"name": "saved_tokens", "value": 32, "min": 10, "evidence": ["saved_tokens: 32"]}
      ],
      "runtime_versions": [
        {"name": "claude-code", "version": "2.1.139"}
      ]
    }
  ]
}"#,
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "verify",
            "experiments",
            "--root",
            root.path().to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
            "--require-workflow",
            "Claude Code hook smoke",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 experiment(s) verified"));
}

#[test]
fn test_verify_handoffs_reports_ci_summary_and_threshold() {
    let root = TempDir::new().unwrap();
    let task_id = "task-verify-handoffs";
    for (context_version, body) in [
        (
            "ctx-ci-1",
            "cargo test -p suite-cli ci_handoff_test $PACKET28_CI_MISSING_ENV_12345",
        ),
        ("ctx-ci-2", "cargo test -p suite-cli ci_handoff_test"),
        (
            "ctx-ci-3",
            "cargo test -p suite-cli ci_handoff_test $PACKET28_CI_MISSING_ENV_12345",
        ),
    ] {
        let path =
            packet28_daemon_core::task_version_json_path(root.path(), task_id, context_version);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "context_version": context_version,
                "artifact_id": context_version,
                "brief": "## Task Objective\nCI handoff readiness.",
                "sections": [{
                    "id": "verification",
                    "title": "Verification",
                    "body": body
                }],
                "changed_paths_since_checkpoint": ["src/lib.rs"],
                "next_action_summary": "verify CI handoff summary"
            }))
            .unwrap(),
        )
        .unwrap();
    }

    suite_cmd()
        .current_dir(root.path())
        .args([
            "verify",
            "handoffs",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"ok\":false"))
        .stdout(predicate::str::contains("\"regression_count\":1"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "verify",
            "handoffs",
            "--root",
            root.path().to_str().unwrap(),
            "--max-regressions",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("handoff_latest_status=blocked"))
        .stdout(predicate::str::contains("handoff_regression_count=1"))
        .stdout(predicate::str::contains("handoff_ok=true"));
}
