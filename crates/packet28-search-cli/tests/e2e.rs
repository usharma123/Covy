mod support;

use std::fs;
use std::path::{Path, PathBuf};

use predicates::prelude::*;
use std::os::unix::fs::PermissionsExt;
use support::{cli, output, stderr_text, stdout_text, write_fixture};

fn write_fake_fff_mcp(root: &Path) -> PathBuf {
    let fake_fff = root.join("fake-fff-mcp.sh");
    fs::write(
        &fake_fff,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake-fff","version":"0"}}}'
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"→ Read src/lib.rs (best match)\nsrc/lib.rs\n 1: pub struct Alpha;\nsrc/nested/mod.rs\n 1: pub enum Beta { AlphaVariant }"}]}}'
      ;;
  esac
done
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&fake_fff).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_fff, perms).unwrap();
    fake_fff
}

#[test]
fn debug_build_prints_generation_and_file_count() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("build_ms="))
        .stdout(predicate::str::contains("generation="))
        .stdout(predicate::str::contains("files="));
}

#[test]
fn p28_searches_from_repo_root_with_rg_style_output() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .current_dir(dir.path())
        .args(["Alpha", "--fixed-strings"])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"));
}

#[test]
fn p28_filters_paths_from_current_directory() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .current_dir(dir.path())
        .args(["handle_value", "src/nested/mod.rs", "--fixed-strings"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "src/nested/mod.rs:2:fn handle_value() { println!(\"beta\"); }",
        ))
        .stdout(predicate::str::contains("src/lib.rs").not());
}

#[test]
fn p28_stats_go_to_stderr_while_hits_stay_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let output = output({
        let mut command = cli();
        command
            .current_dir(dir.path())
            .args(["Alpha", "--fixed-strings", "--stats"]);
        command
    });

    assert!(output.status.success());
    let stdout = stdout_text(&output);
    let stderr = stderr_text(&output);
    assert!(stdout.contains("src/lib.rs:1:pub struct Alpha;"));
    assert!(!stdout.contains("backend="));
    assert!(stderr.contains("p28_ms="));
    assert!(stderr.contains("transport="));
    assert!(stderr.contains("backend="));
}

#[test]
fn p28_fff_engine_adapts_mcp_grep_results() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let fake_fff = write_fake_fff_mcp(dir.path());

    cli()
        .current_dir(dir.path())
        .env("P28_FFF_MCP_BIN", &fake_fff)
        .args(["Alpha", "--engine", "fff", "--fixed-strings", "--stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"))
        .stdout(predicate::str::contains(
            "src/nested/mod.rs:1:pub enum Beta { AlphaVariant }",
        ))
        .stderr(predicate::str::contains("backend=fff_mcp"));
}

#[test]
fn p28_fff_engine_respects_requested_paths() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let fake_fff = write_fake_fff_mcp(dir.path());

    cli()
        .current_dir(dir.path())
        .env("P28_FFF_MCP_BIN", &fake_fff)
        .args([
            "Alpha",
            "src/lib.rs",
            "--engine",
            "fff",
            "--fixed-strings",
            "--stats",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"))
        .stdout(predicate::str::contains("src/nested/mod.rs").not())
        .stderr(predicate::str::contains("backend=fff_mcp"));
}

#[test]
fn p28_auto_uses_fff_for_broad_index_fallback_when_available() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let fake_fff = write_fake_fff_mcp(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cli()
        .current_dir(dir.path())
        .env("P28_FFF_MCP_BIN", &fake_fff)
        .args(["fn", "--transport", "inproc", "--stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"))
        .stderr(predicate::str::contains("backend=fff_mcp"));
}

#[test]
fn p28_auto_can_prefer_fff_when_configured() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());
    let fake_fff = write_fake_fff_mcp(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cli()
        .current_dir(dir.path())
        .env("P28_FFF_MCP_BIN", &fake_fff)
        .env("P28_FFF_AUTO", "prefer")
        .args(["Alpha", "--transport", "inproc", "--stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"))
        .stderr(predicate::str::contains("backend=fff_mcp"));
}

#[test]
fn p28_auto_prefer_records_fff_backend_failure_before_native_fallback() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cli()
        .current_dir(dir.path())
        .env("P28_FFF_MCP_BIN", dir.path().join("missing-fff-mcp"))
        .env("P28_FFF_AUTO", "prefer")
        .args([
            "Alpha",
            "--transport",
            "inproc",
            "--fixed-strings",
            "--stats",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"))
        .stderr(predicate::str::contains("backend=indexed_regex"))
        .stderr(predicate::str::contains(
            "fallback_reason=fff auto preferred backend failed",
        ));
}

#[test]
fn p28_handles_anchored_line_start_regexes() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn build() {\n    SearchRequest {\n        query: pattern,\n    };\n}\n",
    )
    .unwrap();

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cli()
        .current_dir(dir.path())
        .args([r"^\s*SearchRequest\s*\{", "--stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "src/main.rs:2:    SearchRequest {",
        ))
        .stderr(predicate::str::contains("backend="));
}

#[test]
fn debug_bench_prints_packet28_and_legacy_timings() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cli()
        .args([
            "debug",
            "bench",
            dir.path().to_str().unwrap(),
            "Alpha",
            "--fixed-strings",
            "--transport",
            "inproc",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("guard=index"))
        .stdout(predicate::str::contains("parity=exact"))
        .stdout(predicate::str::contains("p28_ms="))
        .stdout(predicate::str::contains("legacy_rg_ms="));
}
