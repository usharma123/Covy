use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_system_core_json_deps_and_env_commands() {
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
fn test_system_core_read_command_filters_and_numbers_files() {
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
fn test_system_core_summary_command_preserves_exit_and_summarizes_output() {
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
fn test_system_core_err_command_preserves_exit_and_summarizes_failure() {
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
