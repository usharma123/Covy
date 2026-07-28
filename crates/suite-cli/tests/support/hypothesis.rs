use assert_cmd::Command;
use std::fs;
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::sync::OnceLock;

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn ensure_packet28d_built() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let status = ProcessCommand::new("cargo")
            .args(["build", "-p", "packet28d"])
            .status()
            .unwrap();
        assert!(status.success(), "failed to build packet28d");
    });
}

fn git(root: &Path, args: &[&str]) {
    let status = ProcessCommand::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed with {status}", args);
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
