#[expect(
    dead_code,
    reason = "this lifecycle fixture exercises a focused subset of the shared harness"
)]
#[path = "process_harness.rs"]
mod process_harness;

use assert_cmd::Command;
use std::fs;
use std::path::Path;

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn ensure_packet28d_built() {
    process_harness::ensure_packet28d_built();
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

fn git(root: &Path, args: &[&str]) {
    process_harness::run_git(root, args);
}

pub fn init_repo(root: &Path) {
    git(root, &["init"]);
}
