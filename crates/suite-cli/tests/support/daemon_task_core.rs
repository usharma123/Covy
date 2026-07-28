use crate::process_harness::{HarnessLimits, ProcessHarness};
use assert_cmd::Command;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

const BUILD_TIMEOUT: Duration = Duration::from_secs(180);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn ensure_packet28d_built() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut command = std::process::Command::new("cargo");
        command.args(["build", "-p", "packet28d", "--locked"]);
        let output =
            ProcessHarness::run(&mut command, &[], BUILD_TIMEOUT, HarnessLimits::default())
                .unwrap_or_else(|error| panic!("failed to run packet28d build: {error}"));
        assert!(
            output.status.success(),
            "failed to build packet28d\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    });
}

pub fn write_repo_fixture(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("alpha.rs"),
        r#"
use crate::beta::Beta;

fn alpha() {}
struct Alpha;
"#,
    )
    .unwrap();
    fs::write(
        src.join("beta.rs"),
        r#"
fn beta() {}
enum Beta {
  A,
}
"#,
    )
    .unwrap();
}

pub fn git(root: &Path, args: &[&str]) {
    let mut command = std::process::Command::new("git");
    command.current_dir(root).args(args);
    let output = ProcessHarness::run(&mut command, &[], COMMAND_TIMEOUT, HarnessLimits::default())
        .unwrap_or_else(|error| panic!("git {args:?} failed to run: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
