#[path = "support/verify.rs"]
mod verify;

use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;
use verify::suite_cmd;

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
