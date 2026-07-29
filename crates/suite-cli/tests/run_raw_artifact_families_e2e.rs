#[expect(
    dead_code,
    reason = "shared integration harness APIs are exercised by sibling test binaries"
)]
#[path = "support/process_harness.rs"]
mod process_harness;
#[path = "support/run_raw_artifact.rs"]
mod run_raw_artifact;
#[path = "support/run_raw_artifact_families.rs"]
mod run_raw_artifact_families;

use predicates::prelude::*;
use run_raw_artifact::{init_repo, suite_cmd};
use run_raw_artifact_families::install_fake_reducer_bins;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

#[test]
#[cfg(unix)]
fn test_run_raw_artifact_families_available_across_reducer_families() {
    let root = TempDir::new().unwrap();
    init_repo(root.path());
    fs::write(root.path().join("raw-visible.txt"), "fs raw marker\n").unwrap();
    fs::write(root.path().join("git-visible.txt"), "changed\n").unwrap();

    let fake_bins = install_fake_reducer_bins();

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
            .env("PATH", fake_bins.path_env())
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
