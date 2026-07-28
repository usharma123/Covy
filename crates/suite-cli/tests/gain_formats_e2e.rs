#[path = "support/gain.rs"]
mod gain;

use gain::{init_git_status_fixture, record_git_status_run, suite_cmd};
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn test_gain_reports_savings_formats_after_reduced_git_status() {
    let root = TempDir::new().unwrap();
    init_git_status_fixture(&root);
    record_git_status_run(&root);

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"invocation_count\":1"))
        .stdout(predicate::str::contains("run_reducer:git"))
        .stdout(predicate::str::contains("\"route_roi\""))
        .stdout(predicate::str::contains("\"saved_est_tokens\""))
        .stdout(predicate::str::contains("\"avg_saved_tokens\""))
        .stdout(predicate::str::contains("\"downstream_followup_count\""))
        .stdout(predicate::str::contains("\"avg_followups_per_invocation\""));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "csv",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("kind,name,value"))
        .stdout(predicate::str::contains("summary,invocation_count,1"))
        .stdout(predicate::str::contains("route,run_reducer:git,1"));
    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "history",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "timestamp_unix_ms,family,raw_est_tokens",
        ))
        .stdout(predicate::str::contains("git status --short"));

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "--history"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "timestamp_unix_ms,family,raw_est_tokens",
        ))
        .stdout(predicate::str::contains("git status --short"));

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "-H"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "timestamp_unix_ms,family,raw_est_tokens",
        ))
        .stdout(predicate::str::contains("git status --short"));

    for format in ["daily", "weekly", "monthly"] {
        suite_cmd()
            .current_dir(root.path())
            .args([
                "gain",
                "--root",
                root.path().to_str().unwrap(),
                "--format",
                format,
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "period,invocation_count,raw_est_tokens",
            ))
            .stdout(predicate::str::contains(",1,"));
    }

    for flag in ["--daily", "--weekly", "--monthly"] {
        suite_cmd()
            .current_dir(root.path())
            .args(["gain", "--root", root.path().to_str().unwrap(), flag])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "period,invocation_count,raw_est_tokens",
            ))
            .stdout(predicate::str::contains(",1,"));
    }
}
