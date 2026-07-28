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
fn test_system_remaining_filter_backed_wrappers_use_builtin_filters() {
    let root = TempDir::new().unwrap();
    let bin_dir = root.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable_script(
        &bin_dir.join("basedpyright"),
        "#!/bin/sh\nprintf 'basedpyright 1.22.0\\nSearching for source files\\nFound 42 source files\\n\\n/home/user/app/main.py\\n  /home/user/app/main.py:10:5 - error: \"foo\" is not defined (reportUndefinedVariable)\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("biome"),
        "#!/bin/sh\nprintf 'Checked 42 files in 0.5s\\n\\nsrc/app.tsx:5:3 lint/suspicious/noExplicitAny\\n  x Unexpected any. Specify a different type.\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("gcc"),
        "#!/bin/sh\nprintf 'In file included from /usr/include/stdio.h:42:\\nmain.c:10:5: error: use of undeclared identifier '\\''foo'\\''\\n    foo();\\n    ^\\n1 error generated.\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("g++"),
        "#!/bin/sh\nprintf '/usr/bin/ld: /tmp/main.o: undefined reference to '\\''missing_func'\\''\\ncollect2: error: ld returned 1 exit status\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("gradle"),
        "#!/bin/sh\nprintf '> Task :compileJava\\nBUILD SUCCESSFUL in 2s\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("jira"),
        "#!/bin/sh\nprintf 'TYPE\\tKEY\\tSUMMARY\\tSTATUS\\n\\nStory\\tPROJ-123\\tAdd login feature\\tIn Progress\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("jj"),
        "#!/bin/sh\nprintf '@  qpvuntsm user@example.com 2026-03-10 12:00 abc123\\n│  feat: add new feature\\n\\nWorking copy now at: qpvuntsm abc123 feat: add new feature\\nHint: use `jj log` to see the full history\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("jq"),
        "#!/bin/sh\nprintf '{\\n  \"name\": \"test\",\\n  \"version\": \"1.0\"\\n}\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("just"),
        "#!/bin/sh\nprintf 'cargo test\\n\\ntest result: ok. 42 passed; 0 failed\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("mise"),
        "#!/bin/sh\nprintf 'mise Installing node@20.0.0\\nmise Downloading node@20.0.0\\n\\nlint check passed\\n2 warnings found\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("nx"),
        "#!/bin/sh\nprintf '   > NX   Running target build for project myapp\\n\\nCompiled successfully.\\nOutput: dist/apps/myapp\\n\\n   Nx (powered by computation caching)\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("ollama"),
        "#!/bin/sh\nprintf '⠋ \\n⠙ \\nHello! How can I help you today?\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("oxlint"),
        "#!/bin/sh\nprintf '  x eslint(no-console): Unexpected console statement.\\n   ╭─[src/app.ts:5:3]\\nFinished in 12ms on 100 files.\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("skopeo"),
        "#!/bin/sh\nprintf 'Getting image source signatures\\nCopying blob sha256:abc123 done\\nWriting manifest to image destination\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("ssh"),
        "#!/bin/sh\nprintf 'Warning: Permanently added '\\''192.168.1.10'\\'' (ED25519) to the list of known hosts.\\n\\ntotal 32\\ndrwxr-xr-x 4 user user 4096 Mar 10 12:00 app\\nConnection to 192.168.1.10 closed.\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("stat"),
        "#!/bin/sh\nprintf '  File: main.rs\\n  Size: 12345           Blocks: 24         IO Block: 4096   regular file\\nDevice: 801h/2049d      Inode: 1234567     Links: 1\\nAccess: 2026-03-10 12:00:00.000000000 +0100\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("swift"),
        "#!/bin/sh\ncase \"$1\" in build) printf 'Compiling MyApp MyApp.swift\\nBuild complete!\\n' ;; test) printf \"CompileSwift normal x86_64 MyTests.swift\\nTest Suite 'All tests' passed at 2026-05-12 10:00:00.000.\\n\" ;; esac; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("task"),
        "#!/bin/sh\nprintf 'task: [test] go test ./...\\nok  myapp 0.5s\\ntask: Task \"lint\" is up to date\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("turbo"),
        "#!/bin/sh\nprintf ' cache hit, replaying logs abc123\\n\\n> myapp:build\\n\\nCompiled successfully.\\n\\nTasks:    2 successful, 2 total (1 cached)\\nDuration: 3.2s\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("ty"),
        "#!/bin/sh\nprintf 'ty 0.1.0\\nChecking 15 files\\n\\nerror[unresolved-reference]: Name `foo` used when not defined\\n  --> app/main.py:10:5\\nFound 1 error, 0 warnings\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("xcodebuild"),
        "#!/bin/sh\nprintf 'note: Using new build system\\nCompileSwift normal arm64 /Users/dev/App/ViewController.swift\\n/Users/dev/App/ViewController.swift:42:9: error: use of unresolved identifier '\\''foo'\\''\\n** BUILD FAILED **\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("yadm"),
        "#!/bin/sh\nprintf 'On branch main\\n\\n  (use \"yadm add\" to update what will be committed)\\n\\nChanges not staged for commit:\\n  modified:   .bashrc\\n'; exit 0\n",
    );
    let path_env = std::env::join_paths(std::iter::once(bin_dir.as_path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();

    for (args, expected) in [
        (vec!["basedpyright"], "reportUndefinedVariable"),
        (vec!["biome", "check", "."], "lint/suspicious/noExplicitAny"),
        (vec!["gcc", "main.c"], "use of undeclared identifier"),
        (vec!["g++", "main.cc"], "undefined reference"),
        (vec!["gradle", "build"], "Build Summary:"),
        (vec!["jira", "issue", "list"], "PROJ-123"),
        (vec!["jj", "log"], "feat: add new feature"),
        (vec!["jq", "."], "\"name\": \"test\""),
        (vec!["just", "test"], "test result: ok"),
        (vec!["mise", "run", "lint"], "lint check passed"),
        (vec!["nx", "build", "myapp"], "Compiled successfully."),
        (vec!["ollama", "run", "llama3"], "Hello!"),
        (vec!["oxlint"], "eslint(no-console)"),
        (vec!["skopeo", "copy"], "skopeo: ok"),
        (vec!["ssh", "host", "ls"], "total 32"),
        (vec!["stat", "main.rs"], "Size: 12345"),
        (vec!["swift", "build"], "ok (build complete)"),
        (vec!["swift", "test"], "ok (swift tests passed)"),
        (vec!["task", "test"], "ok  myapp 0.5s"),
        (vec!["turbo", "build"], "Compiled successfully."),
        (vec!["ty", "check"], "unresolved-reference"),
        (vec!["xcodebuild"], "** BUILD FAILED **"),
        (vec!["yadm", "status"], "modified:   .bashrc"),
    ] {
        suite_cmd()
            .current_dir(root.path())
            .env("PATH", &path_env)
            .args(args)
            .assert()
            .success()
            .stdout(predicate::str::contains(expected));
    }
}
