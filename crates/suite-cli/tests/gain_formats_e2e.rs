use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_gain_reports_savings_formats_after_reduced_git_status() {
    let root = TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(root.path())
        .status()
        .unwrap();
    fs::write(root.path().join("tracked.txt"), "changed\n").unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "git",
            "status",
            "--short",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"git\""))
        .stdout(predicate::str::contains("\"raw_est_tokens\""))
        .stdout(predicate::str::contains("\"savings_percent\""))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));

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
