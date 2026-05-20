use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::Path;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[cfg(unix)]
fn write_executable_script(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
fn test_run_reducer_reduces_cargo_check() {
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

#[test]
#[cfg(unix)]
fn test_run_reducer_reduces_tree_command() {
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
fn test_run_reducer_reduces_npm_test_and_pytest() {
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
fn test_run_reducer_reduces_file_and_search_commands() {
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
