#[path = "support/verify.rs"]
mod verify;

use predicates::prelude::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;
use verify::suite_cmd;

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
