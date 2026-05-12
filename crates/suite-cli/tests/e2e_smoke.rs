use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::OnceLock;
use std::time::Duration;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn agent_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("packet28-agent")
}

fn mcp_cmd() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
}

fn ensure_packet28d_built() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let status = std::process::Command::new("cargo")
            .args(["build", "-p", "packet28d"])
            .status()
            .unwrap();
        assert!(status.success(), "failed to build packet28d");
    });
}

fn fixture(rel: &str) -> String {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    workspace
        .join("tests")
        .join("fixtures")
        .join(rel)
        .to_string_lossy()
        .to_string()
}

fn write_manifest(path: &Path) {
    let line = format!(
        "{{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"coverage_report\":\"{}\"}}\n",
        fixture("lcov/basic.info")
    );
    std::fs::write(path, line).unwrap();
}

fn write_guard_context(path: &Path) {
    fs::write(
        path,
        r#"
version: 1
policy:
  tools:
    allowlist: ["covy"]
  reducers:
    allowlist: ["merge"]
  paths:
    include: ["src/**"]
    exclude: ["src/private/**"]
  token_budget:
    cap: 200
  runtime_budget:
    cap_ms: 1000
  redaction:
    forbidden_patterns: ["(?i)password"]
"#,
    )
    .unwrap();
}

#[test]
fn test_top_level_rewrite_plans_supported_command() {
    let root = TempDir::new().unwrap();
    suite_cmd()
        .current_dir(root.path())
        .args([
            "rewrite",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "git",
            "status",
            "--short",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"route\":\"reducer_rewrite\""))
        .stdout(predicate::str::contains("\"reducer_family\":\"git\""));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "rewrite",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "gradle",
            "test",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"route\":\"reducer_rewrite\""))
        .stdout(predicate::str::contains("\"reducer_family\":\"jvm\""));
}

#[test]
fn test_top_level_rewrite_respects_repo_exclude_config() {
    let root = TempDir::new().unwrap();
    fs::write(
        root.path().join("covy.toml"),
        "[packet28.rewrite]\nexclude_commands = [\"git\"]\n",
    )
    .unwrap();
    suite_cmd()
        .current_dir(root.path())
        .args([
            "rewrite",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "git",
            "status",
            "--short",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"route\":\"raw_passthrough\""))
        .stdout(predicate::str::contains("\"reason\":\"config_excluded\""));
}

#[test]
fn test_system_json_deps_and_env_commands() {
    let root = TempDir::new().unwrap();
    let payload = root.path().join("payload.json");
    fs::write(
        &payload,
        serde_json::to_string(&json!({
            "name": "demo",
            "items": [1, 2, 3, 4, 5, 6],
            "long": "x".repeat(120)
        }))
        .unwrap(),
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args(["json", payload.to_str().unwrap(), "--schema-only"])
        .assert()
        .success()
        .stdout(predicate::str::contains("name: string"))
        .stdout(predicate::str::contains("items:"))
        .stdout(predicate::str::contains("[int] (6)"))
        .stdout(predicate::str::contains("long: string"));

    fs::write(
        root.path().join("package.json"),
        r#"{"name":"packet28-demo","version":"1.0.0","dependencies":{"react":"18.2.0"},"devDependencies":{"vite":"5.0.0"}}"#,
    )
    .unwrap();
    fs::write(
        root.path().join("requirements.txt"),
        "pytest==8.0.0\n# comment\nruff>=0.4\n",
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args(["deps", root.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Node.js (package.json):"))
        .stdout(predicate::str::contains("packet28-demo @ 1.0.0"))
        .stdout(predicate::str::contains("react (18.2.0)"))
        .stdout(predicate::str::contains("Python (requirements.txt):"))
        .stdout(predicate::str::contains("pytest==8.0.0"));

    suite_cmd()
        .current_dir(root.path())
        .env_clear()
        .env("PATH", "/a:/b:/c:/d:/e:/f")
        .env("PACKET28_SECRET_TOKEN", "supersecrettoken")
        .args(["env", "packet28"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PACKET28_SECRET_TOKEN=su****en"))
        .stdout(predicate::str::contains("supersecrettoken").not());
}

#[test]
fn test_system_read_command_filters_and_numbers_files() {
    let root = TempDir::new().unwrap();
    let source = root.path().join("main.rs");
    fs::write(
        &source,
        "// module comment\nfn main() {\n    println!(\"hello\");\n}\n",
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "read",
            source.to_str().unwrap(),
            "--level",
            "minimal",
            "--max-lines",
            "2",
            "--line-numbers",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("module comment").not())
        .stdout(predicate::str::contains("1 | fn main() {"))
        .stdout(predicate::str::contains("2 |     println!(\"hello\");"))
        .stdout(predicate::str::contains("more lines"));
}

#[test]
fn test_system_summary_command_preserves_exit_and_summarizes_output() {
    let root = TempDir::new().unwrap();
    suite_cmd()
        .current_dir(root.path())
        .args([
            "summary",
            "sh",
            "-c",
            "printf 'test result: FAILED. 1 passed; 2 failed; 3 ignored\\n'; exit 7",
        ])
        .assert()
        .code(7)
        .stdout(predicate::str::contains("[FAIL] Command:"))
        .stdout(predicate::str::contains("[ok] 1 passed"))
        .stdout(predicate::str::contains("[FAIL] 2 failed"))
        .stdout(predicate::str::contains("skip 3 skipped"));
}

#[test]
fn test_system_err_command_preserves_exit_and_summarizes_failure() {
    let root = TempDir::new().unwrap();
    suite_cmd()
        .current_dir(root.path())
        .args([
            "err",
            "sh",
            "-c",
            "printf 'fatal: broken build\\n' >&2; exit 42",
        ])
        .assert()
        .code(42)
        .stdout(predicate::str::contains("[FAIL] Command:"))
        .stdout(predicate::str::contains("fatal: broken build"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "err",
            "--json",
            "sh",
            "-c",
            "printf 'fatal: broken build\\n' >&2; exit 42",
        ])
        .assert()
        .code(42)
        .stdout(predicate::str::contains("\"command\":\"Packet28 err\""))
        .stdout(predicate::str::contains("fatal: broken build"));
}

#[test]
fn test_system_smart_command_summarizes_source_file() {
    let root = TempDir::new().unwrap();
    let source = root.path().join("lib.rs");
    fs::write(
        &source,
        r#"
use anyhow::Result;

#[derive(Debug)]
pub struct Config {
    name: String,
}

pub fn load_config() -> Result<Config> {
    Ok(Config { name: "demo".to_string() })
}
"#,
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args(["smart", source.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust module"))
        .stdout(predicate::str::contains("1 fn"))
        .stdout(predicate::str::contains("1 type"))
        .stdout(predicate::str::contains("derive"));

    suite_cmd()
        .current_dir(root.path())
        .args(["smart", source.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"command\":\"Packet28 smart\""))
        .stdout(predicate::str::contains("Rust module"));
}

#[test]
fn test_system_find_command_supports_native_find_shape() {
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src").join("a.rs"), "").unwrap();
    fs::write(root.path().join("src").join("b.rs"), "").unwrap();
    fs::write(root.path().join("src").join("note.txt"), "").unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args(["find", ".", "-name", "*.rs", "-type", "f", "-maxdepth", "2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 match(es) under . for *.rs"))
        .stdout(predicate::str::contains("src/a.rs"))
        .stdout(predicate::str::contains("src/b.rs"))
        .stdout(predicate::str::contains("note.txt").not());
}

#[test]
fn test_system_grep_command_groups_matches() {
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("src").join("a.rs"),
        "fn alpha() {}\nfn beta() {}\n",
    )
    .unwrap();
    fs::write(root.path().join("src").join("note.txt"), "fn ignored\n").unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args(["grep", "fn", "src", "--file-type", "rs", "--max", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 matches in 1 files"))
        .stdout(predicate::str::contains("src/a.rs:1:fn alpha() {}"))
        .stdout(predicate::str::contains("[+1 more]"))
        .stdout(predicate::str::contains("ignored").not());
}

#[cfg(unix)]
#[test]
fn test_system_grep_fff_engine_delegates_to_p28() {
    let root = TempDir::new().unwrap();
    let fake_p28 = root.path().join("p28");
    fs::write(
        &fake_p28,
        r#"#!/usr/bin/env sh
printf '%s\n' '{"result":{"query":"Alpha","match_count":2,"returned_match_count":2,"paths":["src/a.rs"],"groups":[{"path":"src/a.rs","match_count":2,"displayed_match_count":2,"matches":[{"path":"src/a.rs","line":1,"text":"Alpha one"},{"path":"src/a.rs","line":2,"text":"Alpha two"}]}],"engine":{"engine":"fff_mcp"}}}'
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&fake_p28).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_p28, perms).unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("P28_SEARCH_BIN", &fake_p28)
        .args(["grep", "Alpha", "--engine", "fff", "--max", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 matches in 1 files"))
        .stdout(predicate::str::contains("src/a.rs:1:Alpha one"))
        .stdout(predicate::str::contains("[+1 more]"));
}

#[test]
fn test_system_log_command_deduplicates_noisy_lines() {
    let root = TempDir::new().unwrap();
    let log = root.path().join("app.log");
    fs::write(
        &log,
        "2026-05-12T01:00:00 ERROR failed request id=1001 path=/tmp/a\n\
         2026-05-12T01:00:01 ERROR failed request id=1002 path=/tmp/b\n\
         2026-05-12T01:00:02 WARN retrying request id=2001\n\
         2026-05-12T01:00:03 INFO healthy\n",
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args(["log", log.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Log Summary"))
        .stdout(predicate::str::contains("[error] 2 errors (1 unique)"))
        .stdout(predicate::str::contains("[warn] 1 warnings (1 unique)"))
        .stdout(predicate::str::contains("[info] 1 info messages"))
        .stdout(predicate::str::contains("[x2]"));
}

#[test]
fn test_run_reduces_git_status() {
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
        .stdout(predicate::str::contains("run_reducer:git"));

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
        .stdout(predicate::str::contains("route,count,share_pct,bar"))
        .stdout(predicate::str::contains("run_reducer:git"));

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "--graph"])
        .assert()
        .success()
        .stdout(predicate::str::contains("route,count,share_pct,bar"))
        .stdout(predicate::str::contains("run_reducer:git"));

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "-g"])
        .assert()
        .success()
        .stdout(predicate::str::contains("route,count,share_pct,bar"))
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
    suite_cmd()
        .current_dir(root.path())
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
        .stdout(predicate::str::contains("\"fallback_reason\""));

    suite_cmd()
        .current_dir(root.path())
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
            "timestamp_unix_ms,family,exit_code,fallback_reason,command",
        ))
        .stdout(predicate::str::contains("fallback,7"))
        .stdout(predicate::str::contains("unsupported"))
        .stdout(predicate::str::contains("packet28 failure"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "gain",
            "--root",
            root.path().to_str().unwrap(),
            "--failures",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "timestamp_unix_ms,family,exit_code,fallback_reason,command",
        ))
        .stdout(predicate::str::contains("fallback,7"))
        .stdout(predicate::str::contains("unsupported"))
        .stdout(predicate::str::contains("packet28 failure"));

    suite_cmd()
        .current_dir(root.path())
        .args(["gain", "--root", root.path().to_str().unwrap(), "-F"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "timestamp_unix_ms,family,exit_code,fallback_reason,command",
        ))
        .stdout(predicate::str::contains("fallback,7"))
        .stdout(predicate::str::contains("unsupported"))
        .stdout(predicate::str::contains("packet28 failure"));
}

#[test]
fn test_cc_economics_merges_ccusage_and_packet28_savings() {
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

#[test]
fn test_run_reduced_command_exposes_fetchable_raw_artifact() {
    let root = TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(root.path())
        .status()
        .unwrap();
    fs::write(root.path().join("raw-visible.txt"), "changed\n").unwrap();

    let output = suite_cmd()
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
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let handle = value["raw_artifact"]["handle"].as_str().unwrap();
    assert!(value["raw_artifact"]["available"].as_bool().unwrap());

    suite_cmd()
        .current_dir(root.path())
        .args([
            "compact",
            "fetch-raw",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            "run-raw",
            "--handle",
            handle,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("raw-visible.txt"))
        .stdout(predicate::str::contains("--- stdout ---"));
}

#[test]
fn test_run_fallback_command_exposes_fetchable_raw_artifact() {
    let root = TempDir::new().unwrap();

    let output = suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "sh",
            "-c",
            "echo fallback-stdout; echo fallback-stderr >&2",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["fallback_reason"], "unsupported");
    assert_eq!(value["command"]["exit_code"], 0);
    assert!(value["raw_artifact"]["available"].as_bool().unwrap());
    let handle = value["raw_artifact"]["handle"].as_str().unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "compact",
            "fetch-raw",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            "run-raw",
            "--handle",
            handle,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("fallback-stdout"))
        .stdout(predicate::str::contains("fallback-stderr"));
}

#[test]
fn test_run_applies_project_toml_filter_to_fallback_command() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(root.path().join(".packet28")).unwrap();
    fs::write(
        root.path().join(".packet28").join("filters.toml"),
        r#"
schema_version = 1

[filters.demo]
match_command = "^sh\\s+-c"
strip_lines_matching = ["^debug:"]
keep_lines_matching = []
truncate_lines_at = 80
filter_stderr = true

[[filters.demo.replace]]
pattern = "TOKEN=[A-Za-z0-9]+"
replacement = "TOKEN=<redacted>"

[[tests.demo]]
name = "redacts and strips noise"
input = """
debug: noisy
value TOKEN=abcdef
"""
expected = "value TOKEN=<redacted>"
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
            "--require-all",
            "--trust",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Trusted filter config"));

    let output = suite_cmd()
        .current_dir(root.path())
        .env("HOME", &home)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "sh",
            "-c",
            "printf 'debug: noisy\\nvalue TOKEN=abcdef\\n'; printf 'stderr TOKEN=secret\\n' >&2",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["fallback_reason"], Value::Null);
    assert_eq!(value["reduction"]["family"], "custom_filter");
    assert_eq!(value["reduction"]["canonical_kind"], "demo");
    let preview = value["reduction"]["compact_preview"].as_str().unwrap();
    assert!(preview.contains("value TOKEN=<redacted>"));
    assert!(preview.contains("stderr TOKEN=<redacted>"));
    assert!(!preview.contains("debug: noisy"));
    assert!(value["raw_artifact"]["available"].as_bool().unwrap());
    let handle = value["raw_artifact"]["handle"].as_str().unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "compact",
            "fetch-raw",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            "run-raw",
            "--handle",
            handle,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("TOKEN=abcdef"))
        .stdout(predicate::str::contains("TOKEN=secret"));
}

#[test]
fn test_run_skips_untrusted_project_toml_filter() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(root.path().join(".packet28")).unwrap();
    fs::write(
        root.path().join(".packet28").join("filters.toml"),
        r#"
schema_version = 1

[filters.demo]
match_command = "^sh\\s+-c"
keep_lines_matching = ["safe"]
"#,
    )
    .unwrap();

    let output = suite_cmd()
        .current_dir(root.path())
        .env("HOME", &home)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "sh",
            "-c",
            "printf 'safe\\nnoise\\n'",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["fallback_reason"], "unsupported");
    assert_eq!(value["stdout"], "safe\nnoise\n");
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
fn test_run_failing_reduced_command_preserves_exit_and_raw_stderr() {
    let root = TempDir::new().unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"packet28-broken-run-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/lib.rs"), "pub fn broken( {}\n").unwrap();

    let output = suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "cargo",
            "check",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(101));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["fallback_reason"], Value::Null);
    assert_eq!(value["command"]["exit_code"], 101);
    assert_eq!(value["reduction"]["exit_code"], 101);
    assert!(value["raw_artifact"]["available"].as_bool().unwrap());
    let handle = value["raw_artifact"]["handle"].as_str().unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "compact",
            "fetch-raw",
            "--root",
            root.path().to_str().unwrap(),
            "--task-id",
            "run-raw",
            "--handle",
            handle,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("exit_code: 101"))
        .stdout(predicate::str::contains("error:"))
        .stdout(predicate::str::contains("cargo check"));
}

#[test]
#[cfg(unix)]
fn test_run_raw_artifact_available_across_reducer_families() {
    let root = TempDir::new().unwrap();
    init_repo(root.path());
    fs::write(root.path().join("raw-visible.txt"), "fs raw marker\n").unwrap();
    fs::write(root.path().join("git-visible.txt"), "changed\n").unwrap();

    let bin_dir = TempDir::new().unwrap();
    write_executable_script(
        &bin_dir.path().join("cargo"),
        "#!/bin/sh\nprintf 'rust raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("npx"),
        "#!/bin/sh\nprintf 'javascript raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("python3"),
        "#!/bin/sh\nprintf 'python raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("go"),
        "#!/bin/sh\nprintf 'ok\\tpacket28.test\\t0.01s\\ngo raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("docker"),
        "#!/bin/sh\nprintf 'infra raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("gh"),
        "#!/bin/sh\nprintf 'build\\tpass\\t1s\\ngithub raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("gt"),
        "#!/bin/sh\nprintf 'Pushed branch feat/add-auth\\nCreated pull request #42 for feat/add-auth\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("ruby"),
        "#!/bin/sh\nprintf '1 runs, 1 assertions, 0 failures, 0 errors, 0 skips\\nruby raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("dotnet"),
        "#!/bin/sh\nprintf 'Passed!  - Failed: 0, Passed: 1, Skipped: 0, Total: 1, Duration: 1 s\\ndotnet raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("gradle"),
        "#!/bin/sh\nprintf 'ExampleTest > fails FAILED\\n    java.lang.AssertionError: expected true\\n        at org.junit.Assert.fail(Assert.java:89)\\n        at com.example.ExampleTest.fails(ExampleTest.java:42)\\n2 tests completed, 1 failed\\nBUILD FAILED in 1s\\ngradle raw marker\\n'\n",
    );
    let path_env = format!(
        "{}:{}",
        bin_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let cases: Vec<(&str, Vec<&str>, &str)> = vec![
        ("git", vec!["git", "status", "--short"], "git-visible.txt"),
        ("git", vec!["gt", "submit"], "Created pull request"),
        ("fs", vec!["cat", "raw-visible.txt"], "fs raw marker"),
        ("rust", vec!["cargo", "check"], "rust raw marker"),
        (
            "javascript",
            vec!["npx", "tsc", "--noEmit"],
            "javascript raw marker",
        ),
        (
            "python",
            vec!["python3", "-m", "pytest", "tests"],
            "python raw marker",
        ),
        ("go", vec!["go", "test", "./..."], "go raw marker"),
        ("infra", vec!["docker", "logs", "demo"], "infra raw marker"),
        (
            "github",
            vec!["gh", "pr", "checks", "1"],
            "github raw marker",
        ),
        ("ruby", vec!["ruby", "sample_test.rb"], "ruby raw marker"),
        (
            "dotnet",
            vec!["dotnet", "test", "Packet28.Tests.csproj"],
            "dotnet raw marker",
        ),
        ("jvm", vec!["gradle", "test"], "gradle raw marker"),
    ];

    for (family, argv, raw_marker) in cases {
        let mut command = suite_cmd();
        command
            .current_dir(root.path())
            .env("PATH", &path_env)
            .args(["run", "--root", root.path().to_str().unwrap(), "--json"])
            .args(&argv);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{family} reducer command failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["fallback_reason"], Value::Null, "{family}");
        assert_eq!(value["reduction"]["family"], family, "{family}");
        assert!(
            value["raw_artifact"]["available"].as_bool().unwrap(),
            "{family}"
        );
        let handle = value["raw_artifact"]["handle"].as_str().unwrap();

        suite_cmd()
            .current_dir(root.path())
            .args([
                "compact",
                "fetch-raw",
                "--root",
                root.path().to_str().unwrap(),
                "--task-id",
                "run-raw",
                "--handle",
                handle,
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains(raw_marker));
    }
}

#[test]
fn test_run_reduces_cargo_check() {
    let root = TempDir::new().unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"p28-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("src/lib.rs"),
        "pub fn ok() -> bool { true }\n",
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "cargo",
            "check",
            "--quiet",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"rust\""))
        .stdout(predicate::str::contains(
            "\"canonical_kind\":\"rust_check\"",
        ))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));
}

#[cfg(unix)]
fn write_executable_script(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
#[cfg(unix)]
fn test_run_reduces_tree_command() {
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("src/bin")).unwrap();
    fs::write(root.path().join("src/lib.rs"), "pub fn ok() {}\n").unwrap();
    fs::write(root.path().join("src/bin/cli.rs"), "fn main() {}\n").unwrap();

    let bin_dir = TempDir::new().unwrap();
    write_executable_script(
        &bin_dir.path().join("tree"),
        "#!/bin/sh\nprintf 'src\\n├── lib.rs\\n└── bin\\n    └── cli.rs\\n\\n1 directory, 2 files\\n'\n",
    );
    let path_env = format!(
        "{}:{}",
        bin_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "tree",
            "-L",
            "2",
            "src",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"fs\""))
        .stdout(predicate::str::contains("\"canonical_kind\":\"fs_tree\""))
        .stdout(predicate::str::contains(
            "tree listed 1 dir(s), 2 file(s) under src",
        ))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));
}

#[test]
#[cfg(unix)]
fn test_run_reduces_npm_test_and_pytest() {
    let root = TempDir::new().unwrap();
    let bin_dir = TempDir::new().unwrap();
    write_executable_script(
        &bin_dir.path().join("npm"),
        "#!/bin/sh\nprintf 'npm test fixture passed\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("pytest"),
        "#!/bin/sh\nprintf '2 passed in 0.01s\\n'\n",
    );
    let path_env = format!(
        "{}:{}",
        bin_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "npm",
            "test",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"javascript\""))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "pytest",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"python\""))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));
}

#[test]
fn test_run_reduces_file_and_search_commands() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("sample.txt"), "alpha\nbeta\nalpha again\n").unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "cat",
            "sample.txt",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"fs\""))
        .stdout(predicate::str::contains("\"canonical_kind\":\"fs_cat\""))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "grep",
            "alpha",
            "sample.txt",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"fs\""))
        .stdout(predicate::str::contains("\"canonical_kind\":\"fs_grep\""))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "wc",
            "-l",
            "sample.txt",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"fs\""))
        .stdout(predicate::str::contains("\"canonical_kind\":\"fs_wc\""))
        .stdout(predicate::str::contains("\"summary\":\"wc 3\""))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));
}

#[test]
#[cfg(unix)]
fn test_run_reduces_docker_logs_and_gh_pr_checks() {
    let root = TempDir::new().unwrap();
    let bin_dir = TempDir::new().unwrap();
    write_executable_script(
        &bin_dir.path().join("docker"),
        "#!/bin/sh\nprintf 'service started\\nservice ready\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("gh"),
        "#!/bin/sh\nprintf 'build\\tpass\\t12s\\ntest\\tfail\\t8s\\n'\nexit 1\n",
    );
    write_executable_script(
        &bin_dir.path().join("glab"),
        "#!/bin/sh\nprintf '42\\tFix reducer\\tmain\\topened\\n43\\tUpdate docs\\tmain\\topened\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("psql"),
        "#!/bin/sh\nprintf ' id | name \\n----+------\\n  1 | Ada\\n  2 | Grace\\n(2 rows)\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("aws"),
        "#!/bin/sh\nprintf '{\"Functions\":[{\"FunctionName\":\"api\",\"Runtime\":\"nodejs20.x\"},{\"FunctionName\":\"worker\",\"Runtime\":\"python3.12\"}]}'\n",
    );
    write_executable_script(
        &bin_dir.path().join("wget"),
        "#!/bin/sh\nprintf '%s\n' '--2026-05-12-- https://example.com/pkg.tgz' \"Saving to: 'pkg.tgz'\" \"'pkg.tgz' saved [2048/2048]\" >&2\n",
    );
    let path_env = format!(
        "{}:{}",
        bin_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "docker",
            "logs",
            "demo",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"infra\""))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "gh",
            "pr",
            "checks",
            "1",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"family\":\"github\""))
        .stdout(predicate::str::contains("\"fallback_reason\":null"))
        .stdout(predicate::str::contains("\"failed\":true"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "glab",
            "mr",
            "list",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"github\""))
        .stdout(predicate::str::contains(
            "\"canonical_kind\":\"glab_mr_list\"",
        ))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "psql",
            "-c",
            "select id, name from users",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"infra\""))
        .stdout(predicate::str::contains(
            "\"canonical_kind\":\"psql_query\"",
        ))
        .stdout(predicate::str::contains("psql returned 2 row(s)"))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "aws",
            "lambda",
            "list-functions",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"infra\""))
        .stdout(predicate::str::contains(
            "\"canonical_kind\":\"aws_lambda_list_functions\"",
        ))
        .stdout(predicate::str::contains(
            "aws lambda listed 2 function(s); first api nodejs20.x",
        ))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "wget",
            "https://example.com/pkg.tgz",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"infra\""))
        .stdout(predicate::str::contains(
            "\"canonical_kind\":\"wget_fetch\"",
        ))
        .stdout(predicate::str::contains(
            "wget example.com/pkg.tgz ok | pkg.tgz | 2.0KB",
        ))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));
}

#[test]
#[cfg(unix)]
fn test_run_reduces_ruby_and_dotnet_commands() {
    let root = TempDir::new().unwrap();
    let bin_dir = TempDir::new().unwrap();
    write_executable_script(
        &bin_dir.path().join("bundle"),
        "#!/bin/sh\nprintf 'Failures:\\n  1) User validates email\\n     spec/models/user_spec.rb:12\\n\\n3 examples, 1 failure\\n'\nexit 1\n",
    );
    write_executable_script(
        &bin_dir.path().join("rake"),
        "#!/bin/sh\nprintf 'Run options: --seed 1\\n\\n# Running:\\n\\n.F\\n\\nFinished in 0.1s\\n\\n  1) Failure:\\nUserTest#test_email [test/user_test.rb:12]:\\nExpected: true\\n  Actual: false\\n\\n2 runs, 2 assertions, 1 failures, 0 errors, 0 skips\\n'\nexit 1\n",
    );
    write_executable_script(
        &bin_dir.path().join("dotnet"),
        "#!/bin/sh\nprintf 'Passed!  - Failed: 0, Passed: 12, Skipped: 0, Total: 12, Duration: 1 s\\n'\n",
    );
    let path_env = format!(
        "{}:{}",
        bin_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "bundle",
            "exec",
            "rspec",
            "spec/models/user_spec.rb",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"family\":\"ruby\""))
        .stdout(predicate::str::contains(
            "\"canonical_kind\":\"ruby_rspec\"",
        ))
        .stdout(predicate::str::contains("rspec: 3 examples, 1 failure"))
        .stdout(predicate::str::contains("\"fallback_reason\":null"))
        .stdout(predicate::str::contains("\"failed\":true"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "rake",
            "test",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"family\":\"ruby\""))
        .stdout(predicate::str::contains(
            "\"canonical_kind\":\"ruby_rake_test\"",
        ))
        .stdout(predicate::str::contains(
            "rake test: 2 runs, 2 assertions, 1 failures",
        ))
        .stdout(predicate::str::contains("UserTest#test_email"))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "dotnet",
            "test",
            "Packet28.Tests.csproj",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"family\":\"dotnet\""))
        .stdout(predicate::str::contains(
            "\"canonical_kind\":\"dotnet_test\"",
        ))
        .stdout(predicate::str::contains(
            "dotnet test: Passed!  - Failed: 0, Passed: 12",
        ))
        .stdout(predicate::str::contains("\"fallback_reason\":null"));
}

#[test]
fn test_memory_store_recall_uses_sqlite_home_db() {
    let home = TempDir::new().unwrap();
    let db_path = home.path().join(".packet28").join("packet28.db");
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 remembers local context",
            "--tags",
            "packet28,local",
            "--topic",
            "parity",
            "--importance",
            "high",
            "--keywords",
            "context,local",
            "--project",
            "coverage-a",
            "--source",
            "cli-test",
            "--raw",
            "verbatim context",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"content\""))
        .stdout(predicate::str::contains("\"topic\":\"parity\""))
        .stdout(predicate::str::contains("\"importance\":\"high\""))
        .stdout(predicate::str::contains("\"project\":\"coverage-a\""))
        .stdout(predicate::str::contains("\"source\":\"cli-test\""));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "invalid importance should fail",
            "--importance",
            "urgent",
            "--json",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unsupported memory importance"));

    assert!(db_path.exists());
    let conn = Connection::open(&db_path).unwrap();
    let fts_tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('memories_fts', 'feedback_fts')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fts_tables, 2);

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "recall", "local context", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"recall_score\""))
        .stdout(predicate::str::contains("Packet28 remembers local context"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "recall", "local context", "--format", "toon"])
        .assert()
        .success()
        .stdout(predicate::str::contains("memories[1]{score,id,topic"))
        .stdout(predicate::str::contains("Packet28 remembers local context"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "recall", "local context", "--format", "detail"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[score:"))
        .stdout(predicate::str::contains("topic:"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "wakeup",
            "--query",
            "local",
            "--project",
            "coverage-a",
            "--max-tokens",
            "40",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\":\"packet28.wakeup.v1\""))
        .stdout(predicate::str::contains("\"project\":\"coverage-a\""))
        .stdout(predicate::str::contains("\"max_tokens\":40"))
        .stdout(predicate::str::contains("\"pack\""))
        .stdout(predicate::str::contains("\"included_items\""))
        .stdout(predicate::str::contains("Packet28 remembers local context"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "update",
            "1",
            "--content",
            "Packet28 remembers updated local context",
            "--topic",
            "updated-parity",
            "--project",
            "coverage-b",
            "--importance",
            "CRITICAL",
            "--source",
            "cli-update",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("updated local context"))
        .stdout(predicate::str::contains("\"topic\":\"updated-parity\""))
        .stdout(predicate::str::contains("\"importance\":\"critical\""))
        .stdout(predicate::str::contains("\"project\":\"coverage-b\""))
        .stdout(predicate::str::contains("\"source\":\"cli-update\""));

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "topics", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"topic\":\"updated-parity\""))
        .stdout(predicate::str::contains("\"memory_count\":1"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 remembers a second local context",
            "--topic",
            "updated-parity",
            "--keywords",
            "second,context",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Foreign project context",
            "--topic",
            "foreign-parity",
            "--project",
            "coverage-foreign",
            "--json",
        ])
        .assert()
        .success();

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "context",
            "--topic",
            "updated-parity",
            "--project",
            "coverage-b",
            "--tag",
            "packet28",
            "--keyword",
            "context",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("updated local context"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "Foreign",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::eq("[]\n"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "wakeup",
            "--query",
            "Foreign",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"memories\":[]"))
        .stdout(predicate::str::contains(
            "no Packet28 wake-up context matched",
        ));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "forget", "3", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"deleted\":1"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "list",
            "--topic",
            "updated-parity",
            "--project",
            "coverage-b",
            "--sort",
            "oldest",
            "--all",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Packet28 remembers updated local context",
        ))
        .stdout(predicate::str::contains(
            "Packet28 remembers a second local context",
        ));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "consolidate",
            "--topic",
            "updated-parity",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\":\"consolidated\""))
        .stdout(predicate::str::contains("\"source_count\":2"))
        .stdout(predicate::str::contains("Consolidated memory for topic"));

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"memory_count\":1"));

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "embed", "--all", "--dimensions", "16", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"model\":\"packet28-local-lexical-v2\"",
        ))
        .stdout(predicate::str::contains("\"dimensions\":16"))
        .stdout(predicate::str::contains("\"embedded_count\":1"));
    let embedding_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_embeddings", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(embedding_rows, 1);
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "updated second",
            "--project",
            "coverage-b",
            "--format",
            "toon",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("memories[1]{score,id,topic"))
        .stdout(predicate::str::contains("Consolidated memory for topic"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "updted secnd",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Consolidated memory for topic"))
        .stdout(predicate::str::contains("\"recall_score\""));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "health",
            "--topic",
            "updated-parity",
            "--consolidation-threshold",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"topic_filter\":\"updated-parity\"",
        ))
        .stdout(predicate::str::contains("\"total_memories\":1"))
        .stdout(predicate::str::contains(
            "\"topics_needing_consolidation\":1",
        ))
        .stdout(predicate::str::contains("\"avg_weight\""))
        .stdout(predicate::str::contains("\"avg_access_count\""))
        .stdout(predicate::str::contains("\"consolidation_needed\":true"));

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "forget", "--topic", "updated-parity", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"deleted\":1"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 can prune low-weight local context",
            "--topic",
            "prune-test",
            "--importance",
            "low",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 preserves high-importance local context during prune",
            "--topic",
            "prune-test",
            "--importance",
            "high",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "decay", "--factor", "0.1", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"decayed_count\":2"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "prune",
            "--threshold",
            "0.6",
            "--dry-run",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"candidate_count\":1"))
        .stdout(predicate::str::contains("\"deleted_count\":0"))
        .stdout(predicate::str::contains("\"skipped_protected_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "prune", "--threshold", "0.6", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"candidate_count\":1"))
        .stdout(predicate::str::contains("\"deleted_count\":1"))
        .stdout(predicate::str::contains("\"skipped_protected_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "recall", "high-importance", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("preserves high-importance"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Access-aware decay keeps frequently recalled context",
            "--topic",
            "access-decay",
            "--importance",
            "medium",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Dormant decay comparison note",
            "--topic",
            "access-decay",
            "--importance",
            "medium",
            "--json",
        ])
        .assert()
        .success();
    for _ in 0..5 {
        suite_cmd()
            .env("HOME", home.path())
            .args([
                "memory",
                "recall",
                "frequently recalled context",
                "--topic",
                "access-decay",
                "--json",
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("frequently recalled context"));
    }
    let accessed_count: i64 = conn
        .query_row(
            "SELECT access_count FROM memories WHERE content LIKE 'Access-aware decay%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(accessed_count, 5);
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "decay", "--factor", "0.5", "--json"])
        .assert()
        .success();
    let accessed_weight: f64 = conn
        .query_row(
            "SELECT weight FROM memories WHERE content LIKE 'Access-aware decay%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let unaccessed_weight: f64 = conn
        .query_row(
            "SELECT weight FROM memories WHERE content LIKE 'Dormant decay%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        accessed_weight > unaccessed_weight,
        "accessed weight {accessed_weight} should exceed unaccessed weight {unaccessed_weight}"
    );
}

#[test]
fn test_memory_pending_extraction_queue_processes_into_memory() {
    let home = TempDir::new().unwrap();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "pending",
            "enqueue",
            "- Packet28 pending extraction stores durable local facts",
            "--project",
            "coverage-a",
            "--tool-name",
            "Bash",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"project\":\"coverage-a\""))
        .stdout(predicate::str::contains("\"tool_name\":\"Bash\""));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "pending", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("durable local facts"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "pending", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pending_extraction_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "pending", "process", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pending_count\":1"))
        .stdout(predicate::str::contains("\"extracted_count\":1"))
        .stdout(predicate::str::contains("\"deleted_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "pending", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pending_extraction_count\":0"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "durable local facts",
            "--project",
            "coverage-a",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "pending extraction stores durable",
        ))
        .stdout(predicate::str::contains("pending-extraction:Bash"));
}

#[test]
fn test_memory_store_migrates_legacy_sqlite_schema() {
    let home = TempDir::new().unwrap();
    let packet28_dir = home.path().join(".packet28");
    fs::create_dir_all(&packet28_dir).unwrap();
    let db_path = packet28_dir.join("packet28.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE memories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content TEXT NOT NULL,
            tags TEXT,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE feedback (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            subject TEXT NOT NULL,
            correction TEXT NOT NULL,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE concepts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            description TEXT,
            created_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE transcript_sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_key TEXT NOT NULL UNIQUE,
            agent TEXT,
            started_at_unix_ms INTEGER NOT NULL,
            updated_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE transcript_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id INTEGER NOT NULL,
            role TEXT NOT NULL DEFAULT 'assistant',
            content TEXT NOT NULL,
            source TEXT,
            created_at_unix_ms INTEGER NOT NULL
        );
        INSERT INTO memories (content, tags, created_at_unix_ms)
            VALUES ('legacy Packet28 durable context', 'legacy', 1700000000000);
        INSERT INTO feedback (subject, correction, created_at_unix_ms)
            VALUES ('legacy feedback subject', 'legacy correction body', 1700000000001);
        INSERT INTO concepts (name, description, created_at_unix_ms)
            VALUES ('LegacyConcept', 'legacy graph description', 1700000000002);
        INSERT INTO transcript_sessions (session_key, agent, started_at_unix_ms, updated_at_unix_ms)
            VALUES ('legacy-session', 'codex', 1700000000003, 1700000000003);
        INSERT INTO transcript_messages (session_id, role, content, source, created_at_unix_ms)
            VALUES (1, 'user', 'legacy transcript context', 'legacy-test', 1700000000004);
        ",
    )
    .unwrap();
    drop(conn);

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"memory_count\":1"));

    let conn = Connection::open(&db_path).unwrap();
    assert_table_columns(
        &conn,
        "memories",
        &[
            "topic",
            "importance",
            "keywords",
            "project",
            "source",
            "raw_excerpt",
            "weight",
            "access_count",
            "last_accessed_unix_ms",
            "updated_at_unix_ms",
        ],
    );
    assert_table_columns(
        &conn,
        "feedback",
        &[
            "topic",
            "context",
            "predicted",
            "reason",
            "source",
            "project",
            "applied_count",
        ],
    );
    assert_table_columns(&conn, "transcript_messages", &["source", "project"]);
    assert_table_columns(
        &conn,
        "concepts",
        &[
            "memoir_name",
            "labels",
            "confidence",
            "revision",
            "source_ids",
            "updated_at_unix_ms",
        ],
    );

    let fts_tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='table'
             AND name IN (
                'memories_fts',
                'feedback_fts',
                'feedback_fts_all',
                'concepts_fts',
                'transcript_messages_fts'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fts_tables, 5);
    let trigger_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='trigger'
             AND name IN (
                'memories_ai',
                'memories_ad',
                'memories_au',
                'feedback_all_ai',
                'feedback_all_ad',
                'feedback_all_au',
                'concepts_ai',
                'concepts_ad',
                'concepts_au',
                'transcript_messages_ai',
                'transcript_messages_ad',
                'transcript_messages_au'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trigger_count, 12);

    let migrated_memory: (String, String, f64, i64, i64) = conn
        .query_row(
            "SELECT topic, importance, weight, access_count, last_accessed_unix_ms
             FROM memories WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(migrated_memory.0, "general");
    assert_eq!(migrated_memory.1, "medium");
    assert_eq!(migrated_memory.2, 1.0);
    assert_eq!(migrated_memory.3, 0);
    assert_eq!(migrated_memory.4, 1700000000000);

    assert_eq!(fts_row_count(&conn, "memories_fts"), 1);
    assert_eq!(fts_row_count(&conn, "feedback_fts"), 1);
    assert_eq!(fts_row_count(&conn, "feedback_fts_all"), 1);
    assert_eq!(fts_row_count(&conn, "concepts_fts"), 1);
    assert_eq!(fts_row_count(&conn, "transcript_messages_fts"), 1);

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "recall", "legacy durable", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("legacy Packet28 durable context"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "search", "legacy correction", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("legacy correction body"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "search", "LegacyConcept", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("LegacyConcept"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "search", "legacy transcript", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("legacy transcript context"));
}

fn assert_table_columns(conn: &Connection, table: &str, expected: &[&str]) {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(|row| row.unwrap())
        .collect::<Vec<_>>();
    for column in expected {
        assert!(
            columns.iter().any(|existing| existing == column),
            "expected column {table}.{column}; found {columns:?}"
        );
    }
}

fn fts_row_count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn test_memory_recall_scores_importance_and_keywords() {
    let home = TempDir::new().unwrap();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 ranking shared term high signal",
            "--topic",
            "scoring",
            "--importance",
            "high",
            "--keywords",
            "priority,ranking",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"weight\":0.9"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 ranking shared term low signal",
            "--topic",
            "scoring",
            "--importance",
            "low",
            "--keywords",
            "archive",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"weight\":0.5"));

    let output = suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "ranking shared term",
            "--topic",
            "scoring",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "recall failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Value = serde_json::from_slice(&output.stdout).unwrap();
    let records = records.as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["importance"], "high");
    assert_eq!(records[1]["importance"], "low");
    let high_score = records[0]["recall_score"].as_f64().unwrap();
    let low_score = records[1]["recall_score"].as_f64().unwrap();
    assert!(
        high_score > low_score,
        "high importance keyword score {high_score} should exceed low score {low_score}"
    );

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 fts calibration keeps the exact phrase together",
            "--topic",
            "fts-calibration",
            "--importance",
            "medium",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 fts calibration mentions exact and later mentions phrase",
            "--topic",
            "fts-calibration",
            "--importance",
            "medium",
            "--json",
        ])
        .assert()
        .success();
    let output = suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "exact phrase",
            "--topic",
            "fts-calibration",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fts recall failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Value = serde_json::from_slice(&output.stdout).unwrap();
    let records = records.as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records[0]["content"]
        .as_str()
        .unwrap()
        .contains("exact phrase together"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "update",
            "2",
            "--importance",
            "critical",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"importance\":\"critical\""))
        .stdout(predicate::str::contains("\"weight\":1.0"));
}

#[test]
fn test_memory_consolidate_preserves_metadata_and_deletes_sources() {
    let home = TempDir::new().unwrap();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Consolidation source one keeps parser context",
            "--tags",
            "parser,cli",
            "--topic",
            "consolidation-meta",
            "--importance",
            "low",
            "--keywords",
            "parser,context",
            "--project",
            "coverage-a",
            "--source",
            "source-one",
            "--raw",
            "raw parser context",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Consolidation source two keeps daemon context",
            "--tags",
            "daemon,cli",
            "--topic",
            "consolidation-meta",
            "--importance",
            "critical",
            "--keywords",
            "daemon,context",
            "--project",
            "coverage-b",
            "--source",
            "source-two",
            "--raw",
            "raw daemon context",
            "--json",
        ])
        .assert()
        .success();

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "extract-patterns",
            "--topic",
            "consolidation-meta",
            "--memoir",
            "ConsolidationPatterns",
            "--min-cluster-size",
            "2",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pattern_count\""))
        .stdout(predicate::str::contains("\"key\":\"context\""))
        .stdout(predicate::str::contains("\"created_concepts\""));

    let output = suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "consolidate",
            "--topic",
            "consolidation-meta",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "consolidate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "consolidated");
    assert_eq!(report["source_count"], 2);
    assert_eq!(report["consolidated_memory"]["importance"], "critical");
    assert_eq!(report["consolidated_memory"]["tags"], "daemon,cli,parser");
    assert_eq!(
        report["consolidated_memory"]["keywords"],
        "daemon,context,parser"
    );
    assert_eq!(
        report["consolidated_memory"]["project"],
        "coverage-b,coverage-a"
    );
    assert_eq!(
        report["consolidated_memory"]["source"],
        "source-two,source-one"
    );
    let raw_excerpt = report["consolidated_memory"]["raw_excerpt"]
        .as_str()
        .unwrap();
    assert!(raw_excerpt.contains("raw daemon context"));
    assert!(raw_excerpt.contains("raw parser context"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "list",
            "--topic",
            "consolidation-meta",
            "--all",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"content\":\"Consolidated memory",
        ))
        .stdout(predicate::str::contains("\"importance\":\"critical\""))
        .stdout(predicate::str::contains("\"memory_count\"").not());

    let conn = Connection::open(home.path().join(".packet28").join("packet28.db")).unwrap();
    let memory_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE topic = 'consolidation-meta'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let chunk_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(memory_count, 1);
    assert_eq!(chunk_count, 1);
}

#[test]
fn test_mcp_memory_store_recall_uses_sqlite_home_db() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"mcp-learn-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nserde = \"1\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/lib.rs"), "pub fn fixture() {}\n").unwrap();
    let mut child = mcp_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["mcp", "serve", "--root", root.path().to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    initialize_mcp_session(&mut stdin, &mut stdout);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_store",
                "arguments":{
                    "content":"MCP memory survives locally",
                    "tags":"mcp",
                    "topic":"mcp-topic",
                    "importance":"high",
                    "keywords":"survives,locally",
                    "project":"mcp-project-a",
                    "source":"mcp-test",
                    "raw_excerpt":"verbatim mcp memory"
                }
            }
        }),
    );
    let stored = read_mcp_message_for_id(&mut stdout, 2);
    assert_eq!(
        stored["result"]["structuredContent"]["content"].as_str(),
        Some("MCP memory survives locally")
    );
    assert_eq!(
        stored["result"]["structuredContent"]["topic"].as_str(),
        Some("mcp-topic")
    );
    assert_eq!(
        stored["result"]["structuredContent"]["source"].as_str(),
        Some("mcp-test")
    );
    assert_eq!(
        stored["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-a")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_recall",
                "arguments":{"query":"survives", "limit": 3}
            }
        }),
    );
    let recalled = read_mcp_message_for_id(&mut stdout, 3);
    assert_eq!(
        recalled["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP memory survives locally")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_list",
                "arguments":{"limit": 3}
            }
        }),
    );
    let listed = read_mcp_message_for_id(&mut stdout, 4);
    assert_eq!(
        listed["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP memory survives locally")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":41,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_update",
                "arguments":{"id":1, "content":"MCP memory updated locally", "topic":"mcp-updated", "project":"mcp-project-b", "source":"mcp-update"}
            }
        }),
    );
    let updated = read_mcp_message_for_id(&mut stdout, 41);
    assert_eq!(
        updated["result"]["structuredContent"]["content"].as_str(),
        Some("MCP memory updated locally")
    );
    assert_eq!(
        updated["result"]["structuredContent"]["topic"].as_str(),
        Some("mcp-updated")
    );
    assert_eq!(
        updated["result"]["structuredContent"]["source"].as_str(),
        Some("mcp-update")
    );
    assert_eq!(
        updated["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":42,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_topics",
                "arguments":{}
            }
        }),
    );
    let topics = read_mcp_message_for_id(&mut stdout, 42);
    assert_eq!(
        topics["result"]["structuredContent"][0]["topic"].as_str(),
        Some("mcp-updated")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":43,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_stats",
                "arguments":{}
            }
        }),
    );
    let memory_stats = read_mcp_message_for_id(&mut stdout, 43);
    assert_eq!(
        memory_stats["result"]["structuredContent"]["memory_count"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":66,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_recall",
                "arguments":{
                    "query":"updated",
                    "topic":"mcp-updated",
                    "project":"mcp-project-b",
                    "keyword":"survives",
                    "limit":3
                }
            }
        }),
    );
    let filtered_recall = read_mcp_message_for_id(&mut stdout, 66);
    assert_eq!(
        filtered_recall["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP memory updated locally")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":67,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_list",
                "arguments":{"topic":"mcp-updated", "project":"mcp-project-b", "all":true, "sort":"importance"}
            }
        }),
    );
    let filtered_list = read_mcp_message_for_id(&mut stdout, 67);
    assert_eq!(
        filtered_list["result"]["structuredContent"][0]["topic"].as_str(),
        Some("mcp-updated")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":65,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_embed",
                "arguments":{"all":true, "dimensions":16}
            }
        }),
    );
    let memory_embed = read_mcp_message_for_id(&mut stdout, 65);
    assert_eq!(
        memory_embed["result"]["structuredContent"]["embedded_count"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":46,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_store",
                "arguments":{"content":"Second MCP memory before consolidation", "topic":"mcp-updated"}
            }
        }),
    );
    let _second_memory = read_mcp_message_for_id(&mut stdout, 46);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":47,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_consolidate",
                "arguments":{"topic":"mcp-updated"}
            }
        }),
    );
    let consolidated = read_mcp_message_for_id(&mut stdout, 47);
    assert_eq!(
        consolidated["result"]["structuredContent"]["status"].as_str(),
        Some("consolidated")
    );
    assert_eq!(
        consolidated["result"]["structuredContent"]["source_count"].as_u64(),
        Some(2)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":45,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_health",
                "arguments":{"topic":"mcp-updated", "consolidation_threshold": 1}
            }
        }),
    );
    let health = read_mcp_message_for_id(&mut stdout, 45);
    assert_eq!(
        health["result"]["structuredContent"]["topic_filter"].as_str(),
        Some("mcp-updated")
    );
    assert_eq!(
        health["result"]["structuredContent"]["topics_needing_consolidation"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_record",
                "arguments":{
                    "subject":"mcp",
                    "correction":"store feedback locally",
                    "topic":"mcp-feedback",
                    "context":"MCP feedback context",
                    "predicted":"ignore feedback",
                    "reason":"user correction",
                    "source":"mcp-test",
                    "project":"mcp-project-b"
                }
            }
        }),
    );
    let feedback = read_mcp_message_for_id(&mut stdout, 5);
    assert_eq!(
        feedback["result"]["structuredContent"]["correction"].as_str(),
        Some("store feedback locally")
    );
    assert_eq!(
        feedback["result"]["structuredContent"]["topic"].as_str(),
        Some("mcp-feedback")
    );
    assert_eq!(
        feedback["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_search",
                "arguments":{"query":"feedback", "project":"mcp-project-b", "limit": 3}
            }
        }),
    );
    let feedback_search = read_mcp_message_for_id(&mut stdout, 6);
    assert_eq!(
        feedback_search["result"]["structuredContent"][0]["correction"].as_str(),
        Some("store feedback locally")
    );
    assert_eq!(
        feedback_search["result"]["structuredContent"][0]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":52,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_list",
                "arguments":{"topic":"mcp-feedback", "limit": 3}
            }
        }),
    );
    let feedback_list = read_mcp_message_for_id(&mut stdout, 52);
    assert_eq!(
        feedback_list["result"]["structuredContent"][0]["topic"].as_str(),
        Some("mcp-feedback")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":53,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_apply",
                "arguments":{"id":1}
            }
        }),
    );
    let feedback_apply = read_mcp_message_for_id(&mut stdout, 53);
    assert_eq!(
        feedback_apply["result"]["structuredContent"]["applied_count"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":7,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_stats",
                "arguments":{}
            }
        }),
    );
    let feedback_stats = read_mcp_message_for_id(&mut stdout, 7);
    assert_eq!(
        feedback_stats["result"]["structuredContent"]["feedback_count"].as_i64(),
        Some(1)
    );
    assert_eq!(
        feedback_stats["result"]["structuredContent"]["applied_count"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":54,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_delete",
                "arguments":{"id":1}
            }
        }),
    );
    let feedback_delete = read_mcp_message_for_id(&mut stdout, 54);
    assert_eq!(
        feedback_delete["result"]["structuredContent"]["deleted"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":55,
            "method":"tools/call",
            "params":{
                "name":"packet28.feedback_record",
                "arguments":{
                    "subject":"mcp wakeup",
                    "correction":"wake-up feedback stays project scoped",
                    "topic":"mcp-feedback",
                    "project":"mcp-project-b"
                }
            }
        }),
    );
    let wakeup_feedback = read_mcp_message_for_id(&mut stdout, 55);
    assert_eq!(
        wakeup_feedback["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":60,
            "method":"tools/call",
            "params":{
                "name":"packet28.transcript_append",
                "arguments":{
                    "content":"MCP transcript recall should find reducer notes",
                    "session":"mcp-session",
                    "agent":"codex",
                    "role":"assistant",
                    "source":"mcp-test",
                    "project":"mcp-project-b"
                }
            }
        }),
    );
    let transcript = read_mcp_message_for_id(&mut stdout, 60);
    assert_eq!(
        transcript["result"]["structuredContent"]["session_key"].as_str(),
        Some("mcp-session")
    );
    assert_eq!(
        transcript["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":61,
            "method":"tools/call",
            "params":{
                "name":"packet28.transcript_search",
                "arguments":{"query":"reducer", "project":"mcp-project-b", "limit": 3}
            }
        }),
    );
    let transcript_search = read_mcp_message_for_id(&mut stdout, 61);
    assert_eq!(
        transcript_search["result"]["structuredContent"][0]["content"].as_str(),
        Some("MCP transcript recall should find reducer notes")
    );
    assert_eq!(
        transcript_search["result"]["structuredContent"][0]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":62,
            "method":"tools/call",
            "params":{
                "name":"packet28.transcript_stats",
                "arguments":{}
            }
        }),
    );
    let transcript_stats = read_mcp_message_for_id(&mut stdout, 62);
    assert_eq!(
        transcript_stats["result"]["structuredContent"]["message_count"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":64,
            "method":"tools/call",
            "params":{
                "name":"packet28.transcript_export",
                "arguments":{"session":"mcp-session"}
            }
        }),
    );
    let transcript_export = read_mcp_message_for_id(&mut stdout, 64);
    assert_eq!(
        transcript_export["result"]["structuredContent"]["format"].as_str(),
        Some("packet28.transcript.export")
    );
    assert_eq!(
        transcript_export["result"]["structuredContent"]["messages"][0]["project"].as_str(),
        Some("mcp-project-b")
    );
    let exported_transcript =
        serde_json::to_string(&transcript_export["result"]["structuredContent"]).unwrap();

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":65,
            "method":"tools/call",
            "params":{
                "name":"packet28.transcript_import",
                "arguments":{"content": exported_transcript}
            }
        }),
    );
    let transcript_import = read_mcp_message_for_id(&mut stdout, 65);
    assert_eq!(
        transcript_import["result"]["structuredContent"]["imported_count"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":63,
            "method":"tools/call",
            "params":{
                "name":"packet28.wakeup",
                "arguments":{"project":"mcp-project-b", "limit": 5, "max_tokens": 60, "format":"plain"}
            }
        }),
    );
    let wakeup = read_mcp_message_for_id(&mut stdout, 63);
    assert_eq!(
        wakeup["result"]["structuredContent"]["kind"].as_str(),
        Some("packet28.wakeup.v1")
    );
    assert_eq!(
        wakeup["result"]["structuredContent"]["format"].as_str(),
        Some("plain")
    );
    assert_eq!(
        wakeup["result"]["structuredContent"]["max_tokens"].as_u64(),
        Some(60)
    );
    assert!(wakeup["result"]["structuredContent"]["pack"]
        .as_str()
        .unwrap()
        .contains("mcp-project-b"));
    assert!(
        wakeup["result"]["structuredContent"]["transcripts"]
            .as_array()
            .unwrap()
            .len()
            >= 1
    );
    assert_eq!(
        wakeup["result"]["structuredContent"]["transcripts"][0]["project"].as_str(),
        Some("mcp-project-b")
    );
    assert_eq!(
        wakeup["result"]["structuredContent"]["feedback"][0]["project"].as_str(),
        Some("mcp-project-b")
    );
    assert_eq!(
        wakeup["result"]["structuredContent"]["memories"][0]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":68,
            "method":"tools/call",
            "params":{
                "name":"packet28.learn_project",
                "arguments":{"directory":root.path().to_str().unwrap(), "name":"McpLearnFixture", "memoir":"McpLearnMemoir", "limit":5}
            }
        }),
    );
    let learned = read_mcp_message_for_id(&mut stdout, 68);
    assert_eq!(
        learned["result"]["structuredContent"]["project_name"].as_str(),
        Some("McpLearnFixture")
    );
    assert_eq!(
        learned["result"]["structuredContent"]["memoir_name"].as_str(),
        Some("McpLearnMemoir")
    );
    assert!(
        learned["result"]["structuredContent"]["total_concepts"]
            .as_u64()
            .unwrap()
            >= 3
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":54,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_create",
                "arguments":{"name":"McpMemoir", "description":"MCP graph container"}
            }
        }),
    );
    let graph_memoir = read_mcp_message_for_id(&mut stdout, 54);
    assert_eq!(
        graph_memoir["result"]["structuredContent"]["name"].as_str(),
        Some("McpMemoir")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":55,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_add_concept",
                "arguments":{
                    "name":"Packet28",
                    "description":"local context runtime",
                    "memoir":"McpMemoir",
                    "labels":["domain:context"],
                    "confidence":0.91,
                    "source_ids":["memory:mcp"]
                }
            }
        }),
    );
    let graph_concept = read_mcp_message_for_id(&mut stdout, 55);
    assert_eq!(
        graph_concept["result"]["structuredContent"]["name"].as_str(),
        Some("Packet28")
    );
    assert_eq!(
        graph_concept["result"]["structuredContent"]["memoir_name"].as_str(),
        Some("McpMemoir")
    );
    assert_eq!(
        graph_concept["result"]["structuredContent"]["confidence"].as_f64(),
        Some(0.91)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":56,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_refine",
                "arguments":{"name":"Packet28", "description":"local context runtime with reducers"}
            }
        }),
    );
    let refined = read_mcp_message_for_id(&mut stdout, 56);
    assert_eq!(
        refined["result"]["structuredContent"]["description"].as_str(),
        Some("local context runtime with reducers")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":53,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_add_concept",
                "arguments":{"name":"Reducers", "memoir":"McpMemoir"}
            }
        }),
    );
    let reducer_concept = read_mcp_message_for_id(&mut stdout, 53);
    assert_eq!(
        reducer_concept["result"]["structuredContent"]["memoir_name"].as_str(),
        Some("McpMemoir")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":57,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_link",
                "arguments":{"source":"Packet28", "target":"Reducers", "relation":"uses"}
            }
        }),
    );
    let relation = read_mcp_message_for_id(&mut stdout, 57);
    assert_eq!(
        relation["result"]["structuredContent"]["relation"].as_str(),
        Some("uses")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":58,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_search",
                "arguments":{"query":"context", "memoir":"McpMemoir", "label":"domain:context", "limit": 5}
            }
        }),
    );
    let graph_search = read_mcp_message_for_id(&mut stdout, 58);
    assert!(
        graph_search["result"]["structuredContent"]
            .as_array()
            .unwrap()
            .len()
            >= 1
    );
    assert_eq!(
        graph_search["result"]["structuredContent"][0]["memoir_name"].as_str(),
        Some("McpMemoir")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":59,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_export",
                "arguments":{"format":"dot", "limit": 5}
            }
        }),
    );
    let graph_export = read_mcp_message_for_id(&mut stdout, 59);
    assert_eq!(
        graph_export["result"]["structuredContent"]["format"].as_str(),
        Some("dot")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":64,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_stats",
                "arguments":{}
            }
        }),
    );
    let graph_stats = read_mcp_message_for_id(&mut stdout, 64);
    assert!(
        graph_stats["result"]["structuredContent"]["relation_count"]
            .as_i64()
            .unwrap()
            >= 1
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":66,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_show",
                "arguments":{"name":"McpMemoir", "limit": 5}
            }
        }),
    );
    let graph_show = read_mcp_message_for_id(&mut stdout, 66);
    assert_eq!(
        graph_show["result"]["structuredContent"]["memoir"]["name"].as_str(),
        Some("McpMemoir")
    );
    assert_eq!(
        graph_show["result"]["structuredContent"]["concepts"][0]["revision"].as_i64(),
        Some(2)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":8,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_inspect",
                "arguments":{"limit": 5}
            }
        }),
    );
    let graph = read_mcp_message_for_id(&mut stdout, 8);
    assert!(graph["result"]["structuredContent"]["concepts"].is_array());

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":67,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_inspect_concept",
                "arguments":{"name":"Packet28", "memoir":"McpMemoir", "depth": 1}
            }
        }),
    );
    let graph_concept_inspect = read_mcp_message_for_id(&mut stdout, 67);
    assert_eq!(
        graph_concept_inspect["result"]["structuredContent"]["concept"]["name"].as_str(),
        Some("Packet28")
    );
    assert!(
        graph_concept_inspect["result"]["structuredContent"]["neighbors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|concept| concept["name"] == "Reducers")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":69,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_store",
                "arguments":{
                    "content":"Distill MCP memory into a graph concept",
                    "topic":"mcp-distill",
                    "keywords":"McpDistill,graph",
                    "importance":"critical"
                }
            }
        }),
    );
    let mcp_distill_memory = read_mcp_message_for_id(&mut stdout, 69);
    assert_eq!(
        mcp_distill_memory["result"]["structuredContent"]["topic"].as_str(),
        Some("mcp-distill")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":70,
            "method":"tools/call",
            "params":{
                "name":"packet28.graph_distill",
                "arguments":{"from_topic":"mcp-distill", "into":"McpMemoir", "limit": 5}
            }
        }),
    );
    let graph_distill = read_mcp_message_for_id(&mut stdout, 70);
    assert_eq!(
        graph_distill["result"]["structuredContent"]["created_count"].as_u64(),
        Some(2)
    );
    assert_eq!(
        graph_distill["result"]["structuredContent"]["concepts"][0]["name"].as_str(),
        Some("McpDistill")
    );
    assert_eq!(
        graph_distill["result"]["structuredContent"]["concepts"][1]["name"].as_str(),
        Some("graph")
    );

    for (id, content) in [
        (72, "Pattern extraction should group adapter memories"),
        (
            73,
            "Adapter pattern extraction should create graph concepts",
        ),
    ] {
        write_mcp_message(
            &mut stdin,
            &json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"tools/call",
                "params":{
                    "name":"packet28.memory_store",
                    "arguments":{
                        "content":content,
                        "topic":"mcp-patterns",
                        "keywords":"adapter,pattern",
                        "importance":"critical"
                    }
                }
            }),
        );
        let stored_pattern_memory = read_mcp_message_for_id(&mut stdout, id);
        assert_eq!(
            stored_pattern_memory["result"]["structuredContent"]["topic"].as_str(),
            Some("mcp-patterns")
        );
    }

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":74,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_extract_patterns",
                "arguments":{"topic":"mcp-patterns", "memoir":"McpMemoir", "min_cluster_size":2}
            }
        }),
    );
    let memory_patterns = read_mcp_message_for_id(&mut stdout, 74);
    assert!(
        memory_patterns["result"]["structuredContent"]["pattern_count"]
            .as_u64()
            .unwrap()
            >= 2
    );
    assert!(memory_patterns["result"]["structuredContent"]["patterns"]
        .as_array()
        .unwrap()
        .iter()
        .any(|pattern| pattern["key"] == "adapter" && pattern["memory_count"].as_u64() == Some(2)));
    assert!(
        memory_patterns["result"]["structuredContent"]["created_concepts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|concept| concept["name"] == "adapter" && concept["memoir_name"] == "McpMemoir")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":44,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_forget",
                "arguments":{"topic":"mcp-updated"}
            }
        }),
    );
    let forgotten = read_mcp_message_for_id(&mut stdout, 44);
    assert_eq!(
        forgotten["result"]["structuredContent"]["deleted"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":48,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_store",
                "arguments":{"content":"MCP prunable memory", "topic":"mcp-prune", "importance":"low"}
            }
        }),
    );
    let _prunable = read_mcp_message_for_id(&mut stdout, 48);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":49,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_decay",
                "arguments":{"factor":0.1}
            }
        }),
    );
    let decayed = read_mcp_message_for_id(&mut stdout, 49);
    assert_eq!(
        decayed["result"]["structuredContent"]["decayed_count"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":50,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_prune",
                "arguments":{"threshold":0.5, "dry_run":true}
            }
        }),
    );
    let prune_preview = read_mcp_message_for_id(&mut stdout, 50);
    assert_eq!(
        prune_preview["result"]["structuredContent"]["candidate_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        prune_preview["result"]["structuredContent"]["deleted_count"].as_u64(),
        Some(0)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":51,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_prune",
                "arguments":{"threshold":0.5}
            }
        }),
    );
    let pruned = read_mcp_message_for_id(&mut stdout, 51);
    assert_eq!(
        pruned["result"]["structuredContent"]["deleted_count"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":66,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_pending_enqueue",
                "arguments":{
                    "raw_output":"- MCP pending extraction stores durable facts",
                    "project":"mcp-project-b",
                    "tool_name":"Bash"
                }
            }
        }),
    );
    let pending_enqueue = read_mcp_message_for_id(&mut stdout, 66);
    assert_eq!(
        pending_enqueue["result"]["structuredContent"]["project"].as_str(),
        Some("mcp-project-b")
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":69,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_pending_stats",
                "arguments":{}
            }
        }),
    );
    let pending_stats = read_mcp_message_for_id(&mut stdout, 69);
    assert_eq!(
        pending_stats["result"]["structuredContent"]["pending_extraction_count"].as_i64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":70,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_pending_process",
                "arguments":{"limit": 5}
            }
        }),
    );
    let pending_process = read_mcp_message_for_id(&mut stdout, 70);
    assert_eq!(
        pending_process["result"]["structuredContent"]["extracted_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        pending_process["result"]["structuredContent"]["deleted_count"].as_u64(),
        Some(1)
    );

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":71,
            "method":"tools/call",
            "params":{
                "name":"packet28.memory_recall",
                "arguments":{"query":"durable facts", "project":"mcp-project-b"}
            }
        }),
    );
    let pending_recall = read_mcp_message_for_id(&mut stdout, 71);
    assert_eq!(
        pending_recall["result"]["structuredContent"][0]["source"].as_str(),
        Some("pending-extraction:Bash")
    );

    let _ = child.kill();
    let _ = child.wait();
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn test_feedback_and_graph_cli_use_sqlite() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname = \"cli-learn-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nserde_json = \"1\"\n",
    )
    .unwrap();
    fs::create_dir_all(project.path().join("src")).unwrap();
    fs::write(project.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "feedback",
            "record",
            "test subject",
            "prefer focused reducers",
            "--topic",
            "reducers",
            "--context",
            "test context",
            "--predicted",
            "verbose reducers",
            "--reason",
            "too noisy",
            "--source",
            "cli-test",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("prefer focused reducers"))
        .stdout(predicate::str::contains("\"topic\":\"reducers\""))
        .stdout(predicate::str::contains("\"project\":\"coverage-b\""))
        .stdout(predicate::str::contains(
            "\"predicted\":\"verbose reducers\"",
        ));

    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "search", "focused", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prefer focused reducers"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "list", "--topic", "reducers", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"topic\":\"reducers\""));
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "apply", "1", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"applied_count\":1"));
    let conn = Connection::open(home.path().join(".packet28").join("packet28.db")).unwrap();
    let feedback_fts_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM feedback_fts_all", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(feedback_fts_rows, 1);
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"feedback_count\":1"))
        .stdout(predicate::str::contains("\"applied_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "delete", "1", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"deleted\":1"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "learn",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--project-name",
            "CliLearnFixture",
            "--memoir",
            "CliLearnMemoir",
            "--project-limit",
            "5",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"project_name\":\"CliLearnFixture\"",
        ))
        .stdout(predicate::str::contains(
            "\"memoir_name\":\"CliLearnMemoir\"",
        ))
        .stdout(predicate::str::contains("\"link_count\""))
        .stdout(predicate::str::contains("serde_json"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "show", "CliLearnMemoir", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CliLearnFixture"))
        .stdout(predicate::str::contains("serde_json"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "transcript",
            "append",
            "Need compact transcript recall for reducers",
            "--session",
            "cli-session",
            "--agent",
            "codex",
            "--role",
            "user",
            "--source",
            "cli-test",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"session_key\":\"cli-session\""))
        .stdout(predicate::str::contains("\"role\":\"user\""))
        .stdout(predicate::str::contains("\"project\":\"coverage-b\""));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "transcript",
            "append",
            "Foreign transcript recall for reducers",
            "--session",
            "foreign-session",
            "--agent",
            "codex",
            "--role",
            "user",
            "--source",
            "cli-test",
            "--project",
            "coverage-foreign",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"project\":\"coverage-foreign\""));
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "search", "reducers", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("compact transcript recall"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "show", "cli-session", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"agent\":\"codex\""));
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"message_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"session_count\":2"))
        .stdout(predicate::str::contains("\"message_count\":2"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "wakeup",
            "--query",
            "reducers",
            "--project",
            "coverage-b",
            "--format",
            "plain",
            "--max-tokens",
            "80",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\":\"packet28.wakeup.v1\""))
        .stdout(predicate::str::contains("\"format\":\"plain\""))
        .stdout(predicate::str::contains("\"estimated_tokens\""))
        .stdout(predicate::str::contains("\"transcripts\""))
        .stdout(predicate::str::contains("\"pack\""))
        .stdout(predicate::str::contains("compact transcript recall"))
        .stdout(predicate::str::contains("\"project\":\"coverage-b\""))
        .stdout(predicate::str::contains("Foreign transcript recall").not());
    let transcript_fts_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM transcript_messages_fts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(transcript_fts_rows, 2);

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "create",
            "--name",
            "Packet28Memoir",
            "--description",
            "Packet28 graph parity evidence",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28Memoir"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "add-concept",
            "Packet28",
            "--memoir",
            "Packet28Memoir",
            "--label",
            "domain:context",
            "--confidence",
            "0.82",
            "--source-id",
            "memory:packet28",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28"))
        .stdout(predicate::str::contains(
            "\"memoir_name\":\"Packet28Memoir\"",
        ))
        .stdout(predicate::str::contains("domain:context"))
        .stdout(predicate::str::contains("\"confidence\":0.82"))
        .stdout(predicate::str::contains("memory:packet28"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "refine",
            "Packet28",
            "local context runtime with reducers",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "local context runtime with reducers",
        ));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "add-concept",
            "Reducers",
            "--memoir",
            "Packet28Memoir",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "link",
            "Packet28",
            "Reducers",
            "--relation",
            "uses",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "search",
            "context",
            "--memoir",
            "Packet28Memoir",
            "--label",
            "domain:context",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28"))
        .stdout(predicate::str::contains("domain:context"))
        .stdout(predicate::str::contains("Packet28Memoir"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "export", "--format", "dot"])
        .assert()
        .success()
        .stdout(predicate::str::contains("digraph packet28_graph"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"relation\":\"uses\""));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28Memoir"))
        .stdout(predicate::str::contains("\"concept_count\":2"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "show", "Packet28Memoir", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"revision\":2"))
        .stdout(predicate::str::contains("\"average_confidence\":0.659"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "inspect", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28"))
        .stdout(predicate::str::contains("Reducers"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "inspect-concept",
            "Packet28",
            "--memoir",
            "Packet28Memoir",
            "--depth",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"concept\""))
        .stdout(predicate::str::contains("\"neighbors\""))
        .stdout(predicate::str::contains("\"relations\""))
        .stdout(predicate::str::contains("Reducers"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Reducer distillation should become a graph concept",
            "--topic",
            "graph-distill",
            "--keywords",
            "ReducerDistill,graph",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "distill",
            "--from-topic",
            "graph-distill",
            "--into",
            "Packet28Memoir",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"created_count\":2"))
        .stdout(predicate::str::contains("ReducerDistill"))
        .stdout(predicate::str::contains("\"graph\""))
        .stdout(predicate::str::contains("topic:graph-distill"))
        .stdout(predicate::str::contains("memory:"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "show", "Packet28Memoir", "--limit", "20", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ReducerDistill"))
        .stdout(predicate::str::contains("\"target\":\"graph\""))
        .stdout(predicate::str::contains("\"relation\":\"mentions\""));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "delete", "Packet28", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"deleted_concepts\":1"));
}

#[test]
fn test_transcript_export_import_round_trip() {
    let home_a = TempDir::new().unwrap();
    let home_b = TempDir::new().unwrap();
    let export_path = home_a.path().join("transcripts.json");

    suite_cmd()
        .env("HOME", home_a.path())
        .args([
            "transcript",
            "append",
            "Exported transcript context",
            "--session",
            "export-session",
            "--agent",
            "codex",
            "--role",
            "assistant",
            "--source",
            "fixture",
        ])
        .assert()
        .success();

    suite_cmd()
        .env("HOME", home_a.path())
        .args([
            "transcript",
            "export",
            "--session",
            "export-session",
            "--output",
            export_path.to_str().unwrap(),
            "--pretty",
        ])
        .assert()
        .success();
    assert!(fs::read_to_string(&export_path)
        .unwrap()
        .contains("packet28.transcript.export"));

    suite_cmd()
        .env("HOME", home_b.path())
        .args([
            "transcript",
            "import",
            export_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"imported_count\":1"));

    suite_cmd()
        .env("HOME", home_b.path())
        .args(["transcript", "show", "export-session", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Exported transcript context"))
        .stdout(predicate::str::contains("\"agent\":\"codex\""));
}

#[test]
fn test_dashboard_shows_local_product_metrics() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(root.path())
        .status()
        .unwrap();
    fs::write(root.path().join("tracked.txt"), "changed\n").unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
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
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "store", "dashboard memory"])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "append", "dashboard transcript context"])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "record", "dashboard", "shows feedback"])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "link", "Dashboard", "Packet28"])
        .assert()
        .success();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "dashboard",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"commands_reduced\":1"))
        .stdout(predicate::str::contains("\"memory_count\":1"))
        .stdout(predicate::str::contains("\"memory_topics\""))
        .stdout(predicate::str::contains("\"topic\":\"general\""))
        .stdout(predicate::str::contains("\"memory_health\""))
        .stdout(predicate::str::contains("\"total_memories\":1"))
        .stdout(predicate::str::contains("\"feedback_corrections\":1"))
        .stdout(predicate::str::contains("\"feedback_stats\""))
        .stdout(predicate::str::contains("\"transcript_stats\""))
        .stdout(predicate::str::contains("\"message_count\":1"))
        .stdout(predicate::str::contains("\"graph_concepts\""))
        .stdout(predicate::str::contains("\"graph_stats\""))
        .stdout(predicate::str::contains("\"windsurf_doctor_status\""));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["dashboard", "--root", root.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("memory_topics=1"))
        .stdout(predicate::str::contains("topics_needing_consolidation=0"))
        .stdout(predicate::str::contains("transcript_messages=1"));

    let html_path = root.path().join("packet28-dashboard.html");
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "dashboard",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "html",
            "--output",
            html_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("dashboard_html="));
    let html = fs::read_to_string(&html_path).unwrap();
    assert!(html.contains("<title>Packet28 Dashboard</title>"));
    assert!(html.contains("Saved tokens"));
    assert!(html.contains("Memory Topics"));
    assert!(html.contains("Integration Health"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "dashboard",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "tui",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28 Dashboard"))
        .stdout(predicate::str::contains("panel=Overview"))
        .stdout(predicate::str::contains("commands_reduced=1"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "dashboard",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "tui",
            "--interactive",
        ])
        .write_stdin("memory\nintegrations\nq\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("panel=Memory"))
        .stdout(predicate::str::contains("recent_memories:"))
        .stdout(predicate::str::contains("panel=Integrations"))
        .stdout(predicate::str::contains("windsurf_doctor_status="));
}

#[test]
fn test_discover_reports_run_missed_savings() {
    let root = TempDir::new().unwrap();
    let missing_sessions = root.path().join("missing-sessions");
    suite_cmd()
        .current_dir(root.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "printf",
            "hello",
        ])
        .assert()
        .success();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "discover",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            missing_sessions.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"missed_savings\""))
        .stdout(predicate::str::contains("\"command\":\"printf hello\""));
}

#[test]
fn test_discover_splits_chained_session_commands() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root.path().join("claude-projects").join("project");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_file = sessions_dir.join("session-b.jsonl");
    let line = json!({
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": {
                        "command": "git status --short && echo raw"
                    }
                },
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": {
                        "command": "pytest -q"
                    }
                }
            ]
        }
    });
    fs::write(&session_file, format!("{line}\n")).unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "discover",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"commands_found\":3"))
        .stdout(predicate::str::contains("\"supported_commands\":2"))
        .stdout(predicate::str::contains("\"unsupported_commands\":1"))
        .stdout(predicate::str::contains("\"command\":\"echo\""));
}

#[test]
fn test_discover_all_and_since_scan_multiple_session_files() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root.path().join("claude-projects").join("project");
    fs::create_dir_all(&sessions_dir).unwrap();
    for (name, command) in [
        ("session-a.jsonl", "git status --short"),
        ("session-b.jsonl", "pytest -q"),
    ] {
        let line = json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": command }
                }]
            }
        });
        fs::write(sessions_dir.join(name), format!("{line}\n")).unwrap();
    }

    suite_cmd()
        .current_dir(root.path())
        .args([
            "discover",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--limit",
            "1",
            "--all",
            "--since",
            "7",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sessions_scanned\":2"))
        .stdout(predicate::str::contains("\"commands_found\":2"))
        .stdout(predicate::str::contains("\"supported_commands\":2"));
}

#[test]
fn test_hook_records_local_event_log_stats_and_dashboard_count() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let payload = json!({
        "hook_event_name":"PostToolUse",
        "task_id":"hook-telemetry-task",
        "session_id":"hook-telemetry-session",
        "project":"coverage-hook",
        "matcher":"Bash",
        "tool_name":"Bash",
        "tool_input":{"command":"git status --short"},
        "tool_response":{"stdout":"- Hook auto extraction stores post tool facts\n","stderr":"","exit_code":0}
    });

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "claude", "--root", root.path().to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(serde_json::to_string(&payload).unwrap().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "log", "--limit", "5", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"runtime\":\"claude\""))
        .stdout(predicate::str::contains("\"event_kind\":\"post_tool_use\""))
        .stdout(predicate::str::contains("hook-telemetry-session"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event_count\":1"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "dashboard",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"hook_event_history\":1"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["memory", "pending", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pending_extraction_count\":1"));
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["transcript", "search", "post tool facts", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hook-telemetry-session"))
        .stdout(predicate::str::contains(
            "auto extraction stores post tool facts",
        ));
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["memory", "pending", "process", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"extracted_count\":1"))
        .stdout(predicate::str::contains("\"deleted_count\":1"));
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "post tool facts",
            "--project",
            "coverage-hook",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "auto extraction stores post tool facts",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["daemon", "stop", "--root", root.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn test_hook_failure_output_is_searchable_transcript_context() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let payload = json!({
        "hook_event_name":"PostToolUseFailure",
        "task_id":"hook-failure-task",
        "session_id":"hook-failure-session",
        "project":"coverage-hook",
        "matcher":"Bash",
        "tool_name":"Bash",
        "tool_input":{"command":"cargo test failing_case"},
        "error":"failure transcript keeps compiler diagnostic E0425"
    });

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "claude", "--root", root.path().to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(serde_json::to_string(&payload).unwrap().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["transcript", "search", "compiler diagnostic", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hook-failure-session"))
        .stdout(predicate::str::contains("E0425"))
        .stdout(predicate::str::contains("\"source\":\"packet28-hook\""));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"event_kind\":\"post_tool_use_failure\"",
        ));
}

#[test]
fn test_hook_session_end_is_recorded_in_local_lifecycle_log() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let payload = json!({
        "hook_event_name":"SessionEnd",
        "task_id":"hook-session-end-task",
        "session_id":"hook-session-end-session",
        "matcher":"session",
        "cwd": root.path().display().to_string(),
    });

    let (status, _stdout, stderr) = run_hook_raw_with_env(
        "claude",
        root.path(),
        &serde_json::to_string(&payload).unwrap(),
        &[("HOME", home.path().as_os_str())],
    );
    assert_eq!(status, 0, "stderr={stderr}");

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "log", "--limit", "5", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event_kind\":\"session_end\""))
        .stdout(predicate::str::contains("hook-session-end-session"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["hook", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event_kind\":\"session_end\""))
        .stdout(predicate::str::contains("\"event_count\":1"));
}

#[test]
fn test_session_reports_adoption_from_session_jsonl() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root
        .path()
        .join("claude-projects")
        .join("project")
        .join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_file = sessions_dir.join("session-a.jsonl");
    let line = json!({
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": {
                        "command": "git status --short && echo raw"
                    }
                },
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": {
                        "command": "Packet28 run cargo check"
                    }
                }
            ]
        }
    });
    fs::write(&session_file, format!("{line}\n")).unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "session",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sessions_scanned\":1"))
        .stdout(predicate::str::contains("\"total_commands\":3"))
        .stdout(predicate::str::contains("\"packet28_commands\":2"))
        .stdout(predicate::str::contains("\"adoption_pct\":66.666"));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "session",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28 Session Overview"))
        .stdout(predicate::str::contains("Session"))
        .stdout(predicate::str::contains("Packet28"))
        .stdout(predicate::str::contains("@@@.."))
        .stdout(predicate::str::contains("Average adoption: 67%"))
        .stdout(predicate::str::contains("Packet28 discover --sessions-dir"));
}

#[test]
fn test_session_adoption_all_and_since_scan_multiple_session_files() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root
        .path()
        .join("claude-projects")
        .join("project")
        .join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    for (name, command) in [
        ("session-a.jsonl", "git status --short"),
        ("session-b.jsonl", "Packet28 run cargo check"),
    ] {
        let line = json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": command }
                }]
            }
        });
        fs::write(sessions_dir.join(name), format!("{line}\n")).unwrap();
    }

    suite_cmd()
        .current_dir(root.path())
        .args([
            "session",
            "--root",
            root.path().to_str().unwrap(),
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--limit",
            "1",
            "--all",
            "--since",
            "7",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sessions_scanned\":2"))
        .stdout(predicate::str::contains("\"total_commands\":2"))
        .stdout(predicate::str::contains("\"packet28_commands\":2"));
}

#[test]
fn test_learn_detects_cli_correction_from_session_history() {
    let root = TempDir::new().unwrap();
    let sessions_dir = root.path().join("claude-projects").join("project");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_file = sessions_dir.join("session-learn.jsonl");
    let bad_use = json!({
        "type": "assistant",
        "message": {"content": [{
            "type": "tool_use",
            "name": "Bash",
            "input": {"command": "git status --porcelain=v9"}
        }]}
    });
    let bad_result = json!({
        "type": "user",
        "message": {"content": [{
            "type": "tool_result",
            "is_error": true,
            "content": "error: unknown option `porcelain=v9`"
        }]}
    });
    let good_use = json!({
        "type": "assistant",
        "message": {"content": [{
            "type": "tool_use",
            "name": "Bash",
            "input": {"command": "git status --short"}
        }]}
    });
    let good_result = json!({
        "type": "user",
        "message": {"content": [{
            "type": "tool_result",
            "is_error": false,
            "content": " M src/main.rs"
        }]}
    });
    fs::write(
        &session_file,
        format!("{bad_use}\n{bad_result}\n{good_use}\n{good_result}\n"),
    )
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .args([
            "learn",
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--min-frequency",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sessions_scanned\":1"))
        .stdout(predicate::str::contains("\"corrections_found\":1"))
        .stdout(predicate::str::contains("git status --porcelain=v9"))
        .stdout(predicate::str::contains("git status --short"))
        .stdout(predicate::str::contains("\"error_type\":\"unknown_flag\""));

    suite_cmd()
        .current_dir(root.path())
        .args([
            "learn",
            "--sessions-dir",
            root.path().join("claude-projects").to_str().unwrap(),
            "--min-frequency",
            "1",
            "--write-rules",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Corrections found: 1"))
        .stdout(predicate::str::contains("unknown_flag"))
        .stdout(predicate::str::contains("Corrections written"));
    assert!(root
        .path()
        .join(".claude")
        .join("rules")
        .join("cli-corrections.md")
        .exists());
}

fn write_mcp_message(stdin: &mut ChildStdin, value: &Value) {
    let body = serde_json::to_vec(value).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.flush().unwrap();
}

fn write_mcp_message_newline(stdin: &mut ChildStdin, value: &Value) {
    let body = serde_json::to_vec(value).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
}

fn read_mcp_message(stdout: &mut BufReader<ChildStdout>) -> Value {
    let mut content_length = None::<usize>;
    let mut line = String::new();
    loop {
        line.clear();
        stdout.read_line(&mut line).unwrap();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
    }
    let mut body = vec![0_u8; content_length.unwrap()];
    stdout.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn read_mcp_message_newline(stdout: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    loop {
        line.clear();
        stdout.read_line(&mut line).unwrap();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        return serde_json::from_str(trimmed).unwrap();
    }
}

fn read_mcp_message_for_id(stdout: &mut BufReader<ChildStdout>, expected_id: u64) -> Value {
    loop {
        let value = read_mcp_message(stdout);
        if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
            return value;
        }
    }
}

fn start_mcp_server(root: &Path) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = mcp_cmd()
        .current_dir(root)
        .args(["mcp", "serve", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (child, stdin, stdout)
}

fn start_mcp_proxy_server(
    root: &Path,
    config_path: &Path,
    task_id: &str,
) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = mcp_cmd()
        .current_dir(root)
        .args([
            "mcp",
            "proxy",
            "--root",
            root.to_str().unwrap(),
            "--upstream-config",
            config_path.to_str().unwrap(),
            "--task-id",
            task_id,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (child, stdin, stdout)
}

fn start_mcp_proxy_server_with_tool(
    root: &Path,
    config_path: &Path,
    task_id: &str,
    tool_name: &str,
) -> (Child, ChildStdin, BufReader<ChildStdout>, Value) {
    for _ in 0..3 {
        let (mut child, mut stdin, mut stdout) = start_mcp_proxy_server(root, config_path, task_id);
        initialize_mcp_session(&mut stdin, &mut stdout);
        write_mcp_message(
            &mut stdin,
            &json!({
                "jsonrpc":"2.0",
                "id":2,
                "method":"tools/list"
            }),
        );
        let tools = read_mcp_message_for_id(&mut stdout, 2);
        let has_tool = tools["result"]["tools"]
            .as_array()
            .is_some_and(|items| items.iter().any(|tool| tool["name"] == tool_name));
        if has_tool {
            return (child, stdin, stdout, tools);
        }
        let _ = child.kill();
        let _ = child.wait();
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("proxy tool catalog never exposed required tool '{tool_name}'");
}

fn initialize_mcp_session(stdin: &mut ChildStdin, stdout: &mut BufReader<ChildStdout>) {
    write_mcp_message(
        stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}
        }),
    );
    let _ = read_mcp_message_for_id(stdout, 1);
}

fn workspace_packet28_version() -> String {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = workspace.parent().unwrap().parent().unwrap();
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let value: toml::Value = toml::from_str(&manifest).unwrap();
    value["workspace"]["package"]["version"]
        .as_str()
        .unwrap()
        .to_string()
}

fn write_intention_via_mcp(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    id: u64,
    task_id: &str,
    text: &str,
    step_id: &str,
    paths: &[&str],
) -> Value {
    write_mcp_message(
        stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":"tools/call",
            "params":{
                "name":"packet28.write_intention",
                "arguments":{
                    "task_id":task_id,
                    "text":text,
                    "step_id":step_id,
                    "paths":paths,
                }
            }
        }),
    );
    read_mcp_message_for_id(stdout, id)
}

fn run_claude_hook(root: &Path, payload: &Value) -> (i32, String) {
    let (status, stdout, _) =
        run_hook_raw("claude", root, &serde_json::to_string(payload).unwrap());
    (status, stdout)
}

fn run_hook_raw(runtime: &str, root: &Path, stdin_payload: &str) -> (i32, String, String) {
    run_hook_raw_with_env(runtime, root, stdin_payload, &[])
}

fn run_hook_raw_with_env(
    runtime: &str,
    root: &Path,
    stdin_payload: &str,
    envs: &[(&str, &std::ffi::OsStr)],
) -> (i32, String, String) {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    command
        .current_dir(root)
        .args(["hook", runtime, "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_payload.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn write_invalid_guard_context(path: &Path) {
    fs::write(
        path,
        r#"
version: 2
policy:
  tools:
    allowlist: [""]
  reducers:
    allowlist: [""]
  paths:
    include: ["["]
    exclude: []
  token_budget:
    cap: 0
  runtime_budget:
    cap_ms: 0
  redaction:
    forbidden_patterns: ["("]
"#,
    )
    .unwrap();
}

fn write_governed_context(path: &Path) {
    fs::write(
        path,
        r#"
version: 1
policy:
  tools:
    allowlist: ["diffy", "testy", "stacky", "buildy", "contextq"]
  reducers:
    allowlist: ["analyze", "impact", "slice", "reduce", "assemble", "contextq.assemble", "diffy.analyze", "testy.impact", "stacky.slice", "buildy.reduce", "governed.assemble"]
  paths:
    include: ["**"]
    exclude: []
  token_budget:
    cap: 5000
  runtime_budget:
    cap_ms: 5000
  tool_call_budget:
    cap: 10
  redaction:
    forbidden_patterns: []
  human_review:
    required: false
    on_policy_violation: true
    on_budget_violation: true
    on_redaction_violation: true
    paths: []
"#,
    )
    .unwrap();
}

fn write_guard_packet(path: &Path) {
    fs::write(
        path,
        r#"{
  "tool": "covy",
  "reducer": "merge",
  "paths": ["src/lib.rs"],
  "token_usage": 50,
  "runtime_ms": 300,
  "payload": {"message": "all clear"}
}"#,
    )
    .unwrap();
}

fn write_denied_guard_packet(path: &Path) {
    fs::write(
        path,
        r#"{
  "tool": "covy",
  "reducer": "merge",
  "paths": ["src/private/secret.rs"],
  "token_usage": 500,
  "runtime_ms": 5000,
  "payload": {"password": "secret"}
}"#,
    )
    .unwrap();
}

fn write_wrapped_guard_packet(path: &Path) {
    fs::write(
        path,
        r#"{
  "schema_version": "suite.packet.v1",
  "packet_type": "suite.proxy.run.v1",
  "packet": {
    "tool": "proxy",
    "payload": {
      "highlights": ["my_password_is_secret123"]
    }
  }
}"#,
    )
    .unwrap();
}

fn write_redaction_only_context(path: &Path) {
    fs::write(
        path,
        r#"
version: 1
policy:
  paths:
    include: ["**"]
    exclude: []
  redaction:
    forbidden_patterns: ["secret123", "(?i)password"]
"#,
    )
    .unwrap();
}

fn write_permissive_context(path: &Path) {
    fs::write(
        path,
        r#"
version: 1
policy:
  paths:
    include: ["**"]
    exclude: []
  redaction:
    forbidden_patterns: []
"#,
    )
    .unwrap();
}

fn write_context_packet(path: &Path, packet_id: &str, title: &str, body: &str, path_ref: &str) {
    fs::write(
        path,
        format!(
            r#"{{
  "packet_id": "{packet_id}",
  "tool": "{packet_id}",
  "reducer": "reduce",
  "paths": ["{path_ref}"],
  "sections": [
    {{
      "title": "{title}",
      "body": "{body}",
      "refs": [{{ "kind": "file", "value": "{path_ref}" }}],
      "relevance": 0.9
    }}
  ]
}}"#
        ),
    )
    .unwrap();
}

fn write_packet_value(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

fn write_stack_log(path: &Path) {
    fs::write(
        path,
        r#"
java.lang.IllegalStateException: boom
  at com.example.Service.run(src/service.rs:42)
  at com.example.Main.main(src/main.rs:10)

java.lang.IllegalStateException: boom
  at com.example.Service.run(src/service.rs:42)
  at com.example.Main.main(src/main.rs:10)
"#,
    )
    .unwrap();
}

fn write_build_log(path: &Path) {
    fs::write(
        path,
        r#"
src/lib.rs:10:5: error: cannot find value `x` in this scope [E0425]
src/lib.rs:10:5: error: cannot find value `x` in this scope [E0425]
main.c(40,2): warning C4996: use of deprecated function
"#,
    )
    .unwrap();
}

fn write_repo_fixture(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("alpha.rs"),
        r#"
use crate::beta::Beta;

fn alpha() {}
struct Alpha;
"#,
    )
    .unwrap();
    fs::write(
        src.join("beta.rs"),
        r#"
fn beta() {}
enum Beta {
  A,
}
"#,
    )
    .unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed with {status}", args);
}

fn setup_changed_repo(root: &Path) {
    write_repo_fixture(root);
    git(root, &["init"]);
    git(root, &["add", "src/alpha.rs", "src/beta.rs"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "init",
        ],
    );
    fs::write(
        root.join("src/alpha.rs"),
        r#"
use crate::beta::Beta;

fn alpha() -> i32 { 2 }
struct Alpha;
"#,
    )
    .unwrap();
    git(root, &["add", "src/alpha.rs"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "change alpha",
        ],
    );
}

fn init_repo(root: &Path) {
    git(root, &["init"]);
}

fn kernel_cache_file(root: &Path) -> PathBuf {
    root.join(".packet28").join("packet-cache-v2.bin")
}

fn parse_packet_wrapper(output: &[u8], packet_type: &str) -> Value {
    let value: Value = serde_json::from_slice(output).unwrap();
    assert_eq!(
        value.get("schema_version").and_then(Value::as_str),
        Some("suite.packet.v1")
    );
    assert_eq!(
        value.get("packet_type").and_then(Value::as_str),
        Some(packet_type)
    );
    assert!(value.get("packet").is_some());
    value
}

fn parse_broker_response(output: &[u8]) -> Value {
    let value: Value = serde_json::from_slice(output).unwrap();
    assert!(value.get("context_version").is_some());
    assert!(value.get("brief").is_some());
    value
}

fn packet_payload(wrapper: &Value) -> &Value {
    wrapper
        .get("packet")
        .and_then(|packet| packet.get("payload"))
        .expect("packet.payload should exist")
}

fn packet_debug(wrapper: &Value) -> Option<&Value> {
    packet_payload(wrapper).get("debug")
}

fn write_cached_coverage_state(root: &Path) {
    let mut coverage = suite_packet_core::CoverageData::new();
    let mut file = suite_packet_core::FileCoverage::new();
    file.lines_instrumented.insert(1);
    file.lines_covered.insert(1);
    coverage.files.insert("src/alpha.rs".to_string(), file);
    let bytes = suite_foundation_core::cache::serialize_coverage(&coverage).unwrap();
    let state_dir = root.join(".covy").join("state");
    fs::create_dir_all(&state_dir).unwrap();
    fs::write(state_dir.join("latest.bin"), bytes).unwrap();
}

fn write_cached_testmap_state(root: &Path) {
    let mut index = suite_packet_core::TestMapIndex::default();
    index.file_to_tests.insert(
        "src/alpha.rs".to_string(),
        ["tests/alpha_test.rs".to_string()].into_iter().collect(),
    );
    let state_dir = root.join(".covy").join("state");
    fs::create_dir_all(&state_dir).unwrap();
    testy_core::pipeline_testmap::write_testmap(&state_dir.join("testmap.bin"), &index).unwrap();
}

fn write_state_event(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
}

#[test]
fn test_suite_cover_check_smoke() {
    let output = suite_cmd()
        .args([
            "cover",
            "check",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.cover.check.v1");
    assert!(packet_payload(&value).get("passed").is_some());
}

#[test]
fn test_suite_cover_check_rich_json_smoke() {
    let output = suite_cmd()
        .args([
            "cover",
            "check",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
            "--packet-detail",
            "rich",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.cover.check.v1");
    assert!(packet_payload(&value).get("violations").is_some());
}

#[test]
fn test_suite_diff_analyze_smoke() {
    suite_cmd()
        .args([
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"passed\""));
}

#[test]
fn test_suite_diff_analyze_governed_smoke() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    write_governed_context(&context);

    let output = suite_cmd()
        .args([
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.diff.analyze.v1");
    assert!(packet_debug(&value)
        .and_then(|v| v.get("governed_packet"))
        .and_then(|v| v.get("tool"))
        .and_then(Value::as_str)
        .is_some());
}

#[test]
fn test_suite_test_impact_smoke() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    write_manifest(&manifest);

    suite_cmd()
        .args([
            "test",
            "map",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .args([
            "test",
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.test.impact.v1");
    assert!(packet_payload(&value)
        .get("result")
        .and_then(|v| v.get("selected_tests"))
        .is_some());
}

#[test]
fn test_suite_test_impact_governed_smoke() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    let context = dir.path().join("context.yaml");
    write_manifest(&manifest);
    write_governed_context(&context);

    suite_cmd()
        .args([
            "test",
            "map",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .args([
            "test",
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.test.impact.v1");
    assert!(packet_debug(&value)
        .and_then(|v| v.get("governed_packet"))
        .and_then(|v| v.get("tool"))
        .and_then(Value::as_str)
        .is_some());
}

#[test]
fn test_suite_diff_analyze_governed_json_metadata_shape() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    write_governed_context(&context);

    let output = suite_cmd()
        .args([
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.diff.analyze.v1");
    assert!(packet_debug(&value)
        .and_then(|v| v.get("kernel_metadata"))
        .and_then(|meta| meta.get("diff"))
        .is_some());
    assert!(packet_debug(&value)
        .and_then(|v| v.get("kernel_metadata"))
        .and_then(|meta| meta.get("governed"))
        .and_then(|governed| governed.get("budget_trim"))
        .is_some());
}

#[test]
fn test_suite_diff_analyze_task_id_propagates_focus_to_map_repo() {
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--task-id",
            "task-diff",
            "--json",
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "map",
            "repo",
            "--repo-root",
            ".",
            "--task-id",
            "task-diff",
            "--json",
            "--packet-detail",
            "rich",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.map.repo.v1");
    let files = value
        .get("packet")
        .and_then(|packet| packet.get("files"))
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(
        files
            .first()
            .and_then(|file| file.get("path"))
            .and_then(Value::as_str),
        Some("src/alpha.rs")
    );
    assert!(
        files[0].get("relevance").and_then(Value::as_f64).unwrap()
            > files[1].get("relevance").and_then(Value::as_f64).unwrap()
    );
}

#[test]
fn test_suite_test_impact_governed_json_metadata_shape() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    let context = dir.path().join("context.yaml");
    write_manifest(&manifest);
    write_governed_context(&context);

    suite_cmd()
        .args([
            "test",
            "map",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .args([
            "test",
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.test.impact.v1");
    assert!(packet_debug(&value)
        .and_then(|v| v.get("kernel_metadata"))
        .and_then(|meta| meta.get("impact"))
        .is_some());
    assert!(packet_debug(&value)
        .and_then(|v| v.get("kernel_metadata"))
        .and_then(|meta| meta.get("governed"))
        .and_then(|governed| governed.get("budget_trim"))
        .is_some());
}

#[test]
fn test_suite_guard_validate_smoke() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    write_guard_context(&context);

    suite_cmd()
        .args(["guard", "validate", "--config", context.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"valid\": true"));
}

#[test]
fn test_suite_guard_validate_with_context_config_flag() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    write_guard_context(&context);

    suite_cmd()
        .args([
            "guard",
            "validate",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"valid\": true"));
}

#[test]
fn test_suite_guard_check_smoke() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    let packet = dir.path().join("packet.json");
    write_guard_context(&context);
    write_guard_packet(&packet);

    let output = suite_cmd()
        .args([
            "guard",
            "check",
            "--packet",
            packet.to_str().unwrap(),
            "--config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.guard.check.v1");
    assert_eq!(
        packet_payload(&value)
            .get("passed")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn test_suite_guard_validate_exit_code_stable_for_invalid_config() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    write_invalid_guard_context(&context);

    suite_cmd()
        .args(["guard", "validate", "--config", context.to_str().unwrap()])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"valid\": false"));
}

#[test]
fn test_suite_guard_check_exit_code_stable_for_denied_packet() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    let packet = dir.path().join("packet.json");
    write_guard_context(&context);
    write_denied_guard_packet(&packet);

    let output = suite_cmd()
        .args([
            "guard",
            "check",
            "--packet",
            packet.to_str().unwrap(),
            "--config",
            context.to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.guard.check.v1");
    assert_eq!(
        packet_payload(&value)
            .get("passed")
            .and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn test_suite_guard_check_detects_wrapped_packet_redaction() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    let packet = dir.path().join("wrapped-packet.json");
    write_redaction_only_context(&context);
    write_wrapped_guard_packet(&packet);

    let output = suite_cmd()
        .args([
            "guard",
            "check",
            "--packet",
            packet.to_str().unwrap(),
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.guard.check.v1");
    assert!(packet_payload(&value)
        .get("findings")
        .and_then(Value::as_array)
        .and_then(|findings| findings.first())
        .and_then(|finding| finding.get("rule"))
        .and_then(Value::as_str)
        .is_some_and(|rule| rule == "redaction"));
}

#[test]
fn test_suite_cover_check_terminal_default() {
    suite_cmd()
        .args([
            "cover",
            "check",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Quality Gate: PASSED"))
        .stdout(predicate::str::contains("\"schema_version\"").not());
}

#[test]
fn test_suite_context_assemble_smoke() {
    let dir = TempDir::new().unwrap();
    let packet_a = dir.path().join("a.json");
    let packet_b = dir.path().join("b.json");
    write_context_packet(
        &packet_a,
        "diffy",
        "Diff gate",
        "critical regression in coverage",
        "src/lib.rs",
    );
    write_context_packet(
        &packet_b,
        "testy",
        "Impact plan",
        "selected tests for src/lib.rs",
        "src/lib.rs",
    );

    let output = suite_cmd()
        .args([
            "context",
            "assemble",
            "--packet",
            packet_a.to_str().unwrap(),
            "--input",
            packet_b.to_str().unwrap(),
            "--budget-tokens",
            "1200",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.context.assemble.v1");
    assert_eq!(
        value
            .get("packet")
            .and_then(|packet| packet.get("tool"))
            .and_then(Value::as_str),
        Some("contextq")
    );
    assert!(packet_payload(&value).get("sections").is_some());
}

#[test]
fn test_suite_context_assemble_governed_smoke() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    let packet_a = dir.path().join("a.json");
    let packet_b = dir.path().join("b.json");
    write_governed_context(&context);
    write_context_packet(
        &packet_a,
        "diffy",
        "Diff gate",
        "critical regression in coverage",
        "src/lib.rs",
    );
    write_context_packet(
        &packet_b,
        "testy",
        "Impact plan",
        "selected tests for src/lib.rs",
        "src/lib.rs",
    );

    let output = suite_cmd()
        .args([
            "context",
            "assemble",
            "--packet",
            packet_a.to_str().unwrap(),
            "--packet",
            packet_b.to_str().unwrap(),
            "--budget-tokens",
            "1200",
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.context.assemble.v1");
    assert!(packet_debug(&value)
        .and_then(|debug| debug.get("kernel_metadata"))
        .and_then(|meta| meta.get("governed"))
        .is_some());
}

#[test]
fn test_suite_context_correlate_emits_v1_findings() {
    let dir = TempDir::new().unwrap();
    let diff = dir.path().join("diff.json");
    let impact = dir.path().join("impact.json");
    let stack = dir.path().join("stack.json");
    let build = dir.path().join("build.json");
    let map = dir.path().join("map.json");

    write_packet_value(
        &diff,
        &json!({
            "version": "1",
            "tool": "diffy",
            "kind": "diff_analyze",
            "hash": "diff-hash",
            "summary": "changed StopWatch",
            "files": [{"path": "src/StopWatch.java", "relevance": 1.0}],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["diff"], "generated_at_unix": 1},
            "payload": {
                "gate_result": {"passed": true, "violations": []},
                "diffs": [{"path": "src/StopWatch.java", "old_path": null, "status": "Modified", "changed_lines": [10, 11]}]
            }
        }),
    );
    write_packet_value(
        &impact,
        &json!({
            "version": "1",
            "tool": "testy",
            "kind": "test_impact",
            "hash": "impact-hash",
            "summary": "impact",
            "files": [],
            "symbols": [{"name": "StopWatchTest#testSplit", "kind": "test_id", "relevance": 1.0}],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["testmap.bin"], "generated_at_unix": 1},
            "payload": {
                "result": {
                    "selected_tests": ["StopWatchTest#testSplit"],
                    "smoke_tests": [],
                    "missing_mappings": [],
                    "confidence": 0.9,
                    "stale": false,
                    "escalate_full_suite": false
                },
                "known_tests": 1,
                "print_command": null
            }
        }),
    );
    write_packet_value(
        &stack,
        &json!({
            "version": "1",
            "tool": "stacky",
            "kind": "stack_slice",
            "hash": "stack-hash",
            "summary": "stack",
            "files": [{"path": "src/ArrayUtils.java", "relevance": 1.0}],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["stack.log"], "generated_at_unix": 1},
            "payload": {
                "schema_version": "stacky.slice.v1",
                "source": "stack.log",
                "total_failures": 1,
                "unique_failures": 1,
                "duplicates_removed": 0,
                "failures": []
            }
        }),
    );
    write_packet_value(
        &build,
        &json!({
            "version": "1",
            "tool": "buildy",
            "kind": "build_reduce",
            "hash": "build-hash",
            "summary": "build",
            "files": [{"path": "src/CharUtils.java", "relevance": 1.0}],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["build.log"], "generated_at_unix": 1},
            "payload": {
                "schema_version": "buildy.reduce.v1",
                "source": "build.log",
                "total_diagnostics": 1,
                "unique_diagnostics": 1,
                "duplicates_removed": 0,
                "groups": [],
                "ordered_fixes": []
            }
        }),
    );
    write_packet_value(
        &map,
        &json!({
            "version": "1",
            "tool": "mapy",
            "kind": "repo_map",
            "hash": "map-hash",
            "summary": "map",
            "files": [
                {"path": "src/StopWatch.java", "relevance": 1.0},
                {"path": "src/ArrayUtils.java", "relevance": 0.8}
            ],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["repo"], "generated_at_unix": 1},
            "payload": {
                "files_ranked": [{"file_idx": 0, "score": 1.0}, {"file_idx": 1, "score": 0.8}],
                "symbols_ranked": [],
                "edges": [],
                "focus_hits": [],
                "truncation": {"files_dropped": 0, "symbols_dropped": 0, "edges_dropped": 0}
            }
        }),
    );

    let output = suite_cmd()
        .args([
            "context",
            "correlate",
            "--packet",
            diff.to_str().unwrap(),
            "--packet",
            impact.to_str().unwrap(),
            "--packet",
            stack.to_str().unwrap(),
            "--packet",
            build.to_str().unwrap(),
            "--packet",
            map.to_str().unwrap(),
            "--task-id",
            "task-correlation",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.context.correlate.v1");
    let findings = packet_payload(&value)
        .get("findings")
        .and_then(Value::as_array)
        .unwrap();
    assert!(findings.len() >= 3);
    assert!(findings
        .iter()
        .any(|finding| { finding.get("relation").and_then(Value::as_str) == Some("unrelated") }));
    assert!(findings
        .iter()
        .any(|finding| { finding.get("relation").and_then(Value::as_str) == Some("supports") }));
    assert!(findings.iter().any(|finding| {
        finding.get("relation").and_then(Value::as_str) == Some("pre_existing_or_unrelated")
    }));
    assert!(findings
        .iter()
        .any(|finding| { finding.get("rule").and_then(Value::as_str) == Some("shared_file") }));
}

#[test]
fn test_suite_governed_local_workflow_smoke() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    let context = dir.path().join("context.yaml");
    let packet_a = dir.path().join("a.json");
    let packet_b = dir.path().join("b.json");

    write_manifest(&manifest);
    write_governed_context(&context);
    write_context_packet(
        &packet_a,
        "diffy",
        "Diff gate",
        "critical regression in coverage",
        "src/lib.rs",
    );
    write_context_packet(
        &packet_b,
        "impact",
        "Impact plan",
        "selected tests for src/lib.rs",
        "src/lib.rs",
    );

    suite_cmd()
        .args(["guard", "validate", "--config", context.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"valid\": true"));

    suite_cmd()
        .args([
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"governed_packet\""))
        .stdout(predicate::str::contains("\"kernel_audit\""));

    suite_cmd()
        .args([
            "test",
            "map",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success();

    suite_cmd()
        .args([
            "test",
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"governed_packet\""))
        .stdout(predicate::str::contains("\"kernel_audit\""));

    let output = suite_cmd()
        .args([
            "context",
            "assemble",
            "--packet",
            packet_a.to_str().unwrap(),
            "--packet",
            packet_b.to_str().unwrap(),
            "--budget-tokens",
            "1200",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.context.assemble.v1");
    assert_eq!(
        value
            .get("packet")
            .and_then(|packet| packet.get("tool"))
            .and_then(Value::as_str),
        Some("contextq")
    );
    assert!(packet_payload(&value).get("assembly").is_some());
}

#[test]
fn test_suite_stack_slice_governed_smoke() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("stack.log");
    let context = dir.path().join("context.yaml");
    write_stack_log(&input);
    write_governed_context(&context);

    let output = suite_cmd()
        .args([
            "stack",
            "slice",
            "--input",
            input.to_str().unwrap(),
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.stack.slice.v1");
    assert!(packet_debug(&value)
        .and_then(|debug| debug.get("kernel_audit"))
        .and_then(|v| v.get("stack"))
        .is_some());
    assert!(packet_debug(&value)
        .and_then(|debug| debug.get("kernel_audit"))
        .and_then(|v| v.get("governed"))
        .is_some());
}

#[test]
fn test_suite_build_reduce_governed_smoke() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("build.log");
    let context = dir.path().join("context.yaml");
    write_build_log(&input);
    write_governed_context(&context);

    let output = suite_cmd()
        .args([
            "build",
            "reduce",
            "--input",
            input.to_str().unwrap(),
            "--json",
            "full",
            "--context-config",
            context.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.build.reduce.v1");
    assert!(packet_debug(&value)
        .and_then(|debug| debug.get("kernel_audit"))
        .and_then(|v| v.get("build"))
        .is_some());
    assert!(packet_debug(&value)
        .and_then(|debug| debug.get("kernel_audit"))
        .and_then(|v| v.get("governed"))
        .is_some());
}

#[test]
fn test_suite_proxy_run_json_smoke() {
    let output = suite_cmd()
        .args(["proxy", "run", "--json", "--", "ls"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.proxy.run.v1");
    assert_eq!(
        value
            .get("packet")
            .and_then(|p| p.get("kind"))
            .and_then(Value::as_str),
        Some("command_summary")
    );
    assert!(value
        .get("packet")
        .and_then(|p| p.get("payload"))
        .and_then(|p| p.get("highlights"))
        .and_then(Value::as_array)
        .map(|v| !v.is_empty())
        .unwrap_or(false));
    assert!(value
        .get("packet")
        .and_then(|p| p.get("payload"))
        .and_then(|p| p.get("output_lines"))
        .is_none());
}

#[test]
fn test_suite_map_repo_json_smoke() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());

    let output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.map.repo.v1");
    assert_eq!(
        value
            .get("packet")
            .and_then(|p| p.get("kind"))
            .and_then(Value::as_str),
        Some("repo_map")
    );
    assert!(value
        .get("packet")
        .and_then(|p| p.get("payload"))
        .and_then(|p| p.get("files_ranked"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("path"))
        .and_then(Value::as_str)
        .is_some());
    assert!(value
        .get("packet")
        .and_then(|p| p.get("payload"))
        .and_then(|p| p.get("symbols_ranked"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("name"))
        .and_then(Value::as_str)
        .is_some());
}

#[test]
fn test_suite_map_repo_cache_flag_writes_kernel_cache_file() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    let cache_file = kernel_cache_file(dir.path());
    assert!(!cache_file.exists());

    suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--cache",
            "--json",
        ])
        .assert()
        .success();

    assert!(cache_file.exists());
    assert!(fs::metadata(cache_file).unwrap().len() > 0);
}

#[test]
fn test_suite_proxy_run_rich_json_smoke() {
    let output = suite_cmd()
        .args([
            "proxy",
            "run",
            "--json",
            "full",
            "--packet-detail",
            "rich",
            "--",
            "ls",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert!(value
        .get("packet")
        .and_then(|p| p.get("payload"))
        .and_then(|p| p.get("output_lines"))
        .and_then(Value::as_array)
        .map(|v| !v.is_empty())
        .unwrap_or(false));
}

#[test]
fn test_suite_proxy_run_cache_flag_writes_kernel_cache_file() {
    let dir = TempDir::new().unwrap();
    let cache_file = kernel_cache_file(dir.path());
    assert!(!cache_file.exists());

    suite_cmd()
        .args([
            "proxy",
            "run",
            "--cache",
            "--cwd",
            dir.path().to_str().unwrap(),
            "--json",
            "--",
            "ls",
        ])
        .assert()
        .success();

    assert!(cache_file.exists());
    assert!(fs::metadata(cache_file).unwrap().len() > 0);
}

#[test]
fn test_suite_map_repo_rich_json_smoke() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());

    let output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json",
            "--packet-detail",
            "rich",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert!(value
        .get("packet")
        .and_then(|p| p.get("payload"))
        .and_then(|p| p.get("files_ranked"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("file_idx"))
        .is_some());
}

#[test]
fn test_suite_output_flag_writes_to_file() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    let out = dir.path().join("map-output.json");

    suite_cmd()
        .args([
            "--output",
            out.to_str().unwrap(),
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    let written = fs::read_to_string(&out).unwrap();
    let value: Value = serde_json::from_str(&written).unwrap();
    assert_eq!(
        value.get("schema_version").and_then(Value::as_str),
        Some("suite.packet.v1")
    );
}

#[test]
fn test_suite_map_repo_rich_governed_section_body_uses_rich_payload() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    let context = dir.path().join("context.yaml");
    write_permissive_context(&context);

    let output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json",
            "full",
            "--packet-detail",
            "rich",
            "--context-config",
            context.to_str().unwrap(),
            "--context-budget-tokens",
            "5000",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    let body = value
        .get("packet")
        .and_then(|v| v.get("payload"))
        .and_then(|v| v.get("debug"))
        .and_then(|v| v.get("governed_packet"))
        .and_then(|v| v.get("payload"))
        .and_then(|v| v.get("sections"))
        .and_then(Value::as_array)
        .and_then(|sections| sections.first())
        .and_then(|section| section.get("body"))
        .and_then(Value::as_str)
        .unwrap();
    assert!(body.contains("\"path\""));
    assert!(!body.contains("file_idx"));
}

#[test]
fn test_suite_proxy_run_rich_governed_section_body_uses_rich_payload() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    write_permissive_context(&context);

    let output = suite_cmd()
        .args([
            "proxy",
            "run",
            "--json",
            "full",
            "--packet-detail",
            "rich",
            "--context-config",
            context.to_str().unwrap(),
            "--context-budget-tokens",
            "5000",
            "--",
            "ls",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    let body = value
        .get("packet")
        .and_then(|v| v.get("payload"))
        .and_then(|v| v.get("debug"))
        .and_then(|v| v.get("governed_packet"))
        .and_then(|v| v.get("payload"))
        .and_then(|v| v.get("sections"))
        .and_then(Value::as_array)
        .and_then(|sections| sections.first())
        .and_then(|section| section.get("body"))
        .and_then(Value::as_str)
        .unwrap();
    assert!(body.contains("\"output_lines\""));
}

#[test]
fn test_compact_packets_respect_byte_slo_and_estimate() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let java_test = workspace.join("JavaTest");
    assert!(java_test.exists(), "JavaTest fixture folder missing");

    let map_output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            java_test.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        map_output.len() <= 2_500,
        "map output exceeded SLO: {}",
        map_output.len()
    );
    let map_value: Value = serde_json::from_slice(&map_output).unwrap();
    let map_packet = map_value.get("packet").unwrap();
    let map_packet_bytes = serde_json::to_vec(map_packet).unwrap().len();
    let map_est_bytes = map_packet
        .get("budget_cost")
        .and_then(|v| v.get("est_bytes"))
        .and_then(Value::as_u64)
        .unwrap() as usize;
    assert_eq!(map_est_bytes, map_packet_bytes);

    let proxy_output = suite_cmd()
        .args(["proxy", "run", "--json", "--", "ls"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        proxy_output.len() <= 2_500,
        "proxy output exceeded SLO: {}",
        proxy_output.len()
    );
    let proxy_value: Value = serde_json::from_slice(&proxy_output).unwrap();
    let proxy_packet = proxy_value.get("packet").unwrap();
    let proxy_packet_bytes = serde_json::to_vec(proxy_packet).unwrap().len();
    let proxy_est_bytes = proxy_packet
        .get("budget_cost")
        .and_then(|v| v.get("est_bytes"))
        .and_then(Value::as_u64)
        .unwrap() as usize;
    assert_eq!(proxy_est_bytes, proxy_packet_bytes);
}

#[test]
fn test_suite_context_store_cli_list_get_prune_stats_json() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());

    suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--cache",
            "--json",
        ])
        .assert()
        .success();

    let stats_output = suite_cmd()
        .args([
            "context",
            "store",
            "stats",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stats: Value = serde_json::from_slice(&stats_output).unwrap();
    assert_eq!(
        stats.get("schema_version").and_then(Value::as_str),
        Some("suite.context.store.stats.v1")
    );
    assert!(
        stats
            .get("stats")
            .and_then(|v| v.get("entries"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            >= 1
    );

    let list_output = suite_cmd()
        .args([
            "context",
            "store",
            "ls",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list: Value = serde_json::from_slice(&list_output).unwrap();
    assert_eq!(
        list.get("schema_version").and_then(Value::as_str),
        Some("suite.context.store.list.v1")
    );
    let entries = list.get("entries").and_then(Value::as_array).unwrap();
    assert!(!entries.is_empty());
    let key = entries
        .first()
        .and_then(|v| v.get("cache_key"))
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    let get_output = suite_cmd()
        .args([
            "context",
            "store",
            "get",
            "--root",
            dir.path().to_str().unwrap(),
            "--key",
            key.as_str(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let get_value: Value = serde_json::from_slice(&get_output).unwrap();
    assert_eq!(
        get_value
            .get("entry")
            .and_then(|v| v.get("entry"))
            .and_then(|v| v.get("cache_key"))
            .and_then(Value::as_str),
        Some(key.as_str())
    );

    let prune_output = suite_cmd()
        .args([
            "context",
            "store",
            "gc",
            "--root",
            dir.path().to_str().unwrap(),
            "--all",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let prune_value: Value = serde_json::from_slice(&prune_output).unwrap();
    assert_eq!(
        prune_value.get("schema_version").and_then(Value::as_str),
        Some("suite.context.store.prune.v1")
    );
    assert!(
        prune_value
            .get("report")
            .and_then(|v| v.get("removed"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            >= 1
    );
    assert!(
        prune_value
            .get("report")
            .and_then(|v| v.get("reasons"))
            .and_then(|v| v.get("manual_prune"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            >= 1
    );
}

#[test]
fn test_suite_context_recall_returns_recent_hits() {
    let dir = TempDir::new().unwrap();
    let packet = dir.path().join("packet.json");
    write_context_packet(
        &packet,
        "diffy",
        "Parser note",
        "missing mappings in parser for src/lib.rs",
        "src/lib.rs",
    );

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "context",
            "assemble",
            "--packet",
            packet.to_str().unwrap(),
            "--cache",
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .args([
            "context",
            "recall",
            "--root",
            dir.path().to_str().unwrap(),
            "--query",
            "mappings parser src/lib.rs",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        value.get("schema_version").and_then(Value::as_str),
        Some("suite.context.recall.v1")
    );
    assert!(value
        .get("hits")
        .and_then(Value::as_array)
        .map(|hits| !hits.is_empty())
        .unwrap_or(false));
}

#[test]
fn test_suite_map_repo_terminal_shows_cache_hit_and_miss() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());

    let first = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--cache",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first_out = String::from_utf8(first).unwrap();
    assert!(first_out.contains("cache: miss"));

    let second = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--cache",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second_out = String::from_utf8(second).unwrap();
    assert!(second_out.contains("cache: hit"));
}

#[test]
fn test_suite_diff_analyze_json_includes_cache_block() {
    let output = suite_cmd()
        .args([
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--cache",
            "--json",
            "full",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert!(value
        .get("packet")
        .and_then(|packet| packet.get("payload"))
        .and_then(|payload| payload.get("debug"))
        .and_then(|debug| debug.get("cache"))
        .and_then(|v| v.get("diff"))
        .and_then(|v| v.get("hit"))
        .and_then(Value::as_bool)
        .is_some());
}

#[test]
fn test_suite_map_repo_profiles_and_handle_fetch_share_hash() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());

    let compact_output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json=compact",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let compact = parse_packet_wrapper(&compact_output, "suite.map.repo.v1");

    let full_output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json=full",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let full = parse_packet_wrapper(&full_output, "suite.map.repo.v1");

    let handle_output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json=handle",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let handle = parse_packet_wrapper(&handle_output, "suite.map.repo.v1");

    let compact_hash = compact
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    assert!(packet_payload(&compact)
        .get("files_ranked")
        .and_then(Value::as_array)
        .and_then(|files| files.first())
        .and_then(|file| file.get("path"))
        .and_then(Value::as_str)
        .is_some());
    assert!(packet_payload(&compact)
        .get("symbols_ranked")
        .and_then(Value::as_array)
        .and_then(|symbols| symbols.first())
        .and_then(|symbol| symbol.get("name"))
        .and_then(Value::as_str)
        .is_some());

    let full_hash = full
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    let handle_hash = handle
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(compact_hash, full_hash);
    assert_eq!(compact_hash, handle_hash);

    let artifact_handle = packet_payload(&handle)
        .get("artifact_handle")
        .cloned()
        .unwrap();
    assert!(packet_payload(&handle)
        .get("files_ranked")
        .and_then(Value::as_array)
        .and_then(|files| files.first())
        .and_then(|file| file.get("path"))
        .and_then(Value::as_str)
        .is_some());
    let handle_id = artifact_handle
        .get("handle_id")
        .and_then(Value::as_str)
        .unwrap();
    let artifact_path = artifact_handle.get("path").and_then(Value::as_str).unwrap();
    assert!(Path::new(artifact_path).exists());

    let fetch_output = suite_cmd()
        .args([
            "packet",
            "fetch",
            "--handle",
            handle_id,
            "--root",
            dir.path().to_str().unwrap(),
            "--json=full",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let fetched = parse_packet_wrapper(&fetch_output, "suite.map.repo.v1");
    let fetched_hash = fetched
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(compact_hash, fetched_hash);
}

#[test]
fn test_suite_proxy_run_profiles_and_handle_fetch_share_hash() {
    let dir = TempDir::new().unwrap();

    let compact_output = suite_cmd()
        .args([
            "proxy",
            "run",
            "--cwd",
            dir.path().to_str().unwrap(),
            "--json=compact",
            "--",
            "ls",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let compact = parse_packet_wrapper(&compact_output, "suite.proxy.run.v1");

    let full_output = suite_cmd()
        .args([
            "proxy",
            "run",
            "--cwd",
            dir.path().to_str().unwrap(),
            "--json=full",
            "--",
            "ls",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let full = parse_packet_wrapper(&full_output, "suite.proxy.run.v1");

    let handle_output = suite_cmd()
        .args([
            "proxy",
            "run",
            "--cwd",
            dir.path().to_str().unwrap(),
            "--json=handle",
            "--",
            "ls",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let handle = parse_packet_wrapper(&handle_output, "suite.proxy.run.v1");

    let compact_hash = compact
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    let full_hash = full
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    let handle_hash = handle
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(compact_hash, full_hash);
    assert_eq!(compact_hash, handle_hash);

    let artifact_handle = packet_payload(&handle)
        .get("artifact_handle")
        .cloned()
        .unwrap();
    let handle_id = artifact_handle
        .get("handle_id")
        .and_then(Value::as_str)
        .unwrap();

    let fetch_output = suite_cmd()
        .args([
            "packet",
            "fetch",
            "--handle",
            handle_id,
            "--root",
            dir.path().to_str().unwrap(),
            "--json=full",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let fetched = parse_packet_wrapper(&fetch_output, "suite.proxy.run.v1");
    let fetched_hash = fetched
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(compact_hash, fetched_hash);
}

#[test]
fn test_suite_cover_check_report_json_compat_maps_to_packet_wrapper() {
    let output = suite_cmd()
        .args([
            "cover",
            "check",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--report",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.cover.check.v1");
    assert!(packet_payload(&value).get("passed").is_some());
}

#[test]
fn test_suite_map_repo_legacy_json_compat_shape() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());

    let output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json",
            "--legacy-json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        value.get("schema_version").and_then(Value::as_str),
        Some("suite.map.repo.v1")
    );
    assert!(value.get("packet_type").is_none());
    assert!(value.get("packet").is_some());
}

#[test]
fn test_suite_context_state_append_then_snapshot() {
    let dir = TempDir::new().unwrap();
    let event_path = dir.path().join("event.json");
    write_state_event(
        &event_path,
        r#"{
  "event_id": "evt-1",
  "occurred_at_unix": 1700000000,
  "actor": "agent",
  "kind": "question_opened",
  "data": {
    "type": "question_opened",
    "question_id": "q1",
    "text": "Does DateUtils call split()?"
  }
}"#,
    );

    let append_output = suite_cmd()
        .args([
            "context",
            "state",
            "append",
            "--task-id",
            "task-demo",
            "--input",
            event_path.to_str().unwrap(),
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let append = parse_packet_wrapper(&append_output, "suite.agent.state.v1");
    assert_eq!(
        packet_payload(&append)
            .get("task_id")
            .and_then(Value::as_str),
        Some("task-demo")
    );

    let snapshot_output = suite_cmd()
        .args([
            "context",
            "state",
            "snapshot",
            "--task-id",
            "task-demo",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let snapshot = parse_packet_wrapper(&snapshot_output, "suite.agent.snapshot.v1");
    assert_eq!(
        packet_payload(&snapshot)
            .get("event_count")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        packet_payload(&snapshot)
            .get("open_questions")
            .and_then(Value::as_array)
            .map(|questions| questions.len()),
        Some(1)
    );
}

#[test]
fn test_suite_context_assemble_task_id_compresses_read_section() {
    let dir = TempDir::new().unwrap();
    let event_path = dir.path().join("event.json");
    let packet_path = dir.path().join("packet.json");
    write_state_event(
        &event_path,
        r#"{
  "event_id": "evt-1",
  "occurred_at_unix": 1700000000,
  "actor": "agent",
  "kind": "file_read",
  "paths": ["src/time/StopWatch.java"],
  "data": {
    "type": "file_read"
  }
}"#,
    );
    fs::write(
        &packet_path,
        r#"{
  "packet_id": "diffy",
  "sections": [
    {
      "title": "Diff",
      "body": "StopWatch.java changed on lines 10-20",
      "refs": [{"kind": "file", "value": "src/time/StopWatch.java"}],
      "relevance": 0.9
    }
  ]
}"#,
    )
    .unwrap();

    suite_cmd()
        .args([
            "context",
            "state",
            "append",
            "--task-id",
            "task-demo",
            "--input",
            event_path.to_str().unwrap(),
            "--root",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "context",
            "assemble",
            "--packet",
            packet_path.to_str().unwrap(),
            "--task-id",
            "task-demo",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.context.assemble.v1");
    let first_body = packet_payload(&value)
        .get("sections")
        .and_then(Value::as_array)
        .and_then(|sections| sections.first())
        .and_then(|section| section.get("body"))
        .and_then(Value::as_str)
        .unwrap();
    assert!(first_body.starts_with("Reminder: already reviewed"));
}

#[test]
#[cfg(unix)]
fn test_suite_daemon_start_status_stop_cycle() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    init_repo(dir.path());

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let status_output = suite_cmd()
        .args([
            "daemon",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: Value = serde_json::from_slice(&status_output).unwrap();
    let expected_root = fs::canonicalize(dir.path()).unwrap();
    assert_eq!(
        status.get("workspace_root").and_then(Value::as_str),
        expected_root.to_str()
    );
    assert!(status.get("pid").and_then(Value::as_u64).unwrap() > 0);
    assert!(status.get("ready_at_unix").and_then(Value::as_u64).unwrap() > 0);
    assert!(status
        .get("log_path")
        .and_then(Value::as_str)
        .is_some_and(|path| Path::new(path).exists()));
    assert!(dir.path().join(".packet28/daemon/ready").exists());
    assert!(dir.path().join(".packet28/daemon/packet28d.log").exists());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[cfg(not(target_os = "linux"))]
#[test]
fn shell_command_reports_linux_only_support_on_macos() {
    let dir = tempfile::tempdir().unwrap();
    suite_cmd()
        .current_dir(dir.path())
        .args(["shell", "--root", "."])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Packet28 shell is only supported on Linux in Phase A",
        ));
}

#[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
#[test]
fn run_command_auto_backend_reports_missing_platform_backend() {
    let dir = tempfile::tempdir().unwrap();
    suite_cmd()
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", "sh", "-c", "printf ok"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Packet28 run --backend linux-oci is not implemented yet",
        ));
}

#[cfg(target_os = "macos")]
#[test]
fn run_command_auto_backend_swaps_instruction_file_and_restores_it() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let original = format!(
        "# Large AGENTS\n\n{}\n",
        (0..120)
            .map(|idx| format!(
                "## Section {idx}\nPacket28 should compress repeated instruction text while keeping task aware guidance."
            ))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    fs::write(dir.path().join("AGENTS.md"), &original).unwrap();

    let claude = dir.path().join("claude");
    fs::write(
        &claude,
        "#!/bin/sh\nprintf '%s|%s\\n' \"$PACKET28_RUNTIME_BACKEND\" \"$PACKET28_AGENT_FAMILY\"\ncat AGENTS.md\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&claude).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&claude, perms).unwrap();
    }

    let output = suite_cmd()
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", claude.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(stdout.contains("macos_swap|claude"));
    assert!(stdout.contains("# [p28:virtual] sha256:"));
    assert_eq!(
        fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(),
        original
    );

    let reports = fs::read_dir(dir.path().join(".packet28/runtime/macos-swap"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 1);
    let report: Value = serde_json::from_slice(&fs::read(&reports[0]).unwrap()).unwrap();
    assert_eq!(
        report.get("state").and_then(Value::as_str),
        Some("restored")
    );
    assert_eq!(
        report.get("backend_kind").and_then(Value::as_str),
        Some("macos_swap")
    );
    let files = report.get("files").and_then(Value::as_array).unwrap();
    assert!(files.iter().any(|item| {
        item.get("path").and_then(Value::as_str) == Some("AGENTS.md")
            && item.get("decision").and_then(Value::as_str) == Some("rewrite")
    }));
}

#[cfg(target_os = "macos")]
#[test]
fn run_command_recovers_stale_macos_swap_session_before_launch() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let original = "tiny original\n";
    let swapped = "# [p28:virtual] stale\n";
    let agents = dir.path().join("AGENTS.md");
    fs::write(&agents, swapped).unwrap();
    let backup = dir.path().join("AGENTS.md.p28-backup.demo");
    let temp = dir.path().join("AGENTS.md.p28-rewrite.demo.tmp");
    fs::write(&backup, original).unwrap();
    fs::write(&temp, "temp").unwrap();
    let report_dir = dir.path().join(".packet28/runtime/macos-swap");
    fs::create_dir_all(&report_dir).unwrap();
    fs::write(
        report_dir.join("demo.json"),
        serde_json::to_vec_pretty(&json!({
            "session_id":"demo",
            "workspace_root": dir.path(),
            "command":["claude"],
            "agent_family":"claude",
            "backend_kind":"macos_swap",
            "pid":999999u32,
            "started_at":1u64,
            "state":"active",
            "files":[{
                "path":"AGENTS.md",
                "decision":"rewrite",
                "reason":null,
                "content_sha256":"abc",
                "task_label":"default",
                "original_bytes":swapped.len(),
                "rewritten_bytes":swapped.len(),
                "backup_path":backup,
                "temp_path":temp
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let claude = dir.path().join("claude");
    fs::write(&claude, "#!/bin/sh\ncat AGENTS.md\n").unwrap();
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&claude).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&claude, perms).unwrap();
    }

    let output = suite_cmd()
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", claude.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert_eq!(stdout, original);
    assert_eq!(fs::read_to_string(&agents).unwrap(), original);

    let recovered: Value =
        serde_json::from_slice(&fs::read(report_dir.join("demo.json")).unwrap()).unwrap();
    assert_eq!(
        recovered.get("state").and_then(Value::as_str),
        Some("restored")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn run_command_restores_files_after_sigterm() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let original = format!(
        "# Large AGENTS\n\n{}\n",
        (0..80)
            .map(|idx| format!(
                "## Section {idx}\nPacket28 should compress repeated instruction text while keeping task aware guidance."
            ))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    let agents = dir.path().join("AGENTS.md");
    fs::write(&agents, &original).unwrap();

    let claude = dir.path().join("claude");
    fs::write(
        &claude,
        "#!/bin/sh\nprintf '%s' \"$PACKET28_RUNTIME_BACKEND\" > child-backend.txt\nwhile true; do sleep 1; done\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&claude).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&claude, perms).unwrap();
    }

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", claude.to_str().unwrap()])
        .spawn()
        .unwrap();

    let report_dir = dir.path().join(".packet28/runtime/macos-swap");
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if report_dir.exists()
            && fs::read_dir(&report_dir)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| {
                    serde_json::from_slice::<Value>(&fs::read(entry.path()).unwrap())
                        .ok()
                        .and_then(|report| {
                            report
                                .get("state")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                        .is_some_and(|state| state == "active")
                })
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let status = child.wait().unwrap();
    assert!(!status.success());
    let restore_start = std::time::Instant::now();
    loop {
        if fs::read_to_string(&agents).ok().as_deref() == Some(original.as_str()) {
            break;
        }
        assert!(
            restore_start.elapsed() < Duration::from_secs(3),
            "timed out waiting for AGENTS.md to be restored"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn shell_command_injects_ld_preload_for_explicit_commands() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("cargo")
        .args(["build", "-p", "context-instruct-shim"])
        .status()
        .unwrap();
    assert!(status.success(), "failed to build context-instruct-shim");

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "shell",
            "--root",
            ".",
            "--",
            "sh",
            "-c",
            "printf '%s' \"$LD_PRELOAD\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(stdout.contains("libcontext_instruct_shim.so"));
}

#[cfg(target_os = "linux")]
#[test]
fn run_command_linux_preload_sets_backend_and_agent_family() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    fs::write(
        &claude,
        "#!/bin/sh\nprintf '%s|%s|%s' \"$LD_PRELOAD\" \"$PACKET28_RUNTIME_BACKEND\" \"$PACKET28_AGENT_FAMILY\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&claude).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&claude, perms).unwrap();
    }
    let status = std::process::Command::new("cargo")
        .args(["build", "-p", "context-instruct-shim"])
        .status()
        .unwrap();
    assert!(status.success(), "failed to build context-instruct-shim");

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "run",
            "--root",
            ".",
            "--backend",
            "linux-preload",
            "--",
            claude.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(stdout.contains("libcontext_instruct_shim.so"));
    assert!(stdout.contains("linux_preload"));
    assert!(stdout.contains("claude"));
}

#[test]
#[cfg(unix)]
fn test_suite_daemon_index_rebuild_and_status() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    init_repo(dir.path());

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let rebuild_output = suite_cmd()
        .args([
            "daemon",
            "index",
            "rebuild",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rebuild: Value = serde_json::from_slice(&rebuild_output).unwrap();
    assert_eq!(rebuild.get("accepted").and_then(Value::as_bool), Some(true));
    assert_eq!(rebuild.get("full").and_then(Value::as_bool), Some(true));

    let start = std::time::Instant::now();
    let mut ready = false;
    while start.elapsed() < Duration::from_secs(5) {
        let status_output = suite_cmd()
            .args([
                "daemon",
                "index",
                "status",
                "--root",
                dir.path().to_str().unwrap(),
                "--json",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let status: Value = serde_json::from_slice(&status_output).unwrap();
        if status.get("ready").and_then(Value::as_bool) == Some(true) {
            ready = true;
            assert!(
                status
                    .get("manifest")
                    .and_then(|manifest| manifest.get("indexed_files"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
            );
            assert!(
                status
                    .get("manifest")
                    .and_then(|manifest| manifest.get("regex_weight_table_version"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
            );
            assert_eq!(
                status
                    .get("manifest")
                    .and_then(|manifest| manifest.get("regex_status"))
                    .and_then(Value::as_str),
                Some("ready")
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ready, "expected daemon index to become ready");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[cfg(unix)]
fn seed_checkpointed_handoff_task(
    dir: &Path,
    task_id: &str,
    intention_text: &str,
    _checkpoint_id: &str,
) {
    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir);
    initialize_mcp_session(&mut stdin, &mut stdout);
    let _ = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        2,
        task_id,
        intention_text,
        "investigating",
        &["src/alpha.rs"],
    );
    let _ = child.kill();
    let _ = child.wait();
    let (status, _) = run_claude_hook(
        dir,
        &json!({
            "hook_event_name":"Stop",
            "task_id":task_id,
            "session_id": format!("session-{task_id}"),
        }),
    );
    assert_eq!(status, 0);
}

#[test]
#[cfg(unix)]
fn test_packet28_agent_bootstraps_broker_session() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    let task_text = "design auth broker";
    let task_id = suite_cli::broker_client::derive_task_id(task_text);

    let output = agent_cmd()
        .current_dir(dir.path())
        .args([
            "--task",
            task_text,
            "--",
            "sh",
            "-c",
            "printf '%s\n%s\n%s\n%s\n%s\n%s\n%s\n%s\n' \"$PACKET28_BOOTSTRAP_MODE\" \"$PACKET28_TASK_ID\" \"$PACKET28_BROKER_BRIEF_PATH\" \"$PACKET28_BROKER_STATE_PATH\" \"$PACKET28_MCP_COMMAND\" \"$PACKET28_BROKER_WINDOW_MODE\" \"$PACKET28_BROKER_SUPERSESSION\" \"$PACKET28_BROKER_PREPARE_HANDOFF_TOOL\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let lines = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 8);
    assert_eq!(lines[0], "fresh");
    assert_eq!(lines[1], task_id);
    assert!(Path::new(&lines[2]).exists(), "brief path should exist");
    assert!(Path::new(&lines[3]).exists(), "state path should exist");
    assert!(lines[4].contains("Packet28 mcp serve --root"));
    assert_eq!(lines[5], "replace");
    assert_eq!(lines[6], "1");
    assert_eq!(lines[7], "packet28.prepare_handoff");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_agent_resumes_from_checkpoint_handoff() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-handoff-agent",
        "Resume from checkpointed Alpha investigation",
        "cp-agent-1",
    );

    let output = agent_cmd()
        .current_dir(dir.path())
        .args([
            "--wait-for-handoff",
            "--handoff-timeout-secs",
            "5",
            "--task-id",
            "task-handoff-agent",
            "--",
            "sh",
            "-c",
            "printf '%s\n%s\n%s\n%s\n%s\n%s\n' \"$PACKET28_BOOTSTRAP_MODE\" \"$PACKET28_BOOTSTRAP_PATH\" \"$PACKET28_HANDOFF_PATH\" \"$PACKET28_HANDOFF_ARTIFACT_ID\" \"$PACKET28_HANDOFF_CHECKPOINT_ID\" \"$PACKET28_BROKER_PREPARE_HANDOFF_TOOL\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let lines = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 6);
    assert_eq!(lines[0], "handoff");
    assert!(Path::new(&lines[1]).exists(), "bootstrap path should exist");
    assert!(Path::new(&lines[2]).exists(), "handoff path should exist");
    assert!(
        !lines[3].is_empty(),
        "handoff artifact id should be exported"
    );
    assert!(lines[4].is_empty());
    assert_eq!(lines[5], "packet28.prepare_handoff");

    let bootstrap: Value = serde_json::from_str(&fs::read_to_string(&lines[1]).unwrap()).unwrap();
    assert_eq!(
        bootstrap["latest_intention"]["text"],
        "Resume from checkpointed Alpha investigation"
    );
    assert_eq!(bootstrap["response_mode"], "full");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_agent_wait_for_handoff_times_out_when_checkpoint_missing() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    agent_cmd()
        .current_dir(dir.path())
        .args([
            "--wait-for-handoff",
            "--handoff-timeout-secs",
            "1",
            "--handoff-poll-ms",
            "50",
            "--task-id",
            "task-timeout-handoff",
            "--",
            "sh",
            "-c",
            "exit 0",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "timed out waiting for Packet28 handoff",
        ));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_daemon_task_await_handoff_reports_ready_status() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-await",
        "Prepare daemon-owned handoff wait",
        "cp-daemon-1",
    );

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-await",
            "--timeout-ms",
            "1000",
            "--poll-ms",
            "50",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["task_status"]["handoff_ready"], true);
    assert!(value["waited_ms"].as_u64().unwrap() <= 1_000);
    assert!(value["polls"].as_u64().unwrap() >= 1);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_daemon_task_launch_agent_spawns_child_from_handoff() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-launch",
        "Launch fresh worker from daemon",
        "cp-daemon-launch-1",
    );

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "launch-agent",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-launch",
            "--json",
            "--",
            "sh",
            "-c",
            "printf '%s\\n%s\\n' \"$PACKET28_BOOTSTRAP_MODE\" \"$PACKET28_TASK_ID\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launch_value: Value = serde_json::from_slice(&output).unwrap();
    let log_path = launch_value["log_path"].as_str().unwrap();
    assert_eq!(launch_value["bootstrap_mode"], "handoff");
    assert!(launch_value["pid"].as_u64().unwrap() > 0);

    let mut log_contents = String::new();
    for _ in 0..40 {
        if let Ok(raw) = fs::read_to_string(log_path) {
            log_contents = raw;
            if log_contents.contains("handoff") && log_contents.contains("task-daemon-launch") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(log_contents.contains("handoff"));
    assert!(log_contents.contains("task-daemon-launch"));

    let status_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-launch",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_value: Value = serde_json::from_slice(&status_output).unwrap();
    assert_eq!(status_value["latest_agent_bootstrap_mode"], "handoff");
    assert_eq!(
        status_value["latest_agent_pid"].as_u64().unwrap(),
        launch_value["pid"].as_u64().unwrap()
    );
    assert_eq!(status_value["latest_agent_log_path"], log_path);
    assert_eq!(
        status_value["latest_agent_handoff_artifact_id"],
        launch_value["handoff_artifact_id"]
    );
    assert_eq!(
        status_value["latest_agent_handoff_checkpoint_id"],
        launch_value["handoff_checkpoint_id"]
    );
    assert!(status_value["latest_agent_context_version"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_daemon_task_await_handoff_can_require_newer_context_version() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    seed_checkpointed_handoff_task(
        dir.path(),
        "task-daemon-newer-handoff",
        "Prepare initial handoff",
        "cp-daemon-newer-1",
    );
    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    let launch_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "launch-agent",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--json",
            "--",
            "sh",
            "-c",
            "printf '%s\\n' \"$PACKET28_BOOTSTRAP_MODE\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launch_value: Value = serde_json::from_slice(&launch_output).unwrap();
    let launched_context_version = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let launched_status: Value = serde_json::from_slice(&launched_context_version).unwrap();
    let previous_context_version = launched_status["latest_agent_context_version"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(launch_value["bootstrap_mode"], "handoff");

    suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--after-context-version",
            &previous_context_version,
            "--timeout-ms",
            "100",
            "--poll-ms",
            "20",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "newer handoff than context version",
        ));

    let _ = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        4,
        "task-daemon-newer-handoff",
        "Resume from a newer handoff",
        "editing",
        &["src/beta.rs"],
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreCompact",
            "task_id":"task-daemon-newer-handoff",
            "session_id":"session-daemon-newer-handoff",
        }),
    );
    assert_eq!(status, 0);

    let output = suite_cmd()
        .args([
            "daemon",
            "task",
            "await-handoff",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-daemon-newer-handoff",
            "--after-context-version",
            &previous_context_version,
            "--timeout-ms",
            "1000",
            "--poll-ms",
            "50",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["task_status"]["handoff_ready"], true);
    assert_ne!(
        value["task_status"]["latest_context_version"]
            .as_str()
            .unwrap(),
        previous_context_version
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_doctor_reports_healthy_stack() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    git(dir.path(), &["add", "src/alpha.rs", "src/beta.rs"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "init",
        ],
    );
    write_cached_coverage_state(dir.path());
    write_cached_testmap_state(dir.path());
    fs::write(
        dir.path().join(".mcp.json"),
        json!({
            "mcpServers": {
                "packet28": {
                    "command": "packet28-mcp",
                    "args": ["--root", dir.path().to_str().unwrap()]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    for _ in 0..2 {
        let output = suite_cmd()
            .current_dir(dir.path())
            .args(["doctor", "--root", dir.path().to_str().unwrap(), "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let payload: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(payload["daemon"]["ok"], true);
        assert_eq!(payload["index"]["ok"], true);
        assert!(payload["mcp_config"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["packet28_configured"] == true));
        assert_eq!(payload["handshake"]["ok"], true);
        assert_eq!(payload["reducer_round_trip"]["ok"], true);
        assert!(payload.get("push_notifications").is_some());
        assert_eq!(payload["handoff_round_trip"]["ok"], true);
    }

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_prepare_handoff_requires_checkpoint_and_persists_artifact() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());

    initialize_mcp_session(&mut stdin, &mut stdout);
    let intention = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        2,
        "task-handoff",
        "Inspect Alpha before editing it",
        "investigating",
        &["src/alpha.rs"],
    );
    assert_eq!(intention["result"]["structuredContent"]["accepted"], true);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.prepare_handoff",
                "arguments":{
                    "task_id":"task-handoff"
                }
            }
        }),
    );
    let not_ready = read_mcp_message_for_id(&mut stdout, 3);
    let not_ready_payload = &not_ready["result"]["structuredContent"];
    assert_eq!(not_ready_payload["handoff_ready"], false);
    assert!(not_ready_payload["context"].is_null());

    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"Stop",
            "task_id":"task-handoff",
            "session_id":"session-task-handoff",
        }),
    );
    assert_eq!(status, 0);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.prepare_handoff",
                "arguments":{
                    "task_id":"task-handoff",
                    "response_mode":"slim"
                }
            }
        }),
    );
    let handoff = read_mcp_message_for_id(&mut stdout, 4);
    let handoff_payload = &handoff["result"]["structuredContent"];
    assert_eq!(handoff_payload["handoff_ready"], true);
    assert!(handoff_payload["latest_checkpoint_id"].is_null());
    assert_eq!(
        handoff_payload["latest_intention"]["text"],
        "Inspect Alpha before editing it"
    );
    let handoff_context = &handoff_payload["context"];
    assert_eq!(handoff_context["response_mode"], "slim");
    assert_eq!(handoff_context["handoff_ready"], true);
    assert!(handoff_context["brief"]
        .as_str()
        .unwrap()
        .contains("Latest Intention"));
    let handoff_artifact_id = handoff_context["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28.fetch_context",
                "arguments":{
                    "task_id":"task-handoff",
                    "artifact_id": handoff_artifact_id
                }
            }
        }),
    );
    let fetched = read_mcp_message_for_id(&mut stdout, 5);
    let fetched_payload = &fetched["result"]["structuredContent"];
    assert_eq!(fetched_payload["response_mode"], "full");
    assert_eq!(
        fetched_payload["latest_intention"]["step_id"],
        "investigating"
    );
    assert!(fetched_payload["sections"]
        .as_array()
        .unwrap()
        .iter()
        .any(|section| section["id"] == "agent_intention"));

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"packet28.task_status",
                "arguments":{
                    "task_id":"task-handoff"
                }
            }
        }),
    );
    let status = read_mcp_message_for_id(&mut stdout, 6);
    let status_payload = &status["result"]["structuredContent"];
    assert_eq!(status_payload["handoff_ready"], true);
    assert!(status_payload["latest_handoff_checkpoint_id"].is_null());
    assert_eq!(
        status_payload["latest_handoff_artifact_id"],
        handoff_context["artifact_id"]
    );

    let (resume_status, resume_output) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"SessionStart",
            "task_id":"task-handoff",
            "session_id":"session-task-handoff-resume",
            "cwd": dir.path().display().to_string(),
        }),
    );
    assert_eq!(resume_status, 0);
    let resume_payload: Value = serde_json::from_str(&resume_output).unwrap();
    let additional_context = resume_payload["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(additional_context.contains("Packet28 Context v"));
    assert!(additional_context.contains("Latest Intention"));

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_session_start_injects_wakeup_pack() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    let project = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Session start should inject this Packet28 wakeup fact",
            "--project",
            &project,
            "--topic",
            "session-start",
            "--importance",
            "critical",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Session start has a second Packet28 wakeup fact that proves budgeted hook packs truncate deterministically",
            "--project",
            &project,
            "--topic",
            "session-start",
            "--importance",
            "high",
            "--json",
        ])
        .assert()
        .success();

    let payload = json!({
        "hook_event_name":"SessionStart",
        "task_id":"task-wakeup-hook",
        "session_id":"session-wakeup-hook",
        "cwd": dir.path().display().to_string(),
    });
    let (status, stdout, stderr) = run_hook_raw_with_env(
        "claude",
        dir.path(),
        &serde_json::to_string(&payload).unwrap(),
        &[("HOME", home.path().as_os_str())],
    );
    assert_eq!(status, 0, "stderr={stderr}");
    let rendered: Value = serde_json::from_str(&stdout).unwrap();
    let additional_context = rendered["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(additional_context.contains("Packet28 Wake-Up Pack"));
    assert!(additional_context.contains("Session start should inject this Packet28 wakeup fact"));
    assert!(additional_context.contains("Critical memories"));
    let (budget_status, budget_stdout, budget_stderr) = run_hook_raw_with_env(
        "claude",
        dir.path(),
        &serde_json::to_string(&payload).unwrap(),
        &[
            ("HOME", home.path().as_os_str()),
            ("PACKET28_HOOK_WAKEUP_TOKENS", std::ffi::OsStr::new("12")),
        ],
    );
    assert_eq!(budget_status, 0, "stderr={budget_stderr}");
    let budget_rendered: Value = serde_json::from_str(&budget_stdout).unwrap();
    let budget_context = budget_rendered["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(budget_context.contains("Packet28 Wake-Up Pack"));
    assert!(budget_context.contains("budget:"));
    assert!(budget_context.contains("truncated"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_write_intention_derives_task_id_from_full_text() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    let intention_text = "Investigate parser regression in the handoff pipeline";
    let derived_task_id = suite_cli::broker_client::derive_task_id(intention_text);
    let response = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        2,
        "",
        intention_text,
        "investigating",
        &["crates/packet28d/src/hooks.rs"],
    );
    assert_eq!(response["result"]["structuredContent"]["accepted"], true);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.task_status",
                "arguments":{
                    "task_id": derived_task_id
                }
            }
        }),
    );
    let status = read_mcp_message_for_id(&mut stdout, 3);
    assert_eq!(
        status["result"]["structuredContent"]["task"]["task_id"],
        derived_task_id
    );

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_native_read_auto_captures_regions() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());
    git(dir.path(), &["add", "src/alpha.rs", "src/beta.rs"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "init",
        ],
    );
    write_cached_coverage_state(dir.path());
    write_cached_testmap_state(dir.path());

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);
    let _ = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        2,
        "task-native-read",
        "Locate the Alpha definition",
        "investigating",
        &["src/alpha.rs"],
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PostToolUse",
            "task_id":"task-native-read",
            "session_id":"session-native-read",
            "tool_name":"Read",
            "tool_input":{"file_path":"src/alpha.rs","offset":4,"limit":1},
            "tool_response":{"content":"fn alpha() {}\nstruct Alpha;\n","symbols":["Alpha"],"regions":["src/alpha.rs:4-5"]}
        }),
    );
    assert_eq!(status, 0);
    let (status, _) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"Stop",
            "task_id":"task-native-read",
            "session_id":"session-native-read",
        }),
    );
    assert_eq!(status, 0);

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.prepare_handoff",
                "arguments":{
                    "task_id":"task-native-read",
                    "query":"Where is Alpha defined?",
                    "response_mode":"full"
                }
            }
        }),
    );
    let inspect = read_mcp_message_for_id(&mut stdout, 3);
    let inspect_payload = &inspect["result"]["structuredContent"]["context"];
    assert!(inspect["result"]["structuredContent"]["handoff_ready"]
        .as_bool()
        .unwrap());
    assert!(inspect_payload["recent_tool_invocations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["tool_name"] == "Read"
                && item["regions"].as_array().is_some_and(|regions| {
                    regions.iter().any(|region| region == "src/alpha.rs:4-5")
                })
        }));
    assert!(inspect_payload["discovered_paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item == "src/alpha.rs"));
    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_native_tools_return_slim_results_and_fetch_full_artifacts() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());
    initialize_mcp_session(&mut stdin, &mut stdout);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"packet28.search",
                "arguments":{
                    "task_id":"task-native-tools",
                    "query":"Alpha",
                    "response_mode":"slim"
                }
            }
        }),
    );
    let search = read_mcp_message_for_id(&mut stdout, 2);
    let search_payload = &search["result"]["structuredContent"];
    assert_eq!(search_payload["response_mode"], "slim");
    assert!(search_payload["artifact_id"].as_str().is_some());
    assert!(search_payload["match_count"].as_u64().unwrap() >= 1);
    assert_eq!(search_payload["search_strategy"], "hybrid");
    assert!(search_payload["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == "src/alpha.rs"));
    assert!(search_payload["regions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|region| region
            .as_str()
            .is_some_and(|value| value.starts_with("src/alpha.rs:"))));
    assert!(search_payload["engine"].is_object());
    assert!(search_payload["hybrid"].is_object());
    let search_artifact = search_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.fetch_tool_result",
                "arguments":{
                    "task_id":"task-native-tools",
                    "artifact_id": search_artifact
                }
            }
        }),
    );
    let search_full = read_mcp_message_for_id(&mut stdout, 3);
    let search_full_payload = &search_full["result"]["structuredContent"];
    assert_eq!(search_full_payload["response_mode"], "full");
    assert_eq!(search_full_payload["query"], "Alpha");
    assert_eq!(search_full_payload["search_strategy"], "hybrid");
    assert!(!search_full_payload["groups"].as_array().unwrap().is_empty());
    assert!(search_full_payload["engine"].is_object());
    assert!(search_full_payload["hybrid"].is_object());

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"packet28.read_regions",
                "arguments":{
                    "task_id":"task-native-tools",
                    "path":"src/alpha.rs",
                    "line_start":1,
                    "line_end":2,
                    "response_mode":"slim"
                }
            }
        }),
    );
    let read_regions = read_mcp_message_for_id(&mut stdout, 4);
    let read_payload = &read_regions["result"]["structuredContent"];
    assert_eq!(read_payload["response_mode"], "slim");
    assert!(read_payload["artifact_id"].as_str().is_some());
    let read_artifact = read_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"packet28.fetch_tool_result",
                "arguments":{
                    "task_id":"task-native-tools",
                    "artifact_id": read_artifact
                }
            }
        }),
    );
    let read_full = read_mcp_message_for_id(&mut stdout, 5);
    let read_full_payload = &read_full["result"]["structuredContent"];
    assert_eq!(read_full_payload["response_mode"], "full");
    assert_eq!(read_full_payload["path"], "src/alpha.rs");
    assert_eq!(read_full_payload["lines"].as_array().unwrap().len(), 2);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"packet28.glob",
                "arguments":{
                    "task_id":"task-native-tools",
                    "pattern":"src/*.rs",
                    "response_mode":"slim"
                }
            }
        }),
    );
    let glob = read_mcp_message_for_id(&mut stdout, 6);
    let glob_payload = &glob["result"]["structuredContent"];
    assert_eq!(glob_payload["response_mode"], "slim");
    assert!(glob_payload["artifact_id"].as_str().is_some());
    let glob_artifact = glob_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":7,
            "method":"tools/call",
            "params":{
                "name":"packet28.fetch_tool_result",
                "arguments":{
                    "task_id":"task-native-tools",
                    "artifact_id": glob_artifact
                }
            }
        }),
    );
    let glob_full = read_mcp_message_for_id(&mut stdout, 7);
    let glob_full_payload = &glob_full["result"]["structuredContent"];
    assert_eq!(glob_full_payload["response_mode"], "full");
    assert_eq!(glob_full_payload["pattern"], "src/*.rs");
    assert!(glob_full_payload["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == "src/alpha.rs"));

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_doctor_reports_healthy_runtime() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let output = suite_cmd()
        .args(["doctor", "--root", dir.path().to_str().unwrap(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["daemon"]["ok"], true);
    assert_eq!(report["handshake"]["ok"], true);
    assert_eq!(report["reducer_round_trip"]["ok"], true);
    assert_eq!(report["handoff_round_trip"]["ok"], true);

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_reducer_runner_reuses_cached_summary_without_rerunning_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("sample.txt"), "Alpha\nBeta\n").unwrap();

    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let counter_path = dir.path().join("cat-count.txt");
    fs::write(&counter_path, "0\n").unwrap();
    let script_path = bin_dir.join("cat");
    fs::write(
        &script_path,
        format!(
            "#!/bin/sh\ncount=$(/bin/cat \"{count}\" 2>/dev/null || echo 0)\ncount=$((count + 1))\nprintf '%s\\n' \"$count\" > \"{count}\"\nexec /bin/cat \"$@\"\n",
            count = counter_path.display()
        ),
    )
    .unwrap();
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();

    let spec = packet28_reducer_core::classify_command("cat sample.txt").unwrap();
    let path_env = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut first = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    first.current_dir(dir.path()).env("PATH", &path_env).args([
        "hook",
        "reducer-runner",
        "--root",
        dir.path().to_str().unwrap(),
        "--task-id",
        "task-runner-cache",
        "--family",
        &spec.family,
        "--kind",
        &spec.canonical_kind,
        "--fingerprint",
        &spec.cache_fingerprint,
        "--cwd",
        dir.path().to_str().unwrap(),
        "--",
        "cat",
        "sample.txt",
    ]);
    let first = first.output().unwrap();
    assert!(first.status.success());

    let mut second = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    second.current_dir(dir.path()).env("PATH", &path_env).args([
        "hook",
        "reducer-runner",
        "--root",
        dir.path().to_str().unwrap(),
        "--task-id",
        "task-runner-cache",
        "--family",
        &spec.family,
        "--kind",
        &spec.canonical_kind,
        "--fingerprint",
        &spec.cache_fingerprint,
        "--cwd",
        dir.path().to_str().unwrap(),
        "--",
        "cat",
        "sample.txt",
    ]);
    let second = second.output().unwrap();
    assert!(second.status.success());
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(fs::read_to_string(&counter_path).unwrap().trim(), "1");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hooks_degrade_gracefully_on_bad_json_and_no_rewrite() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());

    let (status, stdout, stderr) = run_hook_raw("claude", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout, stderr) = run_hook_raw("cursor", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout, stderr) = run_hook_raw("copilot", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout, stderr) = run_hook_raw("gemini", dir.path(), "{not json");
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("malformed JSON"));

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-no-rewrite",
            "session_id":"session-pretool-no-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"definitely-unsupported-packet28-tool --flag"}
        }),
    );
    assert_eq!(status, 0);
    assert!(matches!(stdout.trim(), "" | "{}"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_git_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-rewrite",
            "session_id":"session-pretool-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"git status --short src/alpha.rs"}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family git"));
    assert!(rewritten.contains("--kind git_status"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_is_idempotent_and_ignores_non_bash_tools() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let base_payload = json!({
        "hook_event_name":"PreToolUse",
        "task_id":"task-pretool-idempotent",
        "session_id":"session-pretool-idempotent",
        "cwd":dir.path().to_str().unwrap(),
        "tool_name":"Bash",
        "tool_input":{"command":"git status --short src/alpha.rs"}
    });
    let (status, stdout) = run_claude_hook(dir.path(), &base_payload);
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-idempotent",
            "session_id":"session-pretool-idempotent",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command": rewritten}
        }),
    );
    assert_eq!(status, 0);
    assert_eq!(stdout.trim(), "{}");

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-non-bash",
            "session_id":"session-pretool-idempotent",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Read",
            "tool_input":{"file_path":"src/alpha.rs"}
        }),
    );
    assert_eq!(status, 0);
    assert_eq!(stdout.trim(), "{}");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_cursor_hook_pretool_rewrites_and_returns_empty_json_on_noop() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let payloads = [
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "command":"git status --short src/alpha.rs"
        }),
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-tool-input-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"command":"git status --short src/alpha.rs"}
        }),
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-command-line-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "command_line":"git status --short src/alpha.rs"
        }),
        json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-shell-command-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "shell_command":"git status --short src/alpha.rs"
        }),
    ];
    let mut first_rewritten = String::new();
    for payload in payloads {
        let (status, stdout, _stderr) = run_hook_raw(
            "cursor",
            dir.path(),
            &serde_json::to_string(&payload).unwrap(),
        );
        assert_eq!(status, 0);
        let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(rendered["permission"].as_str(), Some("allow"));
        let rewritten = rendered["updated_input"]["command"].as_str().unwrap();
        assert!(rewritten.contains("hook reducer-runner"));
        assert!(rewritten.contains("--family git"));
        assert!(rewritten.contains("--kind git_status"));
        if first_rewritten.is_empty() {
            first_rewritten = rewritten.to_string();
        }
    }

    let (status, stdout, _stderr) = run_hook_raw(
        "cursor",
        dir.path(),
        &serde_json::to_string(&json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-idempotent",
            "cwd":dir.path().to_str().unwrap(),
            "command":first_rewritten
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    assert_eq!(stdout.trim(), "{}");

    let (status, stdout, _stderr) = run_hook_raw(
        "cursor",
        dir.path(),
        &serde_json::to_string(&json!({
            "hook_event_name":"beforeShellExecution",
            "conversation_id":"cursor-session-noop",
            "cwd":dir.path().to_str().unwrap(),
            "command":"definitely-unsupported-packet28-tool --flag"
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    assert_eq!(stdout.trim(), "{}");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_gemini_hook_before_tool_rewrites_shell_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout, _stderr) = run_hook_raw(
        "gemini",
        dir.path(),
        &serde_json::to_string(&json!({
            "tool_name":"run_shell_command",
            "session_id":"gemini-session-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"command":"git status --short src/alpha.rs"}
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(rendered["decision"].as_str(), Some("allow"));
    let rewritten = rendered["hookSpecificOutput"]["tool_input"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family git"));
    assert!(rewritten.contains("--kind git_status"));

    let (status, stdout, _stderr) = run_hook_raw(
        "gemini",
        dir.path(),
        &serde_json::to_string(&json!({
            "tool_name":"read_file",
            "session_id":"gemini-session-noop",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"path":"src/alpha.rs"}
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(rendered["decision"].as_str(), Some("allow"));
    assert!(rendered.get("hookSpecificOutput").is_none());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_copilot_hook_rewrites_vscode_and_denies_cli_with_suggestion() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout, _stderr) = run_hook_raw(
        "copilot",
        dir.path(),
        &serde_json::to_string(&json!({
            "tool_name":"Bash",
            "session_id":"copilot-vscode-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_input":{"command":"git status --short src/alpha.rs"}
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        rendered["hookSpecificOutput"]["permissionDecision"].as_str(),
        Some("allow")
    );
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family git"));
    assert!(rewritten.contains("--kind git_status"));

    let tool_args = serde_json::to_string(&json!({
        "command":"git status --short src/alpha.rs"
    }))
    .unwrap();
    let (status, stdout, _stderr) = run_hook_raw(
        "copilot",
        dir.path(),
        &serde_json::to_string(&json!({
            "toolName":"bash",
            "toolArgs":tool_args
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(rendered["permissionDecision"].as_str(), Some("deny"));
    let reason = rendered["permissionDecisionReason"].as_str().unwrap();
    assert!(reason.contains("hook reducer-runner"));
    assert!(reason.contains("Packet28"));

    let (status, stdout, _stderr) = run_hook_raw(
        "copilot",
        dir.path(),
        &serde_json::to_string(&json!({
            "toolName":"view",
            "toolArgs":"{}"
        }))
        .unwrap(),
    );
    assert_eq!(status, 0);
    assert!(stdout.trim().is_empty());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_github_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-gh-rewrite",
            "session_id":"session-pretool-gh-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"gh pr list --limit 5"}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family github"));
    assert!(rewritten.contains("--kind gh_pr_list"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_python_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-python-rewrite",
            "session_id":"session-pretool-python-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"python3 -m pytest tests"}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family python"));
    assert!(rewritten.contains("--kind python_pytest"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_javascript_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-js-rewrite",
            "session_id":"session-pretool-js-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"npx tsc --noEmit"}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family javascript"));
    assert!(rewritten.contains("--kind javascript_tsc"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_go_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-go-rewrite",
            "session_id":"session-pretool-go-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"go test ./..."}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family go"));
    assert!(rewritten.contains("--kind go_test"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_hook_pretool_rewrites_supported_infra_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let (status, stdout) = run_claude_hook(
        dir.path(),
        &json!({
            "hook_event_name":"PreToolUse",
            "task_id":"task-pretool-infra-rewrite",
            "session_id":"session-pretool-infra-rewrite",
            "cwd":dir.path().to_str().unwrap(),
            "tool_name":"Bash",
            "tool_input":{"command":"kubectl get pods"}
        }),
    );
    assert_eq!(status, 0);
    let rendered: Value = serde_json::from_str(stdout.trim()).unwrap();
    let rewritten = rendered["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains("--family infra"));
    assert!(rewritten.contains("--kind kubectl_get"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_accepts_newline_json_stdio() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());

    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir.path());

    write_mcp_message_newline(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"protocolVersion":"2025-11-25","capabilities":{"roots":{}},"clientInfo":{"name":"claude-code","version":"2.1.72"}}
        }),
    );
    let initialize = read_mcp_message_newline(&mut stdout);
    assert_eq!(initialize["result"]["serverInfo"]["name"], "Packet28");
    assert_eq!(
        initialize["result"]["serverInfo"]["version"],
        workspace_packet28_version()
    );
    assert_eq!(initialize["result"]["protocolVersion"], "2024-11-05");
    assert!(initialize["result"]["capabilities"]["experimental"].is_null());

    write_mcp_message_newline(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/list"
        }),
    );
    let tools = read_mcp_message_newline(&mut stdout);
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28.write_intention"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28.search"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28.read_regions"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28.glob"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28.fetch_tool_result"));
    assert!(!tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "packet28.sync"));

    let _ = child.kill();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_proxy_namespaces_colliding_tools() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_alpha = dir.path().join("alpha_mcp.py");
    fs::write(
        &script_alpha,
        r#"import json, sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    params = message.get("params", {})
    if method == "initialize":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}, "resources": {}}, "serverInfo": {"name": "alpha", "version": "1"}}})
    elif method == "tools/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": [{"name": "shared.read", "description": "alpha shared tool", "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "resources/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resources": []}})
    elif method == "resources/templates/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resourceTemplates": []}})
    elif method == "tools/call":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"content": [{"type": "text", "text": "alpha ok"}], "structuredContent": {"owner": "alpha"}}})
    else:
        write_message({"jsonrpc": "2.0", "id": msg_id, "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .unwrap();

    let script_beta = dir.path().join("beta_mcp.py");
    fs::write(
        &script_beta,
        r#"import json, sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    params = message.get("params", {})
    if method == "initialize":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}, "resources": {}}, "serverInfo": {"name": "beta", "version": "1"}}})
    elif method == "tools/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": [{"name": "shared.read", "description": "beta shared tool", "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "resources/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resources": []}})
    elif method == "resources/templates/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resourceTemplates": []}})
    elif method == "tools/call":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"content": [{"type": "text", "text": "beta ok"}], "structuredContent": {"owner": "beta"}}})
    else:
        write_message({"jsonrpc": "2.0", "id": msg_id, "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .unwrap();

    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "alpha": {
                    "command": "python3",
                    "args": ["-u", script_alpha.to_str().unwrap()]
                },
                "beta": {
                    "command": "python3",
                    "args": ["-u", script_beta.to_str().unwrap()]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut child, mut stdin, mut stdout) =
        start_mcp_proxy_server(dir.path(), &config_path, "task-proxy-collision");

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}
        }),
    );
    let _ = read_mcp_message_for_id(&mut stdout, 1);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/list"
        }),
    );
    let tools = read_mcp_message_for_id(&mut stdout, 2);
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "alpha.shared.read"));
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "beta.shared.read"));

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"beta.shared.read",
                "arguments":{}
            }
        }),
    );
    let response = read_mcp_message_for_id(&mut stdout, 3);
    assert_eq!(
        response["result"]["structuredContent"]["owner"]
            .as_str()
            .unwrap(),
        "beta"
    );

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_proxy_caches_tool_catalog_and_respects_timeout_ms() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let counter_path = dir.path().join("tools-list-count.txt");
    let script_path = dir.path().join("slow_mcp.py");
    fs::write(
        &script_path,
        format!(
            r#"import json, pathlib, sys, time

COUNTER = pathlib.Path({counter:?})

def read_message():
    headers = {{}}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {{len(body)}}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    if method == "initialize":
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"protocolVersion": "2024-11-05", "capabilities": {{"tools": {{}}, "resources": {{}}}}, "serverInfo": {{"name": "slow", "version": "1"}}}}}})
    elif method == "tools/list":
        count = 0
        if COUNTER.exists():
            count = int(COUNTER.read_text() or "0")
        COUNTER.write_text(str(count + 1))
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"tools": [{{"name": "slow.read", "description": "slow tool", "inputSchema": {{"type": "object", "properties": {{}}}}}}]}}}})
    elif method == "resources/list":
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"resources": []}}}})
    elif method == "resources/templates/list":
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"resourceTemplates": []}}}})
    elif method == "tools/call":
        time.sleep(0.2)
        write_message({{"jsonrpc": "2.0", "id": msg_id, "result": {{"content": [{{"type": "text", "text": "slow ok"}}]}}}})
    else:
        write_message({{"jsonrpc": "2.0", "id": msg_id, "error": {{"code": -32601, "message": "unknown method"}}}})
"#,
            counter = counter_path,
        ),
    )
    .unwrap();

    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "slow": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "timeout_ms": 50
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut child, mut stdin, mut stdout, tools) = start_mcp_proxy_server_with_tool(
        dir.path(),
        &config_path,
        "task-proxy-timeout",
        "slow.read",
    );
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "slow.read"));
    let catalog_refresh_count = fs::read_to_string(&counter_path)
        .unwrap()
        .trim()
        .parse::<u64>()
        .unwrap();
    assert!(catalog_refresh_count >= 1);

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":10,
            "method":"tools/call",
            "params":{
                "name":"slow.read",
                "arguments":{}
            }
        }),
    );
    let timeout = read_mcp_message_for_id(&mut stdout, 10);
    assert!(timeout["error"]["message"]
        .as_str()
        .unwrap()
        .contains("50ms"));
    assert!(timeout["error"]["message"]
        .as_str()
        .unwrap()
        .contains("python3 -u"));
    assert_eq!(
        fs::read_to_string(&counter_path)
            .unwrap()
            .trim()
            .parse::<u64>()
            .unwrap(),
        catalog_refresh_count
    );

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_packet28_mcp_proxy_compacts_allowlisted_read_tool_results() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write_repo_fixture(dir.path());

    let script_path = dir.path().join("compact_mcp.py");
    fs::write(
        &script_path,
        r#"import json, sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.lower().strip()] = value.strip()
    length = int(headers.get("content-length", "0"))
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def write_message(value):
    body = json.dumps(value).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    msg_id = message.get("id")
    if msg_id is None:
        continue
    method = message.get("method")
    if method == "initialize":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}, "resources": {}}, "serverInfo": {"name": "compact", "version": "1"}}})
    elif method == "tools/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": [{"name": "compact.read", "description": "compact test tool", "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "resources/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resources": []}})
    elif method == "resources/templates/list":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"resourceTemplates": []}})
    elif method == "tools/call":
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": {"content": [{"type": "text", "text": "Alpha content line 1\nAlpha content line 2"}], "structuredContent": {"path": "src/alpha.rs", "lines": ["pub struct Alpha;", "impl Alpha {}"], "notes": "verbose upstream payload"}}})
    else:
        write_message({"jsonrpc": "2.0", "id": msg_id, "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .unwrap();

    let config_path = dir.path().join(".mcp.proxy.json");
    fs::write(
        &config_path,
        json!({
            "mcpServers": {
                "compact": {
                    "command": "python3",
                    "args": ["-u", script_path.to_str().unwrap()],
                    "compact_tools": ["compact.read"]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let (mut child, mut stdin, mut stdout, tools) = start_mcp_proxy_server_with_tool(
        dir.path(),
        &config_path,
        "task-proxy-compact",
        "compact.read",
    );
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "compact.read"));

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"compact.read",
                "arguments":{}
            }
        }),
    );
    let compact = read_mcp_message_for_id(&mut stdout, 2);
    let compact_payload = &compact["result"]["structuredContent"];
    assert_eq!(compact_payload["response_mode"], "slim");
    assert_eq!(compact_payload["original_tool"], "compact.read");
    assert!(compact_payload["artifact_id"].as_str().is_some());
    let artifact_id = compact_payload["artifact_id"].as_str().unwrap().to_string();

    write_mcp_message(
        &mut stdin,
        &json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"packet28.fetch_tool_result",
                "arguments":{
                    "task_id":"task-proxy-compact",
                    "artifact_id": artifact_id
                }
            }
        }),
    );
    let fetched = read_mcp_message_for_id(&mut stdout, 3);
    let fetched_payload = &fetched["result"]["structuredContent"];
    assert_eq!(fetched_payload["structuredContent"]["path"], "src/alpha.rs");
    assert!(fetched_payload["structuredContent"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line == "pub struct Alpha;"));

    child.kill().unwrap();
    child.wait().unwrap();

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_agent_prompt_outputs_all_supported_fragments() {
    for format in ["claude", "agents", "cursor"] {
        let output = suite_cmd()
            .args(["agent-prompt", "--format", format])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let rendered = String::from_utf8(output).unwrap();
        assert!(!rendered.trim().is_empty());
        assert!(rendered.contains("Packet28 mcp serve"));
        assert!(rendered.contains("packet28.write_intention"));
        assert!(rendered.contains("p28` instant grep"));
        assert!(rendered.contains("packet28.read_regions"));
        assert!(!rendered.contains("packet28.search"));
        assert!(rendered.to_ascii_lowercase().contains("handoff"));
        assert!(rendered
            .to_ascii_lowercase()
            .contains("fall back to direct file reads"));
    }
}

#[test]
fn test_suite_agent_prompt_root_is_reflected_in_command_example() {
    suite_cmd()
        .args(["agent-prompt", "--format", "claude", "--root", "repo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--root \"repo\""));
}

#[test]
#[cfg(unix)]
fn test_packet28_agent_persists_bootstrap_and_exports_env() {
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let task_text = "trace Alpha";
    let env_dump = dir.path().join("env.txt");

    agent_cmd()
        .current_dir(dir.path())
        .args([
            "--task",
            task_text,
            "--",
            "sh",
            "-c",
            "printf '%s\\n%s\\n' \"$PACKET28_ROOT\" \"$PACKET28_BOOTSTRAP_PATH\" > \"$1\"",
            "sh",
            env_dump.to_str().unwrap(),
        ])
        .assert()
        .success();

    let persisted_path = dir
        .path()
        .join(".packet28")
        .join("agent")
        .join("latest-bootstrap.json");
    assert!(persisted_path.exists());

    let env_lines = fs::read_to_string(&env_dump)
        .unwrap()
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        PathBuf::from(&env_lines[0]).canonicalize().unwrap(),
        dir.path().canonicalize().unwrap()
    );
    assert_eq!(
        PathBuf::from(&env_lines[1]).canonicalize().unwrap(),
        persisted_path.canonicalize().unwrap()
    );

    let value = parse_broker_response(&fs::read(&persisted_path).unwrap());
    assert!(value["brief"]
        .as_str()
        .unwrap()
        .contains("fresh session bootstrap"));
    assert_eq!(value["response_mode"], "full");
}

#[test]
#[cfg(unix)]
fn test_packet28_agent_returns_child_exit_code() {
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let task_text = "trace Alpha";

    agent_cmd()
        .current_dir(dir.path())
        .args(["--task", task_text, "--", "sh", "-c", "exit 7"])
        .assert()
        .code(7);
}

#[test]
fn test_packet28_agent_requires_child_command() {
    agent_cmd()
        .args(["--task", "review alpha.rs change"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("Usage:"));
}

#[test]
#[cfg(unix)]
fn test_suite_daemon_suppresses_disconnect_log_noise() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());
    init_repo(dir.path());

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let status_output = suite_cmd()
        .args([
            "daemon",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: Value = serde_json::from_slice(&status_output).unwrap();
    let socket = PathBuf::from(status.get("socket_path").and_then(Value::as_str).unwrap());
    let start = std::time::Instant::now();
    let mut stream = loop {
        match std::os::unix::net::UnixStream::connect(&socket) {
            Ok(stream) => break stream,
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound
                    && start.elapsed() < std::time::Duration::from_secs(15) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(err) => panic!(
                "failed to connect to daemon socket {}: {err}",
                socket.display()
            ),
        }
    };
    packet28_daemon_core::write_socket_message(
        &mut stream,
        &packet28_daemon_core::DaemonRequest::Status,
    )
    .unwrap();
    drop(stream);

    std::thread::sleep(std::time::Duration::from_millis(300));

    let log_path = dir.path().join(".packet28/daemon/packet28d.log");
    let start = std::time::Instant::now();
    while !log_path.exists() && start.elapsed() < std::time::Duration::from_secs(2) {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let log = fs::read_to_string(&log_path).unwrap();
    assert!(!log.contains("request handling failed: Broken pipe"));
    assert!(!log.contains("request handling failed: Connection reset"));
    assert!(!log.contains("request handling failed: unexpected end of file"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_diff_analyze_via_daemon_matches_packet_shape() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let local_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let via_daemon_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let local = parse_packet_wrapper(&local_output, "suite.diff.analyze.v1");
    let remote = parse_packet_wrapper(&via_daemon_output, "suite.diff.analyze.v1");
    assert_eq!(
        packet_payload(&local)
            .get("gate_result")
            .and_then(|value| value.get("passed"))
            .and_then(Value::as_bool),
        packet_payload(&remote)
            .get("gate_result")
            .and_then(|value| value.get("passed"))
            .and_then(Value::as_bool)
    );
    assert_eq!(
        packet_payload(&local).get("diffs"),
        packet_payload(&remote).get("diffs")
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_daemon_task_submit_returns_watch_id_and_watch_list() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let spec_path = dir.path().join("task-spec.json");
    fs::write(
        &spec_path,
        serde_json::to_string_pretty(&json!({
            "task_id": "task-watch",
            "sequence": {
                "steps": [
                    {
                        "id": "map",
                        "target": "mapy.repo",
                        "depends_on": [],
                        "input_packets": [],
                        "policy_context": {
                            "task_id": "task-watch"
                        },
                        "reducer_input": {
                            "repo_root": dir.path(),
                            "focus_paths": [],
                            "focus_symbols": [],
                            "max_files": 10,
                            "max_symbols": 20,
                            "include_tests": false
                        },
                        "budget": {}
                    }
                ],
                "budget": {},
                "reactive": {
                    "enabled": true,
                    "task_id": "task-watch",
                    "append_focused_map": true
                }
            },
            "watches": [
                {
                    "kind": "file",
                    "task_id": "task-watch",
                    "root": dir.path(),
                    "paths": ["src"],
                    "include_globs": ["src/**"],
                    "exclude_globs": []
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let submit_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "submit",
            "--root",
            dir.path().to_str().unwrap(),
            "--spec",
            spec_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let submit: Value = serde_json::from_slice(&submit_output).unwrap();
    let watch_id = submit
        .get("watches")
        .and_then(Value::as_array)
        .and_then(|watches| watches.first())
        .and_then(|watch| watch.get("watch_id"))
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    let watches_output = suite_cmd()
        .args([
            "daemon",
            "watch",
            "list",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-watch",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let watches: Value = serde_json::from_slice(&watches_output).unwrap();
    assert_eq!(
        watches
            .as_array()
            .and_then(|watches| watches.first())
            .and_then(|watch| watch.get("watch_id"))
            .and_then(Value::as_str),
        Some(watch_id.as_str())
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_daemon_task_submit_autofills_blank_and_missing_step_ids() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let spec_path = dir.path().join("task-spec.json");
    fs::write(
        &spec_path,
        serde_json::to_string_pretty(&json!({
            "task_id": "task-autofill",
            "sequence": {
                "steps": [
                    {
                        "id": "",
                        "target": "mapy.repo",
                        "depends_on": [],
                        "input_packets": [],
                        "policy_context": {
                            "task_id": "task-autofill"
                        },
                        "reducer_input": {
                            "repo_root": dir.path(),
                            "focus_paths": [],
                            "focus_symbols": [],
                            "max_files": 10,
                            "max_symbols": 20,
                            "include_tests": false
                        },
                        "budget": {}
                    },
                    {
                        "target": "mapy.repo",
                        "depends_on": ["mapy-repo-0"],
                        "input_packets": [],
                        "policy_context": {
                            "task_id": "task-autofill"
                        },
                        "reducer_input": {
                            "repo_root": dir.path(),
                            "focus_paths": [],
                            "focus_symbols": [],
                            "max_files": 10,
                            "max_symbols": 20,
                            "include_tests": false
                        },
                        "budget": {}
                    }
                ],
                "budget": {},
                "reactive": {
                    "enabled": true,
                    "task_id": "task-autofill",
                    "append_focused_map": true
                }
            },
            "watches": [
                {
                    "kind": "File",
                    "task_id": "task-autofill",
                    "root": dir.path(),
                    "paths": ["src"],
                    "include_globs": ["src/**"],
                    "exclude_globs": []
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    suite_cmd()
        .args([
            "daemon",
            "task",
            "submit",
            "--root",
            dir.path().to_str().unwrap(),
            "--spec",
            spec_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    let status_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-autofill",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: Value = serde_json::from_slice(&status_output).unwrap();
    let step_ids = status
        .get("sequence")
        .and_then(|sequence| sequence.get("steps"))
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|step| step.get("id").and_then(Value::as_str).unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        step_ids,
        vec!["mapy-repo-0".to_string(), "mapy-repo-1".to_string()]
    );
    assert_eq!(
        status
            .get("sequence")
            .and_then(|sequence| sequence.get("steps"))
            .and_then(Value::as_array)
            .and_then(|steps| steps.get(1))
            .and_then(|step| step.get("depends_on"))
            .and_then(Value::as_array)
            .and_then(|depends_on| depends_on.first())
            .and_then(Value::as_str),
        Some("mapy-repo-0")
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_daemon_task_submit_accepts_pascal_case_watch_kind() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let spec_path = dir.path().join("task-spec-watch.json");
    fs::write(
        &spec_path,
        serde_json::to_string_pretty(&json!({
            "task_id": "task-watch-kind",
            "sequence": {
                "steps": [
                    {
                        "target": "mapy.repo",
                        "depends_on": [],
                        "input_packets": [],
                        "policy_context": {
                            "task_id": "task-watch-kind"
                        },
                        "reducer_input": {
                            "repo_root": dir.path(),
                            "focus_paths": [],
                            "focus_symbols": [],
                            "max_files": 10,
                            "max_symbols": 20,
                            "include_tests": false
                        },
                        "budget": {}
                    }
                ],
                "budget": {},
                "reactive": {
                    "enabled": true,
                    "task_id": "task-watch-kind",
                    "append_focused_map": true
                }
            },
            "watches": [
                {
                    "kind": "File",
                    "task_id": "task-watch-kind",
                    "root": dir.path(),
                    "paths": ["src"],
                    "include_globs": ["src/**"],
                    "exclude_globs": []
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    suite_cmd()
        .args([
            "daemon",
            "task",
            "submit",
            "--root",
            dir.path().to_str().unwrap(),
            "--spec",
            spec_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    let watches_output = suite_cmd()
        .args([
            "daemon",
            "watch",
            "list",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-watch-kind",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let watches: Value = serde_json::from_slice(&watches_output).unwrap();
    assert_eq!(
        watches
            .as_array()
            .and_then(|watches| watches.first())
            .and_then(|watch| watch.get("spec"))
            .and_then(|spec| spec.get("kind"))
            .and_then(Value::as_str),
        Some("file")
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_daemon_failed_submit_cleans_up_task_and_watches() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());
    let spec_path = dir.path().join("bad-task-spec.json");
    fs::write(
        &spec_path,
        serde_json::to_string_pretty(&json!({
            "task_id": "task-invalid",
            "sequence": {
                "steps": [
                    {
                        "id": "",
                        "target": "nope.reducer",
                        "depends_on": [],
                        "input_packets": [],
                        "policy_context": {},
                        "reducer_input": {},
                        "budget": {}
                    }
                ],
                "budget": {},
                "reactive": {
                    "enabled": true,
                    "task_id": "task-invalid",
                    "append_focused_map": true
                }
            },
            "watches": [
                {
                    "kind": "file",
                    "task_id": "task-invalid",
                    "root": dir.path(),
                    "paths": ["src"],
                    "include_globs": ["src/**"],
                    "exclude_globs": []
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    suite_cmd()
        .args(["daemon", "start", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();

    suite_cmd()
        .args([
            "daemon",
            "task",
            "submit",
            "--root",
            dir.path().to_str().unwrap(),
            "--spec",
            spec_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .code(2);

    let task_output = suite_cmd()
        .args([
            "daemon",
            "task",
            "status",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-invalid",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let task_status: Value = serde_json::from_slice(&task_output).unwrap();
    assert!(task_status.is_null());

    let watches_output = suite_cmd()
        .args([
            "daemon",
            "watch",
            "list",
            "--root",
            dir.path().to_str().unwrap(),
            "--task-id",
            "task-invalid",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let watches: Value = serde_json::from_slice(&watches_output).unwrap();
    assert!(watches.as_array().unwrap().is_empty());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_via_daemon_uses_explicit_daemon_root_for_map_repo() {
    ensure_packet28d_built();
    let daemon_root = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    write_repo_fixture(daemon_root.path());
    init_repo(daemon_root.path());
    write_repo_fixture(repo_root.path());

    let output = suite_cmd()
        .current_dir(repo_root.path())
        .args([
            "--via-daemon",
            "--daemon-root",
            daemon_root.path().to_str().unwrap(),
            "map",
            "repo",
            "--repo-root",
            repo_root.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.map.repo.v1");
    assert!(packet_payload(&value).get("files_ranked").is_some());
    assert!(daemon_root
        .path()
        .join(".packet28/daemon/runtime.json")
        .exists());
    assert!(!repo_root
        .path()
        .join(".packet28/daemon/runtime.json")
        .exists());

    suite_cmd()
        .args([
            "daemon",
            "stop",
            "--root",
            daemon_root.path().to_str().unwrap(),
        ])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_via_daemon_honors_daemon_root_env() {
    ensure_packet28d_built();
    let daemon_root = TempDir::new().unwrap();
    let work_root = TempDir::new().unwrap();
    init_repo(daemon_root.path());
    init_repo(work_root.path());
    let manifest = work_root.path().join("manifest.jsonl");
    let testmap = work_root.path().join("testmap.bin");
    let timings = work_root.path().join("testtimings.bin");
    write_manifest(&manifest);

    suite_cmd()
        .current_dir(work_root.path())
        .env("PACKET28_DAEMON_ROOT", daemon_root.path().to_str().unwrap())
        .args([
            "--via-daemon",
            "test",
            "map",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
            "--timings-output",
            timings.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    assert!(daemon_root
        .path()
        .join(".packet28/daemon/runtime.json")
        .exists());
    assert!(!work_root
        .path()
        .join(".packet28/daemon/runtime.json")
        .exists());

    suite_cmd()
        .args([
            "daemon",
            "stop",
            "--root",
            daemon_root.path().to_str().unwrap(),
        ])
        .assert()
        .success();
}

#[test]
fn test_suite_context_assemble_machine_failure_emits_suite_error_v1() {
    let dir = TempDir::new().unwrap();
    let context = dir.path().join("context.yaml");
    let packet_a = dir.path().join("a.json");
    let packet_b = dir.path().join("b.json");
    write_governed_context(&context);
    write_context_packet(
        &packet_a,
        "diffy",
        "Diff gate",
        "critical regression in coverage",
        "src/lib.rs",
    );
    write_context_packet(
        &packet_b,
        "testy",
        "Impact plan",
        "selected tests for src/lib.rs",
        "src/lib.rs",
    );

    let output = suite_cmd()
        .args([
            "context",
            "assemble",
            "--packet",
            packet_a.to_str().unwrap(),
            "--packet",
            packet_b.to_str().unwrap(),
            "--context-config",
            context.to_str().unwrap(),
            "--budget-tokens",
            "1",
            "--budget-bytes",
            "1",
            "--json",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        value.get("schema_version").and_then(Value::as_str),
        Some("suite.error.v1")
    );
}

#[test]
#[cfg(unix)]
fn test_suite_via_daemon_diff_wrapper_surfaces_cache_hit() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    setup_changed_repo(dir.path());

    let first = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "--daemon-root",
            dir.path().to_str().unwrap(),
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "--daemon-root",
            dir.path().to_str().unwrap(),
            "diff",
            "analyze",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let first_value: Value = serde_json::from_slice(&first).unwrap();
    let second_value: Value = serde_json::from_slice(&second).unwrap();
    assert_eq!(
        first_value.get("cache_hit").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        second_value.get("cache_hit").and_then(Value::as_bool),
        Some(true)
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn test_suite_recall_prefers_summary_snippet_over_target_name() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let packet_a = dir.path().join("a.json");
    let packet_b = dir.path().join("b.json");
    write_context_packet(
        &packet_a,
        "diffy",
        "Diff gate",
        "critical regression in coverage",
        "src/lib.rs",
    );
    write_context_packet(
        &packet_b,
        "testy",
        "Impact plan",
        "selected tests for src/lib.rs",
        "src/lib.rs",
    );

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "assemble",
            "--packet",
            packet_a.to_str().unwrap(),
            "--packet",
            packet_b.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "recall",
            "--root",
            ".",
            "--query",
            "critical regression",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    let snippet = value
        .get("hits")
        .and_then(Value::as_array)
        .and_then(|hits| hits.first())
        .and_then(|hit| hit.get("snippet"))
        .and_then(Value::as_str)
        .unwrap();
    assert!(snippet.contains("critical regression"));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn test_suite_recall_json_surfaces_summary_field() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let packet = dir.path().join("stack.json");

    write_packet_value(
        &packet,
        &json!({
            "version": "1",
            "tool": "stacky",
            "kind": "stack_slice",
            "hash": "stack-hash",
            "summary": "stack failures total=2 unique=2",
            "files": [{"path": "src/main.rs", "relevance": 1.0}],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["stack.log"], "generated_at_unix": 1},
            "payload": {
                "total_failures": 2,
                "unique_failures": 2,
                "duplicates_removed": 0
            }
        }),
    );

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "assemble",
            "--packet",
            packet.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "recall",
            "--root",
            ".",
            "--query",
            "stack failures src/main.rs",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    let first_hit = value
        .get("hits")
        .and_then(Value::as_array)
        .and_then(|hits| hits.first())
        .unwrap();
    assert_eq!(
        first_hit.get("summary").and_then(Value::as_str),
        Some("stack failures total=2 unique=2")
    );
    assert_eq!(
        first_hit.get("snippet").and_then(Value::as_str),
        Some("stack failures total=2 unique=2")
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_test_map_and_shard_via_daemon_auto_start() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    let timings = dir.path().join("testtimings.bin");
    let tasks = dir.path().join("tasks.json");
    write_manifest(&manifest);
    fs::write(
        &tasks,
        serde_json::to_string_pretty(&json!({
            "schema_version": 1,
            "tasks": [
                {"id": "com.foo.BarTest", "selector": "com.foo.BarTest", "est_ms": 1200, "tags": ["unit"]},
                {"id": "com.foo.BazTest", "selector": "com.foo.BazTest", "est_ms": 900, "tags": ["unit"]}
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let map_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "test",
            "map",
            "--manifest",
            manifest.to_str().unwrap(),
            "--output",
            testmap.to_str().unwrap(),
            "--timings-output",
            timings.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let map_value: Value = serde_json::from_slice(&map_output).unwrap();
    assert_eq!(map_value.get("records").and_then(Value::as_u64), Some(1));
    assert!(dir.path().join(".packet28/daemon/runtime.json").exists());
    assert!(testmap.exists());
    assert!(timings.exists());

    let shard_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "test",
            "shard",
            "--shards",
            "2",
            "--tasks-json",
            tasks.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shard_value: Value = serde_json::from_slice(&shard_output).unwrap();
    assert_eq!(
        shard_value
            .get("shards")
            .and_then(Value::as_array)
            .map(|value| value.len()),
        Some(2)
    );

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_stack_and_build_via_daemon_emit_packet_wrappers() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let stack_input = dir.path().join("stack.log");
    let build_input = dir.path().join("build.log");
    write_stack_log(&stack_input);
    write_build_log(&build_input);

    let stack_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "stack",
            "slice",
            "--input",
            stack_input.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stack_value = parse_packet_wrapper(&stack_output, "suite.stack.slice.v1");
    assert!(packet_payload(&stack_value)
        .get("failures")
        .and_then(Value::as_array)
        .is_some());

    let build_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "build",
            "reduce",
            "--input",
            build_input.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let build_value = parse_packet_wrapper(&build_output, "suite.build.reduce.v1");
    assert!(packet_payload(&build_value)
        .get("groups")
        .and_then(Value::as_array)
        .is_some());

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_suite_context_non_assemble_via_daemon_smoke() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let diff = dir.path().join("diff.json");
    let impact = dir.path().join("impact.json");
    let event = dir.path().join("event.json");
    let packet_a = dir.path().join("a.json");
    let packet_b = dir.path().join("b.json");

    write_packet_value(
        &diff,
        &json!({
            "version": "1",
            "tool": "diffy",
            "kind": "diff_analyze",
            "hash": "diff-hash",
            "summary": "changed StopWatch",
            "files": [{"path": "src/StopWatch.java", "relevance": 1.0}],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["diff"], "generated_at_unix": 1},
            "payload": {
                "gate_result": {"passed": true, "violations": []},
                "diffs": [{"path": "src/StopWatch.java", "old_path": null, "status": "Modified", "changed_lines": [10, 11]}]
            }
        }),
    );
    write_packet_value(
        &impact,
        &json!({
            "version": "1",
            "tool": "testy",
            "kind": "test_impact",
            "hash": "impact-hash",
            "summary": "impact",
            "files": [],
            "symbols": [{"name": "StopWatchTest#testSplit", "kind": "test_id", "relevance": 1.0}],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["testmap.bin"], "generated_at_unix": 1},
            "payload": {
                "result": {
                    "selected_tests": ["StopWatchTest#testSplit"],
                    "smoke_tests": [],
                    "missing_mappings": [],
                    "confidence": 0.9,
                    "stale": false,
                    "escalate_full_suite": false
                },
                "known_tests": 1,
                "print_command": null
            }
        }),
    );
    write_state_event(
        &event,
        r#"{
  "event_id": "evt-1",
  "occurred_at_unix": 1,
  "actor": "tester",
  "kind": "focus_set",
  "paths": ["src/lib.rs"],
  "symbols": [],
  "data": {"type": "focus_set"}
}"#,
    );
    write_context_packet(
        &packet_a,
        "diffy",
        "Diff gate",
        "critical regression in coverage",
        "src/lib.rs",
    );
    write_context_packet(
        &packet_b,
        "testy",
        "Impact plan",
        "selected tests for src/lib.rs",
        "src/lib.rs",
    );

    let correlate_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "correlate",
            "--packet",
            diff.to_str().unwrap(),
            "--packet",
            impact.to_str().unwrap(),
            "--task-id",
            "task-correlation",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let correlate_value = parse_packet_wrapper(&correlate_output, "suite.context.correlate.v1");
    assert!(packet_payload(&correlate_value)
        .get("findings")
        .and_then(Value::as_array)
        .is_some());

    let state_append_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "state",
            "append",
            "--task-id",
            "task-state",
            "--input",
            event.to_str().unwrap(),
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let state_append_value = parse_packet_wrapper(&state_append_output, "suite.agent.state.v1");
    assert_eq!(
        packet_payload(&state_append_value)
            .get("task_id")
            .and_then(Value::as_str),
        Some("task-state")
    );

    let state_snapshot_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "state",
            "snapshot",
            "--task-id",
            "task-state",
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let state_snapshot_value =
        parse_packet_wrapper(&state_snapshot_output, "suite.agent.snapshot.v1");
    assert_eq!(
        packet_payload(&state_snapshot_value)
            .get("task_id")
            .and_then(Value::as_str),
        Some("task-state")
    );

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "assemble",
            "--packet",
            packet_a.to_str().unwrap(),
            "--packet",
            packet_b.to_str().unwrap(),
        ])
        .assert()
        .success();

    let store_list_output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "list",
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let store_list_value: Value = serde_json::from_slice(&store_list_output).unwrap();
    let entries = store_list_value
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap();
    assert!(!entries.is_empty());

    let key = entries[0]
        .get("cache_key")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "get",
            "--root",
            ".",
            "--key",
            &key,
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&key));

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "stats",
            "--root",
            ".",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"stats\""));

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "recall",
            "--root",
            ".",
            "--query",
            "critical regression",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"query\":\"critical regression\"",
        ));

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "--via-daemon",
            "context",
            "store",
            "prune",
            "--root",
            ".",
            "--all",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"report\""));

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
