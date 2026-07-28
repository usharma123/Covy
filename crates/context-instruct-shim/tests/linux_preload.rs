#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "context-instruct-shim-linux-{}-{nonce}-{}",
            std::process::id(),
            TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create Linux preload test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn preload_bridge_preserves_open_arities_modes_and_errno() {
    let fixture_root = TestDir::new();
    let shim = build_shim(&fixture_root.path().join("cargo-target"));
    fs::create_dir(fixture_root.path().join("directory")).unwrap();
    fs::write(fixture_root.path().join("existing.txt"), "open-existing").unwrap();
    fs::write(
        fixture_root.path().join("existing64.txt"),
        "open64-existing",
    )
    .unwrap();
    fs::write(
        fixture_root.path().join("existing_at.txt"),
        "openat-existing",
    )
    .unwrap();
    fs::write(
        fixture_root.path().join("existing_at64.txt"),
        "openat64-existing",
    )
    .unwrap();
    fs::write(fixture_root.path().join("AGENTS.md"), "agents-existing").unwrap();

    let fixture_binary = fixture_root.path().join("linux-open-abi");
    let fixture_source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linux_open_abi.c");
    let compile = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
        .arg(&fixture_source)
        .arg("-o")
        .arg(&fixture_binary)
        .output()
        .expect("invoke cc for Linux open ABI fixture");
    assert!(
        compile.status.success(),
        "failed to compile Linux open ABI fixture\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let output = Command::new(&fixture_binary)
        .arg(fixture_root.path())
        .current_dir(fixture_root.path())
        .env("LD_PRELOAD", &shim)
        .env("P28_DEBUG", "1")
        .env_remove("PACKET28_DAEMON_ROOT")
        .env_remove("PACKET28_AGENT_FAMILY")
        .output()
        .expect("run Linux open ABI fixture with LD_PRELOAD");
    assert!(
        output.status.success(),
        "Linux open ABI fixture failed with shim '{}'\nstdout:\n{}\nstderr:\n{}",
        shim.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "linux-open-abi-ok"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("reason=daemon_socket_missing").count(),
        4,
        "all four interposed open variants must reach the fixed Rust callbacks\n{stderr}"
    );

    assert_mode(&fixture_root.path().join("created.txt"), 0o601);
    assert_mode(&fixture_root.path().join("created64.txt"), 0o640);
    assert_mode(&fixture_root.path().join("created_at.txt"), 0o624);
    assert_mode(&fixture_root.path().join("created_at64.txt"), 0o660);
}

fn build_shim(target_dir: &Path) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("shim crate belongs to a workspace");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(workspace_root)
        .env("CARGO_TARGET_DIR", target_dir)
        .args([
            "build",
            "--package",
            "context-instruct-shim",
            "--locked",
            "--offline",
        ])
        .output()
        .expect("build Linux preload shim");
    assert!(
        output.status.success(),
        "failed to build Linux preload shim\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let shim = target_dir.join("debug/libcontext_instruct_shim.so");
    assert!(
        shim.is_file(),
        "context-instruct-shim cdylib not found at '{}'",
        shim.display()
    );
    shim
}

fn assert_mode(path: &Path, expected: u32) {
    let actual = fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(actual, expected, "unexpected mode for '{}'", path.display());
}
