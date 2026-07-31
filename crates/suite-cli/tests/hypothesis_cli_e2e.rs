#[path = "support/hypothesis.rs"]
mod hypothesis;
#[expect(
    dead_code,
    reason = "this integration binary exercises a focused subset of the shared harness"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use hypothesis::{ensure_packet28d_built, init_repo, suite_cmd, write_repo_fixture};
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn test_hypothesis_cli_tracks_active_assumptions() {
    ensure_packet28d_built();
    let root = TempDir::new().unwrap();
    init_repo(root.path());
    write_repo_fixture(root.path());
    let task_id = "task-hypothesis-smoke";

    suite_cmd()
        .current_dir(root.path())
        .args([
            "hypothesis",
            "add",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            task_id,
            "--id",
            "auth-cache",
            "--path",
            "src/auth.rs",
            "--symbol",
            "AuthCache",
            "--artifact-id",
            "artifact-auth-cache",
            "--json",
            "Auth cache invalidation is the regression source",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\":\"active\""))
        .stdout(predicate::str::contains(
            "\"decision_id\":\"hypothesis:auth-cache\"",
        ));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "hypothesis",
            "list",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            task_id,
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"id\":\"auth-cache\""))
        .stdout(predicate::str::contains(
            "\"related_paths\":[\"src/auth.rs\"]",
        ))
        .stdout(predicate::str::contains(
            "\"related_symbols\":[\"AuthCache\"]",
        ))
        .stdout(predicate::str::contains(
            "\"related_artifact_ids\":[\"artifact-auth-cache\"]",
        ))
        .stdout(predicate::str::contains("Auth cache invalidation"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "hypothesis",
            "reject",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            task_id,
            "auth-cache",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("hypothesis auth-cache rejected"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "hypothesis",
            "list",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            task_id,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("active_hypotheses=0"));

    suite_cmd()
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}
