use assert_cmd::Command;
use packet28_daemon_core::{
    read_socket_message, ready_path, socket_path, write_socket_message, DaemonRequest,
    DaemonResponse,
};
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

pub fn daemon_bin() -> PathBuf {
    static DAEMON_BIN: OnceLock<PathBuf> = OnceLock::new();
    DAEMON_BIN
        .get_or_init(|| {
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let workspace = manifest_dir
                .parent()
                .and_then(|path| path.parent())
                .expect("workspace root");
            let status = ProcessCommand::new("cargo")
                .args(["build", "-p", "packet28d"])
                .current_dir(workspace)
                .status()
                .expect("build packet28d");
            assert!(status.success(), "packet28d build failed");
            workspace.join("target/debug/packet28d")
        })
        .clone()
}

pub fn cli_with_daemon_env() -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("p28"));
    command.env("CARGO_BIN_EXE_packet28d", daemon_bin());
    command
}

pub struct DaemonHandle {
    child: Child,
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[allow(clippy::zombie_processes)]
pub fn start_daemon(root: &Path) -> DaemonHandle {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut child = ProcessCommand::new(daemon_bin())
        .args(["serve", "--root", canonical_root.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        if ready_path(&canonical_root).exists() && socket_path(&canonical_root).exists() {
            return DaemonHandle { child };
        }
        if let Some(status) = child.try_wait().unwrap() {
            let (stdout, stderr) = child_output(&mut child);
            panic!(
                "packet28d exited early for {} with status {status}; stdout={stdout:?} stderr={stderr:?}",
                canonical_root.display()
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let (stdout, stderr) = child_output(&mut child);
    panic!(
        "packet28d did not become ready for {}; stdout={stdout:?} stderr={stderr:?}",
        canonical_root.display()
    );
}

pub fn stop_daemon(root: &Path) {
    let socket = socket_path(root);
    if !socket.exists() {
        return;
    }
    if let Ok(stream) = UnixStream::connect(&socket) {
        let reader_stream = stream.try_clone().unwrap();
        let mut writer = std::io::BufWriter::new(stream);
        let mut reader = std::io::BufReader::new(reader_stream);
        let _ = write_socket_message(&mut writer, &DaemonRequest::Stop);
        let _ = read_socket_message::<_, DaemonResponse>(&mut reader);
    }
}

fn child_output(child: &mut Child) -> (String, String) {
    let mut stderr = String::new();
    if let Some(mut stream) = child.stderr.take() {
        let _ = stream.read_to_string(&mut stderr);
    }
    let mut stdout = String::new();
    if let Some(mut stream) = child.stdout.take() {
        let _ = stream.read_to_string(&mut stdout);
    }
    (stdout, stderr)
}
