use assert_cmd::Command;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn run_setup(root: &Path, home: &Path, runtime: &str) {
    static SETUP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = SETUP_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("setup test lock should not be poisoned");

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

    suite_cmd()
        .current_dir(root)
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .args(["daemon", "stop", "--root", root.to_str().unwrap()])
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
