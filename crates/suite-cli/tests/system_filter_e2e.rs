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
fn test_system_filter_backed_rtk_wrappers_use_builtin_filters() {
    let root = TempDir::new().unwrap();
    let bin_dir = root.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable_script(
        &bin_dir.join("brew"),
        "#!/bin/sh\nprintf 'Warning: rtk 0.27.1 is already installed and up-to-date.\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("composer"),
        "#!/bin/sh\nprintf 'Loading composer repositories with package information\\nNothing to install, update or remove\\nGenerating autoload files\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("df"),
        "#!/bin/sh\nprintf 'Filesystem     1K-blocks   Used Available Use%% Mounted on\\n/dev/sda1        4096000 123456   3972544   4%% /\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("du"),
        "#!/bin/sh\nprintf '4.0K\\t./src\\n\\n8.0K\\t./tests\\n16K\\t.\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("make"),
        "#!/bin/sh\nprintf \"make[1]: Entering directory '/tmp'\\ngcc -O2 foo.c\\nmake[1]: Leaving directory '/tmp'\\n\"; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("mvn"),
        "#!/bin/sh\nprintf '[INFO] ---\\n[INFO] BUILD SUCCESS\\n[INFO] Total time: 4.123 s\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("poetry"),
        "#!/bin/sh\nprintf 'Installing dependencies from lock file\\n\\nNo dependencies to install or update\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("uv"),
        "#!/bin/sh\nprintf 'Resolved 42 packages in 123ms\\nAudited 42 packages in 0.05ms\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("terraform"),
        "#!/bin/sh\nprintf 'Refreshing state... [id=vpc-abc]\\nNo changes. Your infrastructure matches the configuration.\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("tofu"),
        "#!/bin/sh\nprintf 'Refreshing state... [id=vpc-abc]\\nNo changes. Your infrastructure matches the configuration.\\n'; exit 0\n",
    );
    let path_env = std::env::join_paths(std::iter::once(bin_dir.as_path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();

    for (args, expected) in [
        (vec!["brew", "install", "rtk"], "ok (already installed)"),
        (vec!["composer", "install"], "ok (up to date)"),
        (vec!["df"], "/dev/sda1"),
        (vec!["du", "-sh", "."], "8.0K\t./tests"),
        (vec!["make"], "gcc -O2 foo.c"),
        (vec!["mvn", "package"], "[INFO] BUILD SUCCESS"),
        (vec!["poetry", "install"], "ok (up to date)"),
        (vec!["uv", "sync"], "ok (up to date)"),
        (
            vec!["terraform", "plan"],
            "No changes. Your infrastructure matches the configuration.",
        ),
        (
            vec!["tofu", "plan"],
            "No changes. Your infrastructure matches the configuration.",
        ),
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
