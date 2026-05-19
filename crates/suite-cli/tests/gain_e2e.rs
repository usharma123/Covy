use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
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

#[test]
fn test_gain_reports_failed_and_fallback_runs() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(root.path())
        .output()
        .unwrap();
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

#[test]
fn test_gain_warns_about_disabled_bypass_sessions_like_rtk() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root.path().join("claude-projects").join("project");
    fs::create_dir_all(&sessions_dir).unwrap();
    let line = json!({
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": "PACKET28_DISABLED=1 git status --short" }
                },
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": "RTK_DISABLED=true cargo test" }
                },
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": "git status --short" }
                }
            ]
        }
    });
    fs::write(
        sessions_dir.join("session-gain-disabled.jsonl"),
        format!("{line}\n"),
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "used PACKET28_DISABLED/RTK_DISABLED unnecessarily",
        ))
        .stderr(predicate::str::contains("Packet28 discover"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
}

#[test]
fn test_gain_cc_economics_merges_ccusage_and_packet28_savings() {
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
        .success();

    let ccusage = root.path().join("ccusage.json");
    fs::write(
        &ccusage,
        r#"{
  "monthly": [{
    "month": "2026-05",
    "inputTokens": 1000,
    "outputTokens": 200,
    "cacheCreationTokens": 80,
    "cacheReadTokens": 500,
    "totalTokens": 1780,
    "totalCost": 3.5
  }]
}"#,
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "cc-economics",
            "--root",
            root.path().to_str().unwrap(),
            "--ccusage-json",
            ccusage.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"source\""))
        .stdout(predicate::str::contains("\"cc_total_tokens\":1780"))
        .stdout(predicate::str::contains("\"packet28_commands\":1"))
        .stdout(predicate::str::contains("\"packet28_saved_tokens\""))
        .stdout(predicate::str::contains(
            "\"weighted_input_cost_per_token\"",
        ));
}
