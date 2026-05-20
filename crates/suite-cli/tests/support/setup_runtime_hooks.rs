use assert_cmd::Command;
use std::path::Path;

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn run_setup(root: &Path, home: &Path, runtime: &str) {
    suite_cmd()
        .current_dir(root)
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .args([
            "setup",
            "--root",
            root.to_str().unwrap(),
            "--runtime",
            runtime,
            "--yes",
        ])
        .assert()
        .success();
}

pub fn doctor_command(root: &Path, home: &Path, agent: &str) -> Command {
    let mut command = suite_cmd();
    command
        .current_dir(root)
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .args(["doctor", "--root", root.to_str().unwrap(), "--agent", agent]);
    command
}
