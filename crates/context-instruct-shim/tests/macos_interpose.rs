#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use packet28_daemon_protocol::frame::{read_frame, write_frame};
use packet28_daemon_protocol::message::{
    ContextBackendKind, ContextResolveOutcome, ContextResolveResponse, ContextSourceKind,
    DaemonRequest, DaemonResponse, InstructionRenderMode,
};
use packet28_daemon_protocol::paths::socket_path;

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "context-instruct-shim-macos-{}-{nonce}-{}",
            std::process::id(),
            TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create macOS interpose test directory");
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
fn interpose_virtualizes_only_readonly_instruction_opens() {
    let fixture_root = TestDir::new();
    let canonical_root = fs::canonicalize(fixture_root.path()).expect("canonicalize fixture root");
    let shim = build_shim(&fixture_root.path().join("cargo-target"));
    let instruction_path = fixture_root.path().join("AGENTS.md");
    fs::write(&instruction_path, "agents-original").expect("write instruction fixture");
    let fixture_binary = fixture_root.path().join("macos-open-semantics");
    let fixture_source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/macos_open_semantics.c");
    let compile = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
        .arg(&fixture_source)
        .arg("-o")
        .arg(&fixture_binary)
        .output()
        .expect("invoke cc for macOS open semantics fixture");
    assert!(
        compile.status.success(),
        "failed to compile macOS open semantics fixture\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let (done, socket, server) = spawn_rewrite_server(&canonical_root);
    let output = Command::new(&fixture_binary)
        .arg(&canonical_root)
        .current_dir(&canonical_root)
        .env("DYLD_INSERT_LIBRARIES", &shim)
        .env("P28_DEBUG", "1")
        .env_remove("PACKET28_DAEMON_ROOT")
        .env_remove("PACKET28_AGENT_FAMILY")
        .output()
        .expect("run macOS open semantics fixture with interpose shim");
    done.store(true, Ordering::Release);
    let requests = server
        .join()
        .expect("rewrite server thread must not panic")
        .expect("rewrite server must complete");
    let _ = fs::remove_file(&socket);

    assert_eq!(
        requests, 4,
        "only two pure-read opens for each interposed symbol may reach the daemon"
    );
    assert!(
        output.status.success(),
        "macOS open semantics fixture failed with shim '{}'\nstdout:\n{}\nstderr:\n{}",
        shim.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "macos-open-semantics-ok"
    );
    assert_eq!(
        fs::read_to_string(instruction_path).expect("read real instruction"),
        "agents-original"
    );
}

fn spawn_rewrite_server(
    root: &Path,
) -> (
    Arc<AtomicBool>,
    PathBuf,
    thread::JoinHandle<Result<usize, String>>,
) {
    let socket = socket_path(root);
    fs::create_dir_all(socket.parent().expect("socket has parent"))
        .expect("create daemon socket parent");
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).expect("bind rewrite server");
    listener
        .set_nonblocking(true)
        .expect("make rewrite server nonblocking");
    let done = Arc::new(AtomicBool::new(false));
    let thread_done = Arc::clone(&done);
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut requests = 0;
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .map_err(|error| error.to_string())?;
                    let request: DaemonRequest =
                        read_frame(&mut stream).map_err(|error| error.to_string())?;
                    let DaemonRequest::ContextResolve { request } = request else {
                        return Err("unexpected daemon request variant".to_string());
                    };
                    if request.source_content != "agents-original"
                        || request.source_path.as_deref() != Some("AGENTS.md")
                        || request.backend_kind != ContextBackendKind::Unknown
                    {
                        return Err(format!("unexpected context request: {request:?}"));
                    }
                    let response = DaemonResponse::ContextResolve {
                        response: ContextResolveResponse {
                            source_kind: ContextSourceKind::InstructionFile,
                            source_path: Some("AGENTS.md".to_string()),
                            outcome: ContextResolveOutcome::Rewrite {
                                content: "agents-rewritten".to_string(),
                                content_sha256: "test-content".to_string(),
                                render_mode: InstructionRenderMode::Stable,
                                stable_config_sha256: String::new(),
                                snapshot_sha256: None,
                                rendered_sha256: "test-render".to_string(),
                                task_label: "macos-open-test".to_string(),
                                original_bytes: "agents-original".len(),
                                rewritten_bytes: "agents-rewritten".len(),
                                cache_hit: false,
                                matched_terms: Vec::new(),
                                section_titles: Vec::new(),
                                schema_version: 1,
                            },
                        },
                    };
                    write_frame(&mut stream, &response).map_err(|error| error.to_string())?;
                    requests += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if thread_done.load(Ordering::Acquire) {
                        return Ok(requests);
                    }
                    if Instant::now() >= deadline {
                        return Err("rewrite server exceeded 20-second deadline".to_string());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    });
    (done, socket, handle)
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
        .expect("build macOS interpose shim");
    assert!(
        output.status.success(),
        "failed to build macOS interpose shim\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let shim = target_dir.join("debug/libcontext_instruct_shim.dylib");
    assert!(
        shim.is_file(),
        "context-instruct-shim cdylib not found at '{}'",
        shim.display()
    );
    shim
}
