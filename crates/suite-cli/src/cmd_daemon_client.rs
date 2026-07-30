use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use packet28_daemon_core::storage::read_runtime_info;
use packet28_daemon_protocol::{
    commands::{
        CoverCheckRequest, CoverCheckResponse, PacketFetchRequest, PacketFetchResponse,
        SequenceSubmitResponse, TaskSubmitSpec, TestMapRequest, TestMapResponse, TestShardRequest,
        TestShardResponse,
    },
    context_store::{
        ContextRecallRequest, ContextRecallResponse, ContextStoreGetRequest,
        ContextStoreGetResponse, ContextStoreListRequest, ContextStoreListResponse,
        ContextStorePruneDaemonRequest, ContextStorePruneResponse, ContextStoreStatsRequest,
        ContextStoreStatsResponse,
    },
    frame::{read_frame, write_frame},
    message::{
        ContextResolveRequest, ContextResolveResponse, DaemonRequest, DaemonResponse, DaemonStatus,
        DaemonTransportAuth,
    },
    paths::{
        log_path, ready_path, resolve_workspace_root, runtime_path, socket_path,
        workspace_socket_path,
    },
};

#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::io::{BufReader, BufWriter, Read, Write};
#[cfg(unix)]
use std::net::TcpStream;
#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
const DAEMON_SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(unix)]
#[derive(Clone)]
struct DaemonEndpoint {
    address: String,
    transport_auth: Option<DaemonTransportAuth>,
}

#[cfg(unix)]
pub struct PersistentDaemonClient {
    root: PathBuf,
    reader: BufReader<DaemonStream>,
    writer: BufWriter<DaemonStream>,
}

#[cfg(unix)]
pub(crate) enum DaemonStream {
    Unix(UnixStream),
    Tcp(TcpStream),
}

#[cfg(unix)]
impl DaemonStream {
    fn try_clone(&self) -> std::io::Result<Self> {
        match self {
            DaemonStream::Unix(stream) => stream.try_clone().map(DaemonStream::Unix),
            DaemonStream::Tcp(stream) => stream.try_clone().map(DaemonStream::Tcp),
        }
    }
}

#[cfg(unix)]
impl Read for DaemonStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            DaemonStream::Unix(stream) => stream.read(buf),
            DaemonStream::Tcp(stream) => stream.read(buf),
        }
    }
}

#[cfg(unix)]
impl Write for DaemonStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            DaemonStream::Unix(stream) => stream.write(buf),
            DaemonStream::Tcp(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            DaemonStream::Unix(stream) => stream.flush(),
            DaemonStream::Tcp(stream) => stream.flush(),
        }
    }
}

pub fn via_daemon_env_enabled() -> bool {
    crate::cmd_common::parse_daemon_env_flag(std::env::var("PACKET28_VIA_DAEMON").ok().as_deref())
}

pub fn daemon_root_env() -> Option<String> {
    std::env::var("PACKET28_DAEMON_ROOT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn daemon_workspace_root(explicit_root: Option<&str>) -> Result<PathBuf> {
    let start = if let Some(root) = explicit_root {
        PathBuf::from(root)
    } else if let Some(root) = daemon_root_env() {
        PathBuf::from(root)
    } else {
        std::env::current_dir().context("failed to resolve current directory")?
    };
    Ok(resolve_workspace_root(&start))
}

fn normalize_daemon_root(root: &Path) -> PathBuf {
    resolve_workspace_root(root)
}

#[cfg(not(unix))]
pub(crate) fn daemon_not_supported<T>() -> Result<T> {
    Err(anyhow!(
        "packet28 daemon commands are only supported on Unix targets"
    ))
}

pub fn execute_kernel_request(
    root: &Path,
    request: context_kernel_core::KernelRequest,
) -> Result<context_kernel_core::KernelResponse> {
    ensure_daemon(root)?;
    match send_request(root, &DaemonRequest::Execute { request })? {
        DaemonResponse::Execute { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

pub fn send_kernel_request(
    root: &Path,
    request: context_kernel_core::KernelRequest,
) -> Result<context_kernel_core::KernelResponse> {
    execute_kernel_request(root, request)
}

pub fn execute_sequence(root: &Path, spec: TaskSubmitSpec) -> Result<SequenceSubmitResponse> {
    ensure_daemon(root)?;
    match send_request(root, &DaemonRequest::ExecuteSequence { spec })? {
        DaemonResponse::ExecuteSequence {
            response,
            task,
            watches,
        } => Ok(SequenceSubmitResponse {
            task_id: task.task_id,
            watch_ids: watches.iter().map(|watch| watch.watch_id.clone()).collect(),
            response,
        }),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

pub fn execute_cover_check(root: &Path, request: CoverCheckRequest) -> Result<CoverCheckResponse> {
    ensure_daemon(root)?;
    match send_request(root, &DaemonRequest::CoverCheck { request })? {
        DaemonResponse::CoverCheck { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

pub fn execute_packet_fetch(
    root: &Path,
    request: PacketFetchRequest,
) -> Result<PacketFetchResponse> {
    ensure_daemon(root)?;
    match send_request(root, &DaemonRequest::PacketFetch { request })? {
        DaemonResponse::PacketFetch { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

pub fn send_cover_check(root: &Path, request: CoverCheckRequest) -> Result<CoverCheckResponse> {
    execute_cover_check(root, request)
}

pub fn send_packet_fetch(root: &Path, request: PacketFetchRequest) -> Result<PacketFetchResponse> {
    execute_packet_fetch(root, request)
}

pub fn execute_test_shard(root: &Path, request: TestShardRequest) -> Result<TestShardResponse> {
    ensure_daemon(root)?;
    match send_request(root, &DaemonRequest::TestShard { request })? {
        DaemonResponse::TestShard { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

pub fn execute_test_map(root: &Path, request: TestMapRequest) -> Result<TestMapResponse> {
    ensure_daemon(root)?;
    match send_request(root, &DaemonRequest::TestMap { request })? {
        DaemonResponse::TestMap { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

pub fn execute_context_store_list(
    root: &Path,
    request: ContextStoreListRequest,
) -> Result<ContextStoreListResponse> {
    ensure_daemon(root)?;
    match send_request(root, &DaemonRequest::ContextStoreList { request })? {
        DaemonResponse::ContextStoreList { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

pub fn execute_context_store_get(
    root: &Path,
    request: ContextStoreGetRequest,
) -> Result<ContextStoreGetResponse> {
    ensure_daemon(root)?;
    match send_request(root, &DaemonRequest::ContextStoreGet { request })? {
        DaemonResponse::ContextStoreGet { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

pub fn execute_context_store_prune(
    root: &Path,
    request: ContextStorePruneDaemonRequest,
) -> Result<ContextStorePruneResponse> {
    ensure_daemon(root)?;
    match send_request(root, &DaemonRequest::ContextStorePrune { request })? {
        DaemonResponse::ContextStorePrune { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

pub fn execute_context_store_stats(
    root: &Path,
    request: ContextStoreStatsRequest,
) -> Result<ContextStoreStatsResponse> {
    ensure_daemon(root)?;
    match send_request(root, &DaemonRequest::ContextStoreStats { request })? {
        DaemonResponse::ContextStoreStats { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

pub fn execute_context_recall(
    root: &Path,
    request: ContextRecallRequest,
) -> Result<ContextRecallResponse> {
    ensure_daemon(root)?;
    match send_request(root, &DaemonRequest::ContextRecall { request })? {
        DaemonResponse::ContextRecall { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

pub fn execute_context_resolve(
    root: &Path,
    request: ContextResolveRequest,
) -> Result<ContextResolveResponse> {
    match send_request(root, &DaemonRequest::ContextResolve { request })? {
        DaemonResponse::ContextResolve { response } => Ok(response),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

#[cfg(unix)]
pub fn send_request(root: &Path, request: &DaemonRequest) -> Result<DaemonResponse> {
    let root = normalize_daemon_root(root);
    ensure_daemon(&root)?;
    let response = send_request_existing_daemon(&root, request)?;
    if daemon_response_indicates_protocol_mismatch(&response) {
        restart_daemon(&root)?;
        return send_request_existing_daemon(&root, request);
    }
    Ok(response)
}

#[cfg(unix)]
pub(crate) fn subscribe_task(
    root: &Path,
    task_id: &str,
    replay_last: usize,
    after_seq: Option<u64>,
) -> Result<(DaemonStream, usize)> {
    let endpoint = daemon_endpoint(root)?;
    let stream = connect_daemon_endpoint(&endpoint)?;
    let mut writer = BufWriter::new(stream.try_clone()?);
    let mut reader = BufReader::new(stream.try_clone()?);
    write_frame(
        &mut writer,
        &DaemonRequest::TaskSubscribe {
            task_id: task_id.to_string(),
            replay_last,
            after_seq,
        },
    )?;
    match read_frame(&mut reader)? {
        DaemonResponse::TaskSubscribeAck { replayed, .. } => Ok((stream, replayed)),
        DaemonResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

#[cfg(not(unix))]
pub fn send_request(_root: &Path, _request: &DaemonRequest) -> Result<DaemonResponse> {
    daemon_not_supported()
}

#[cfg(unix)]
pub(crate) fn send_request_without_start(
    root: &Path,
    request: &DaemonRequest,
) -> Result<DaemonResponse> {
    let root = normalize_daemon_root(root);
    send_request_existing_daemon(&root, request)
}

#[cfg(not(unix))]
pub(crate) fn send_request_without_start(
    _root: &Path,
    _request: &DaemonRequest,
) -> Result<DaemonResponse> {
    daemon_not_supported()
}

#[cfg(unix)]
impl PersistentDaemonClient {
    pub fn connect(root: &Path) -> Result<Self> {
        let root = normalize_daemon_root(root);
        ensure_daemon(&root)?;
        let endpoint = daemon_endpoint(&root)?;
        let stream = connect_daemon_endpoint(&endpoint)?;
        let reader_stream = stream.try_clone()?;
        Ok(Self {
            root,
            reader: BufReader::new(reader_stream),
            writer: BufWriter::new(stream),
        })
    }

    pub fn send_request(&mut self, request: &DaemonRequest) -> Result<DaemonResponse> {
        write_frame(&mut self.writer, request)?;
        Ok(read_frame(&mut self.reader)?)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(unix)]
pub(crate) fn ensure_daemon(root: &Path) -> Result<()> {
    let root = normalize_daemon_root(root);
    if daemon_status_existing(&root).is_ok() {
        return Ok(());
    }
    let endpoint = daemon_endpoint(&root)?;
    if endpoint_may_have_stale_socket(&endpoint) && connect_daemon_endpoint(&endpoint).is_err() {
        cleanup_unreachable_runtime_files(&root)?;
    }
    start_daemon(&root)?;
    wait_for_daemon(&root, Duration::from_secs(10))
}

#[cfg(not(unix))]
pub(crate) fn ensure_daemon(_root: &Path) -> Result<()> {
    daemon_not_supported()
}

pub(crate) fn resolve_root_arg(root: &str) -> PathBuf {
    let cwd = PathBuf::from(root);
    resolve_workspace_root(&cwd)
}

#[cfg(unix)]
fn start_daemon(root: &Path) -> Result<()> {
    let binary = packet28d_binary()?;
    ensure_executable(&binary)?;
    let root_arg = root.to_string_lossy().to_string();
    let log_path = log_path(root);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create daemon log dir '{}'", parent.display()))?;
    }
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open daemon log '{}'", log_path.display()))?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open daemon log '{}'", log_path.display()))?;
    Command::new(binary)
        .arg("serve")
        .arg("--root")
        .arg(root_arg)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .context("failed to spawn packet28d")?;
    Ok(())
}

#[cfg(unix)]
fn wait_for_daemon(root: &Path, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if daemon_status_existing(root).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    if let Ok(runtime) = read_runtime_info(root) {
        return Err(anyhow!(
            "packet28d did not become ready; runtime file exists for pid {} at {} (log: {})",
            runtime.pid,
            runtime.socket_path,
            runtime.log_path
        ));
    }
    Err(anyhow!("packet28d did not become ready"))
}

#[cfg(unix)]
pub(crate) fn restart_daemon(root: &Path) -> Result<()> {
    let root = normalize_daemon_root(root);
    stop_daemon_if_running(&root)?;
    wait_for_daemon_shutdown(&root, Duration::from_secs(5))?;
    cleanup_unreachable_runtime_files(&root)?;
    start_daemon(&root)?;
    wait_for_daemon(&root, Duration::from_secs(10))
}

#[cfg(unix)]
fn daemon_response_indicates_protocol_mismatch(response: &DaemonResponse) -> bool {
    matches!(
        response,
        DaemonResponse::Error { message } if daemon_error_indicates_protocol_mismatch(message)
    )
}

#[cfg(unix)]
fn daemon_error_indicates_protocol_mismatch(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("unknown variant") && lower.contains("expected one of")
}

#[cfg(unix)]
fn send_request_existing_daemon(root: &Path, request: &DaemonRequest) -> Result<DaemonResponse> {
    let endpoint = daemon_endpoint(root)?;
    let stream = connect_daemon_endpoint(&endpoint)?;
    let reader_stream = stream.try_clone()?;
    let mut writer = BufWriter::new(stream);
    let mut reader = BufReader::new(reader_stream);
    write_frame(&mut writer, request)?;
    Ok(read_frame(&mut reader)?)
}

#[cfg(unix)]
fn daemon_status_existing(root: &Path) -> Result<DaemonStatus> {
    match send_request_existing_daemon(root, &DaemonRequest::Status) {
        Ok(DaemonResponse::Status { status }) => Ok(status),
        Ok(DaemonResponse::Error { message }) => Err(anyhow!(message)),
        Ok(other) => Err(anyhow!("unexpected daemon status response: {other:?}")),
        Err(err) => Err(err),
    }
}

#[cfg(unix)]
fn connect_daemon_socket(socket: &Path) -> Result<DaemonStream> {
    let stream = UnixStream::connect(socket)
        .with_context(|| format!("failed to connect to '{}'", socket.display()))?;
    verify_unix_server_peer(&stream, effective_uid()).with_context(|| {
        format!(
            "failed to authenticate Unix daemon endpoint '{}'",
            socket.display()
        )
    })?;
    stream
        .set_read_timeout(Some(DAEMON_SOCKET_TIMEOUT))
        .with_context(|| {
            format!(
                "failed to configure read timeout for '{}'",
                socket.display()
            )
        })?;
    stream
        .set_write_timeout(Some(DAEMON_SOCKET_TIMEOUT))
        .with_context(|| {
            format!(
                "failed to configure write timeout for '{}'",
                socket.display()
            )
        })?;
    Ok(DaemonStream::Unix(stream))
}

#[cfg(unix)]
fn verify_unix_server_peer(stream: &UnixStream, expected_uid: u32) -> Result<()> {
    let peer_uid = unix_peer_uid(stream)?;
    if peer_uid != expected_uid {
        anyhow::bail!(
            "Unix daemon peer uid {peer_uid} does not match client effective uid {expected_uid}"
        );
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn unix_peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `credentials` and `length` point to writable storage of the
    // declared sizes, and `stream` owns a live connected Unix socket.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Unix peer credential response had an unexpected size",
        ));
    }
    // SAFETY: a successful `getsockopt(SO_PEERCRED)` initialized the complete
    // `ucred` value after the exact returned length was validated.
    Ok(unsafe { credentials.assume_init() }.uid)
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn unix_peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: `uid` and `gid` are valid writable outputs and `stream` owns a
    // live connected Unix socket for the duration of the call.
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result == 0 {
        Ok(uid)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
fn unix_peer_uid(_stream: &UnixStream) -> std::io::Result<u32> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unix peer credential verification is unavailable on this platform",
    ))
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and does not retain pointers.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn stop_daemon_if_running(root: &Path) -> Result<()> {
    let endpoint = daemon_endpoint(root)?;
    if !endpoint_may_have_stale_socket(&endpoint) {
        return Ok(());
    }
    match send_request_existing_daemon(root, &DaemonRequest::Stop) {
        Ok(_) => Ok(()),
        Err(err) => {
            if connect_daemon_endpoint(&endpoint).is_ok() {
                Err(err)
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(unix)]
fn cleanup_unreachable_runtime_files(root: &Path) -> Result<()> {
    for path in [
        socket_path(root),
        workspace_socket_path(root),
        ready_path(root),
    ] {
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| {
                format!("failed to remove stale runtime file '{}'", path.display())
            })?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn wait_for_daemon_shutdown(root: &Path, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let endpoint = daemon_endpoint(root)?;
        if !endpoint_may_have_stale_socket(&endpoint) || connect_daemon_endpoint(&endpoint).is_err()
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(anyhow!(
        "packet28d did not stop; socket still reachable at '{}'",
        daemon_endpoint(root)?.address
    ))
}

#[cfg(unix)]
fn daemon_endpoint(root: &Path) -> Result<DaemonEndpoint> {
    match read_runtime_info(root) {
        Ok(runtime) if !runtime.socket_path.is_empty() => {
            if runtime.socket_path.starts_with("tcp://") && runtime.transport_auth.is_none() {
                anyhow::bail!(
                    "refusing legacy unauthenticated daemon TCP endpoint '{}'; stop that daemon \
                     with its matching Packet28 version and start it again",
                    runtime.socket_path
                );
            }
            Ok(DaemonEndpoint {
                address: runtime.socket_path,
                transport_auth: runtime.transport_auth,
            })
        }
        Ok(_) => Ok(default_daemon_endpoint(root)),
        Err(error) => match std::fs::symlink_metadata(runtime_path(root)) {
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                Ok(default_daemon_endpoint(root))
            }
            _ => Err(anyhow!(
                "failed to authenticate daemon runtime discovery '{}': {error}",
                runtime_path(root).display()
            )),
        },
    }
}

#[cfg(unix)]
fn default_daemon_endpoint(root: &Path) -> DaemonEndpoint {
    DaemonEndpoint {
        address: socket_path(root).to_string_lossy().to_string(),
        transport_auth: None,
    }
}

#[cfg(unix)]
fn endpoint_may_have_stale_socket(endpoint: &DaemonEndpoint) -> bool {
    endpoint
        .address
        .strip_prefix("tcp://")
        .map(|_| true)
        .unwrap_or_else(|| Path::new(&endpoint.address).exists())
}

#[cfg(unix)]
fn connect_daemon_endpoint(endpoint: &DaemonEndpoint) -> Result<DaemonStream> {
    if let Some(addr) = endpoint.address.strip_prefix("tcp://") {
        let auth = endpoint.transport_auth.as_ref().ok_or_else(|| {
            anyhow!(
                "daemon TCP endpoint '{}' has no owner capability",
                endpoint.address
            )
        })?;
        let stream = TcpStream::connect(addr).with_context(|| {
            format!(
                "failed to connect to daemon endpoint '{}'",
                endpoint.address
            )
        })?;
        stream
            .set_read_timeout(Some(DAEMON_SOCKET_TIMEOUT))
            .with_context(|| {
                format!(
                    "failed to configure read timeout for '{}'",
                    endpoint.address
                )
            })?;
        stream
            .set_write_timeout(Some(DAEMON_SOCKET_TIMEOUT))
            .with_context(|| {
                format!(
                    "failed to configure write timeout for '{}'",
                    endpoint.address
                )
            })?;
        let mut stream = stream;
        write_frame(&mut stream, auth).with_context(|| {
            format!(
                "failed to write authentication prelude to '{}'",
                endpoint.address
            )
        })?;
        match read_frame(&mut stream).with_context(|| {
            format!(
                "failed to read authentication response from '{}'",
                endpoint.address
            )
        })? {
            DaemonResponse::Ack { message } if message == "authenticated" => {}
            DaemonResponse::Error { message } => return Err(anyhow!(message)),
            response => {
                return Err(anyhow!(
                    "unexpected daemon authentication response from '{}': {response:?}",
                    endpoint.address
                ));
            }
        }
        return Ok(DaemonStream::Tcp(stream));
    }
    connect_daemon_socket(Path::new(&endpoint.address))
}

#[cfg(unix)]
fn packet28d_binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_packet28d") {
        return Ok(PathBuf::from(path));
    }
    let current = std::env::current_exe().context("failed to resolve current executable")?;
    let candidate = current
        .parent()
        .ok_or_else(|| anyhow!("missing executable parent"))?
        .join("packet28d");
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(anyhow!(
        "could not locate packet28d next to '{}'",
        current.display()
    ))
}

#[cfg(unix)]
fn ensure_executable(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("failed to inspect packet28d binary '{}'", path.display()))?;
    let mode = metadata.permissions().mode();
    if mode & 0o111 != 0 {
        return Ok(());
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(mode | 0o755);
    std::fs::set_permissions(path, permissions).with_context(|| {
        format!(
            "packet28d binary '{}' is not executable and could not be repaired",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_mismatch_errors_are_detected() {
        assert!(daemon_error_indicates_protocol_mismatch(
            "unknown variant `hook_ingest`, expected one of `execute`, `status` at line 1 column 21"
        ));
    }

    #[test]
    fn normal_daemon_errors_do_not_trigger_protocol_restart() {
        let response = DaemonResponse::Error {
            message: "prepare_handoff did not return a ready handoff".to_string(),
        };
        assert!(!daemon_response_indicates_protocol_mismatch(&response));
    }

    #[cfg(unix)]
    #[test]
    fn legacy_tcp_runtime_without_owner_capability_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        packet28_daemon_core::storage::write_runtime_info(
            root.path(),
            &packet28_daemon_protocol::message::DaemonRuntimeInfo {
                socket_path: "tcp://127.0.0.1:4242".to_string(),
                ..packet28_daemon_protocol::message::DaemonRuntimeInfo::default()
            },
        )
        .unwrap();

        let error = daemon_endpoint(root.path())
            .err()
            .expect("legacy unauthenticated TCP discovery unexpectedly succeeded");

        assert!(error
            .to_string()
            .contains("refusing legacy unauthenticated daemon TCP endpoint"));
    }

    #[cfg(unix)]
    #[test]
    fn runtime_discovery_symlink_is_not_treated_as_missing() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let runtime = runtime_path(root.path());
        std::fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        symlink(root.path().join("missing-runtime-target"), &runtime).unwrap();

        let error = daemon_endpoint(root.path())
            .err()
            .expect("unauthenticated runtime symlink unexpectedly fell back to a Unix endpoint");

        assert!(error
            .to_string()
            .contains("failed to authenticate daemon runtime discovery"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_daemon_peer_authentication_accepts_the_effective_user() {
        use std::os::unix::net::UnixListener;

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let stream = UnixStream::connect(&socket).unwrap();
        let _accepted = listener.accept().unwrap();

        verify_unix_server_peer(&stream, effective_uid()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unix_daemon_peer_authentication_rejects_substituted_owner() {
        use std::os::unix::net::UnixListener;

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let stream = UnixStream::connect(&socket).unwrap();
        let _accepted = listener.accept().unwrap();

        let error = verify_unix_server_peer(&stream, effective_uid() ^ 1).unwrap_err();

        assert!(error
            .to_string()
            .contains("does not match client effective uid"));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_executable_repairs_packaged_daemon_mode() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = dir.path().join("packet28d");
        std::fs::File::create(&daemon)
            .unwrap()
            .write_all(b"#!/bin/sh\n")
            .unwrap();
        let mut permissions = std::fs::metadata(&daemon).unwrap().permissions();
        permissions.set_mode(0o644);
        std::fs::set_permissions(&daemon, permissions).unwrap();

        ensure_executable(&daemon).unwrap();

        let mode = std::fs::metadata(&daemon).unwrap().permissions().mode();
        assert_ne!(mode & 0o111, 0);
    }
}
