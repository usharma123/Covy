use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{self, Value};
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_run_filter_applies_project_toml_filter_to_fallback_command() {
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
fn test_run_filter_skips_untrusted_project_toml_filter() {
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
