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
fn test_run_reducer_raw_artifact_available_across_reducer_families() {
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

#[test]
#[cfg(unix)]
fn test_run_reducer_reduces_infra_and_github_commands() {
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
fn test_run_reducer_reduces_ruby_and_dotnet_commands() {
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
