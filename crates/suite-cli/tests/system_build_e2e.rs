use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
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
#[cfg(unix)]
fn test_system_build_and_lint_commands_use_reducer_wrappers() {
    let root = TempDir::new().unwrap();
    let bin_dir = root.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable_script(
        &bin_dir.join("cargo"),
        "#!/bin/sh\nprintf 'running 2 tests\\n\\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("go"),
        "#!/bin/sh\nprintf 'ok\\tgithub.com/acme/core\\t0.113s\\nFAIL\\tgithub.com/acme/api\\t0.222s\\n'; exit 1\n",
    );
    write_executable_script(
        &bin_dir.join("dotnet"),
        "#!/bin/sh\nprintf 'Passed!  - Failed: 0, Passed: 12, Skipped: 0, Total: 12, Duration: 1 s\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("golangci-lint"),
        "#!/bin/sh\nprintf 'pkg/service/service.go:17:2: undefined: missingHelper (typecheck)\\npkg/service/service.go:22:9: Error return value is not checked (errcheck)\\n' >&2; exit 1\n",
    );
    write_executable_script(
        &bin_dir.join("gradlew"),
        "#!/bin/sh\nprintf 'Starting a Gradle Daemon\\n> Task :compileKotlin\\nw: warning detail\\nFAILURE: Build failed with an exception.\\n* What went wrong:\\nExecution failed for task compileKotlin.\\nBUILD FAILED in 2s\\n'; exit 1\n",
    );
    write_executable_script(
        &bin_dir.join("rake"),
        "#!/bin/sh\nprintf 'Run options: --seed 1\\n\\n# Running:\\n\\n..F....\\n\\n7 runs, 7 assertions, 1 failures, 0 errors, 0 skips\\n'; exit 1\n",
    );
    write_executable_script(
        &bin_dir.join("rspec"),
        "#!/bin/sh\nprintf 'Failures:\\n  1) User validates email\\n     spec/models/user_spec.rb:12\\n\\n3 examples, 1 failure\\n'; exit 1\n",
    );
    write_executable_script(
        &bin_dir.join("rubocop"),
        "#!/bin/sh\nprintf 'Inspecting 1 file\\nC\\n\\n1 file inspected, 1 offense detected\\n'; exit 1\n",
    );
    write_executable_script(
        &bin_dir.join("mypy"),
        "#!/bin/sh\nprintf 'src/auth.py:12: error: Incompatible return value type  [return-value]\\nsrc/auth.py:18: error: Name x is not defined  [name-defined]\\n'; exit 1\n",
    );
    write_executable_script(
        &bin_dir.join("pip"),
        "#!/bin/sh\nprintf '[{\"name\":\"pytest\",\"version\":\"8.1.0\",\"latest_version\":\"8.2.0\"}]\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("pnpm"),
        "#!/bin/sh\nprintf 'Tests 1 failed | 7 passed\\nFAIL src/app.test.ts > app\\n'; exit 1\n",
    );
    write_executable_script(
        &bin_dir.join("bundle"),
        "#!/bin/sh\nif [ \"$1\" = install ]; then printf 'Using rake 13.2.1\\nInstalling rails 7.1.0\\nBundle complete!\\n'; exit 0; fi\nexit 2\n",
    );
    let path_env = std::env::join_paths(std::iter::once(bin_dir.as_path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["cargo", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cargo test passed (2 tests)"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["go", "test", "./..."])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("go test: 1 pkgs passed, 1 failed"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["dotnet", "test", "Packet28.Tests.csproj"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dotnet test: Passed!"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["golangci-lint", "run"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "golangci-lint: 2 diagnostics in pkg/service/service.go",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["gradlew", "build"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "FAILURE: Build failed with an exception.",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["rake", "test"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "rake test: 7 runs, 7 assertions, 1 failures, 0 errors, 0 skips",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["rspec", "spec/models/user_spec.rb"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("rspec: 3 examples, 1 failure"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["rubocop", "app"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "rubocop: 1 file inspected, 1 offense detected",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["mypy", "src"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "mypy: 2 error(s) in 1 file(s); first src/auth.py (return-value)",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["pip", "list", "--outdated"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "pip outdated: 1 package(s); first pytest (8.1.0)",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["pnpm", "test"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("vitest: 1 failed, 7 passed"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["bundle", "install"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Bundle complete!"));
}
