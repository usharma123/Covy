use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{self, Value};
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
fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed with {status}", args);
}

#[cfg(unix)]
fn init_repo(root: &Path) {
    git(root, &["init"]);
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
fn test_run_raw_artifact_available_across_reducer_families() {
    let root = TempDir::new().unwrap();
    init_repo(root.path());
    fs::write(root.path().join("raw-visible.txt"), "fs raw marker\n").unwrap();
    fs::write(root.path().join("git-visible.txt"), "changed\n").unwrap();

    let bin_dir = TempDir::new().unwrap();
    write_executable_script(
        &bin_dir.path().join("cargo"),
        "#!/bin/sh\nprintf '    Checking packet28_fixture v0.1.0\\n    Finished dev [unoptimized + debuginfo] target(s) in 0.01s\\nrust raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("npx"),
        "#!/bin/sh\nprintf 'javascript raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("python3"),
        "#!/bin/sh\nprintf 'tests/test_demo.py .\\n1 passed in 0.01s\\npython raw marker\\n'\n",
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

    let cases: Vec<(&str, Vec<&str>, &str, &str)> = vec![
        (
            "git",
            vec!["git", "status", "--short"],
            "git-visible.txt",
            "git status",
        ),
        (
            "git",
            vec!["gt", "submit"],
            "Created pull request",
            "created PR #42",
        ),
        ("fs", vec!["cat", "raw-visible.txt"], "fs raw marker", "cat"),
        (
            "rust",
            vec!["cargo", "check"],
            "rust raw marker",
            "Checking packet28_fixture",
        ),
        (
            "javascript",
            vec!["npx", "tsc", "--noEmit"],
            "javascript raw marker",
            "tsc passed",
        ),
        (
            "python",
            vec!["python3", "-m", "pytest", "tests"],
            "python raw marker",
            "pytest passed",
        ),
        (
            "go",
            vec!["go", "test", "./..."],
            "go raw marker",
            "go test passed",
        ),
        (
            "infra",
            vec!["docker", "logs", "demo"],
            "infra raw marker",
            "docker logs returned",
        ),
        (
            "github",
            vec!["gh", "pr", "checks", "1"],
            "github raw marker",
            "gh pr checks",
        ),
        (
            "ruby",
            vec!["ruby", "sample_test.rb"],
            "ruby raw marker",
            "1 runs",
        ),
        (
            "dotnet",
            vec!["dotnet", "test", "Packet28.Tests.csproj"],
            "dotnet raw marker",
            "dotnet test",
        ),
        (
            "jvm",
            vec!["gradle", "test"],
            "gradle raw marker",
            "2 tests completed",
        ),
    ];

    for (family, argv, raw_marker, compact_marker) in cases {
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
        let summary = value["reduction"]["summary"].as_str().unwrap_or_default();
        let preview = value["reduction"]["compact_preview"]
            .as_str()
            .unwrap_or_default();
        assert!(
            summary.contains(compact_marker) || preview.contains(compact_marker),
            "{family} compact output missing marker {compact_marker:?}: summary={summary:?} preview={preview:?}"
        );
        assert!(
            !summary.is_empty(),
            "{family} reducer returned an empty compact summary"
        );
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
fn test_run_raw_artifact_reduced_command_is_fetchable() {
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
fn test_run_raw_artifact_fallback_command_is_fetchable() {
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
fn test_run_raw_artifact_failing_reduced_command_preserves_exit_and_stderr() {
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
        .stdout(predicate::str::contains("unclosed delimiter"))
        .stdout(predicate::str::contains("cargo check"));
}
