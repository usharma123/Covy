#![cfg(unix)]

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn write_executable_script(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
fn test_run_reducer_runtime_reduces_ruby_and_dotnet_commands() {
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
