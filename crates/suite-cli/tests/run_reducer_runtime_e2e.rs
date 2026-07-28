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
fn test_run_reducer_runtime_reduces_infra_and_github_commands() {
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
