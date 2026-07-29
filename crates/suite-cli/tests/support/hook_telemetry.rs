use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use assert_cmd::Command;

use crate::process_harness::{HarnessLimits, ProcessHarness};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn run_hook_raw_with_env(
    runtime: &str,
    root: &Path,
    stdin_payload: &str,
    envs: &[(&str, &OsStr)],
) -> (i32, String, String) {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    command
        .current_dir(root)
        .args(["hook", runtime, "--root", root.to_str().unwrap()]);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = ProcessHarness::run(
        &mut command,
        stdin_payload.as_bytes(),
        COMMAND_TIMEOUT,
        HarnessLimits::default(),
    )
    .unwrap_or_else(|error| panic!("{runtime} hook process failed: {error}"));
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}
