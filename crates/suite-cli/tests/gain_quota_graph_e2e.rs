#[path = "support/gain.rs"]
mod gain;
#[expect(
    dead_code,
    reason = "shared integration harness APIs are exercised by sibling test binaries"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use gain::{init_git_status_fixture, record_git_status_run, suite_cmd};
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn test_gain_quota_graph_all_and_reset_formats() {
    let root = TempDir::new().unwrap();
    init_git_status_fixture(&root);
    record_git_status_run(&root);

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "quota",
            "--quota-tokens",
            "1000",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("tier=custom"))
        .stdout(predicate::str::contains("quota_tokens=1000"))
        .stdout(predicate::str::contains("quota_used_pct="))
        .stdout(predicate::str::contains("quota_avoided_pct="));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--quota",
            "--quota-tokens",
            "1000",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("tier=custom"))
        .stdout(predicate::str::contains("quota_tokens=1000"))
        .stdout(predicate::str::contains("quota_used_pct="))
        .stdout(predicate::str::contains("quota_avoided_pct="));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--quota",
            "--tier",
            "pro",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("tier=pro"))
        .stdout(predicate::str::contains("quota_tokens=6000000"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "-q",
            "-t",
            "5x",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("tier=5x"))
        .stdout(predicate::str::contains("quota_tokens=30000000"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "graph",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28 savings graph"))
        .stdout(predicate::str::contains("saved_est_tokens="))
        .stdout(predicate::str::contains("run_reducer:git"));

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "--graph"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28 savings graph"))
        .stdout(predicate::str::contains("impact"))
        .stdout(predicate::str::contains("run_reducer:git"));

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "-g"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28 savings graph"))
        .stdout(predicate::str::contains("share"))
        .stdout(predicate::str::contains("run_reducer:git"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "all",
            "--quota-tokens",
            "1000",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[summary]"))
        .stdout(predicate::str::contains("[graph]"))
        .stdout(predicate::str::contains("[daily]"))
        .stdout(predicate::str::contains("[quota]"))
        .stdout(predicate::str::contains("[failures]"));

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "-a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[summary]"))
        .stdout(predicate::str::contains("[graph]"))
        .stdout(predicate::str::contains("[failures]"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--all",
            "--quota-tokens",
            "1000",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[summary]"))
        .stdout(predicate::str::contains("[graph]"))
        .stdout(predicate::str::contains("[daily]"))
        .stdout(predicate::str::contains("[quota]"))
        .stdout(predicate::str::contains("[failures]"));

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "--reset"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires --yes"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--reset",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Token savings stats reset to zero.",
        ))
        .stdout(predicate::str::contains("cleared_run_savings=1"));

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"invocation_count\":0"));
}
