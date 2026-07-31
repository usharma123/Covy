use std::fs;
use std::path::Path;

use assert_cmd::Command;

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn ensure_packet28d_built() {
    crate::process_harness::ensure_packet28d_built();
}

fn git(root: &Path, args: &[&str]) {
    crate::process_harness::run_git(root, args);
}

pub fn init_repo(root: &Path) {
    git(root, &["init"]);
}

pub fn write_repo_fixture(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("auth.rs"),
        r#"
struct AuthCache;

fn invalidate_auth_cache() {}
"#,
    )
    .unwrap();
}
