#[path = "support/verify.rs"]
mod verify;

use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;
use verify::suite_cmd;

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
