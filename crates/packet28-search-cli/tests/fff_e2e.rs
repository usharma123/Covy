use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use predicates::prelude::*;

fn cli() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("p28"))
}

fn write_fixture(root: &Path) {
    fs::create_dir_all(root.join("src/nested")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub struct Alpha;\npub fn alpha_service() {}\nconst ALPHA: &str = \"Alpha\";\n",
    )
    .unwrap();
    fs::write(
        root.join("src/nested/mod.rs"),
        "pub enum Beta { AlphaVariant }\nfn handle_value() { println!(\"beta\"); }\n",
    )
    .unwrap();
    for idx in 0..10 {
        fs::write(
            root.join("src").join(format!("filler_{idx}.rs")),
            format!("pub fn filler_{idx}() {{ println!(\"beta_{idx}\"); }}\n"),
        )
        .unwrap();
    }
}

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
