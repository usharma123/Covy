#[expect(
    dead_code,
    reason = "this integration binary exercises a focused subset of the shared harness"
)]
#[path = "support/process_harness.rs"]
mod process_harness;

use assert_cmd::Command;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

use process_harness::{HarnessLimits, ProcessHarness};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn ensure_packet28d_built() {
    process_harness::ensure_packet28d_built();
}

fn git(root: &Path, args: &[&str]) {
    process_harness::run_git(root, args);
}

fn init_repo(root: &Path) {
    git(root, &["init"]);
}

fn install_counting_cat(dir: &Path) -> (String, std::path::PathBuf) {
    let bin_dir = dir.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let counter_path = dir.join("cat-count.txt");
    fs::write(&counter_path, "0\n").unwrap();
    let script_path = bin_dir.join("cat");
    fs::write(
        &script_path,
        format!(
            "#!/bin/sh\ncount=$(/bin/cat \"{count}\" 2>/dev/null || echo 0)\ncount=$((count + 1))\nprintf '%s\\n' \"$count\" > \"{count}\"\nexec /bin/cat \"$@\"\n",
            count = counter_path.display()
        ),
    )
    .unwrap();
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();
    let path_env = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    (path_env, counter_path)
}

#[test]
#[cfg(unix)]
fn test_hook_runner_cli_reuses_cached_summary_without_rerunning_command() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("sample.txt"), "Alpha\nBeta\n").unwrap();

    let (path_env, counter_path) = install_counting_cat(dir.path());
    let spec = packet28_reducer_core::classify_command("cat sample.txt").unwrap();
    let mut first = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    first.current_dir(dir.path()).env("PATH", &path_env).args([
        "hook",
        "reducer-runner",
        "--root",
        dir.path().to_str().unwrap(),
        "--task-id",
        "task-runner-cache",
        "--family",
        &spec.family,
        "--kind",
        &spec.canonical_kind,
        "--fingerprint",
        &spec.cache_fingerprint,
        "--cwd",
        dir.path().to_str().unwrap(),
        "--",
        "cat",
        "sample.txt",
    ]);
    let first = ProcessHarness::run(&mut first, &[], COMMAND_TIMEOUT, HarnessLimits::default())
        .unwrap_or_else(|error| panic!("first reducer-runner invocation failed: {error}"));
    assert!(first.status.success());

    let mut second = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    second.current_dir(dir.path()).env("PATH", &path_env).args([
        "hook",
        "reducer-runner",
        "--root",
        dir.path().to_str().unwrap(),
        "--task-id",
        "task-runner-cache",
        "--family",
        &spec.family,
        "--kind",
        &spec.canonical_kind,
        "--fingerprint",
        &spec.cache_fingerprint,
        "--cwd",
        dir.path().to_str().unwrap(),
        "--",
        "cat",
        "sample.txt",
    ]);
    let second = ProcessHarness::run(&mut second, &[], COMMAND_TIMEOUT, HarnessLimits::default())
        .unwrap_or_else(|error| panic!("second reducer-runner invocation failed: {error}"));
    assert!(second.status.success());
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(fs::read_to_string(&counter_path).unwrap().trim(), "1");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
#[cfg(unix)]
fn test_hook_runner_cli_busts_cache_after_out_of_band_file_edit() {
    ensure_packet28d_built();
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("sample.txt"), "Alpha\nBeta\n").unwrap();

    let (path_env, counter_path) = install_counting_cat(dir.path());
    let spec = packet28_reducer_core::classify_command("cat sample.txt").unwrap();
    let runner_args = [
        "hook",
        "reducer-runner",
        "--root",
        dir.path().to_str().unwrap(),
        "--task-id",
        "task-runner-stale-cache",
        "--family",
        &spec.family,
        "--kind",
        &spec.canonical_kind,
        "--fingerprint",
        &spec.cache_fingerprint,
        "--cwd",
        dir.path().to_str().unwrap(),
        "--",
        "cat",
        "sample.txt",
    ];

    let mut first = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    first
        .current_dir(dir.path())
        .env("PATH", &path_env)
        .args(runner_args);
    let first = ProcessHarness::run(&mut first, &[], COMMAND_TIMEOUT, HarnessLimits::default())
        .unwrap_or_else(|error| panic!("first reducer-runner invocation failed: {error}"));
    assert!(first.status.success());

    fs::write(dir.path().join("sample.txt"), "Alpha\nBeta\nGamma\n").unwrap();

    let mut second = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"));
    second
        .current_dir(dir.path())
        .env("PATH", &path_env)
        .args(runner_args);
    let second = ProcessHarness::run(&mut second, &[], COMMAND_TIMEOUT, HarnessLimits::default())
        .unwrap_or_else(|error| panic!("second reducer-runner invocation failed: {error}"));
    assert!(second.status.success());
    assert_ne!(first.stdout, second.stdout);
    assert_eq!(fs::read_to_string(&counter_path).unwrap().trim(), "2");

    suite_cmd()
        .args(["daemon", "stop", "--root", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
