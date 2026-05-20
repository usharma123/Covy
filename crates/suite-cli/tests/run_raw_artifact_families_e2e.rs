#[path = "support/run_raw_artifact.rs"]
mod run_raw_artifact;

use predicates::prelude::*;
use run_raw_artifact::{init_repo, suite_cmd};
use serde_json::Value;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::TempDir;

#[cfg(unix)]
fn write_executable_script(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
#[cfg(unix)]
fn test_run_raw_artifact_families_available_across_reducer_families() {
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
