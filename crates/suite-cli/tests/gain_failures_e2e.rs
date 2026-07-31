#[expect(
    dead_code,
    reason = "shared integration harness APIs are exercised by sibling test binaries"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_gain_reports_failed_and_fallback_runs() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    process_harness::run_git(root.path(), &["init"]);
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "sh",
            "-c",
            "echo packet28 failure >&2; exit 7",
        ])
        .assert()
        .failure()
        .code(7)
        .stdout(predicate::str::contains("\"fallback_reason\""))
        .stdout(predicate::str::contains(
            "\"failure_fingerprint\":\"failure:v1:",
        ));
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "sh",
            "-c",
            "echo packet28 failure >&2; exit 7",
        ])
        .assert()
        .failure()
        .code(7)
        .stdout(predicate::str::contains(
            "\"failure_fingerprint\":\"failure:v1:",
        ));
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "sh",
            "-c",
            "printf fixed > src/fix.txt; echo packet28 fix",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"exit_code\":0"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "failures",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "timestamp_unix_ms,family,exit_code,fallback_reason,failure_fingerprint,repeat_count,next_success_command,next_success_changed_paths,fix_advice,command",
        ))
        .stdout(predicate::str::contains("fallback,7"))
        .stdout(predicate::str::contains(",2,sh -c"))
        .stdout(predicate::str::contains("echo packet28 fix"))
        .stdout(predicate::str::contains("src/fix.txt"))
        .stdout(predicate::str::contains("repeated failure: retry with"))
        .stdout(predicate::str::contains("failure:v1:"))
        .stdout(predicate::str::contains("unsupported"))
        .stdout(predicate::str::contains("packet28 failure"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "failures",
            "--remember-advice",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "remembered_failure_advice_count=1",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["feedback", "search", "packet28 fix"])
        .assert()
        .success()
        .stdout(predicate::str::contains("repeated failure: retry with"))
        .stdout(predicate::str::contains("failure_fingerprint:"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--failures",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "timestamp_unix_ms,family,exit_code,fallback_reason,failure_fingerprint,repeat_count,next_success_command,next_success_changed_paths,fix_advice,command",
        ))
        .stdout(predicate::str::contains("fallback,7"))
        .stdout(predicate::str::contains(",2,sh -c"))
        .stdout(predicate::str::contains("echo packet28 fix"))
        .stdout(predicate::str::contains("src/fix.txt"))
        .stdout(predicate::str::contains("repeated failure: retry with"))
        .stdout(predicate::str::contains("failure:v1:"))
        .stdout(predicate::str::contains("unsupported"))
        .stdout(predicate::str::contains("packet28 failure"));

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "-F"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "timestamp_unix_ms,family,exit_code,fallback_reason,failure_fingerprint,repeat_count,next_success_command,next_success_changed_paths,fix_advice,command",
        ))
        .stdout(predicate::str::contains("fallback,7"))
        .stdout(predicate::str::contains(",2,sh -c"))
        .stdout(predicate::str::contains("echo packet28 fix"))
        .stdout(predicate::str::contains("src/fix.txt"))
        .stdout(predicate::str::contains("repeated failure: retry with"))
        .stdout(predicate::str::contains("failure:v1:"))
        .stdout(predicate::str::contains("unsupported"))
        .stdout(predicate::str::contains("packet28 failure"));
}
