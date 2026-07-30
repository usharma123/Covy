//! Authenticated client connections to the endpoint published by `packet28d`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::fd::AsRawFd as _;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use packet28_daemon_protocol::frame::{read_frame, write_frame, FrameError};
use packet28_daemon_protocol::message::{DaemonResponse, DaemonTransportAuth};
use packet28_daemon_protocol::paths::socket_path;
use thiserror::Error;

use crate::runtime_discovery::{read_runtime_info_if_present, RuntimeDiscoveryError};

/// A daemon endpoint selected from authenticated runtime discovery.
#[derive(Clone)]
pub struct DaemonEndpoint {
    address: String,
    transport_auth: Option<DaemonTransportAuth>,
}

impl DaemonEndpoint {
    /// Returns the Unix path or `tcp://` address published for the daemon.
    pub fn address(&self) -> &str {
        &self.address
    }
}

impl std::fmt::Debug for DaemonEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DaemonEndpoint")
            .field("address", &self.address)
            .field("transport_auth", &self.transport_auth)
            .finish()
    }
}

/// A connected daemon byte stream.
#[derive(Debug)]
pub enum DaemonStream {
    /// Mutually authenticated Unix-domain socket.
    Unix(UnixStream),
    /// Capability-authenticated loopback TCP socket.
    Tcp(TcpStream),
}

impl DaemonStream {
    /// Clones the underlying socket handle.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error from cloning the connected socket.
    pub fn try_clone(&self) -> std::io::Result<Self> {
        match self {
            Self::Unix(stream) => stream.try_clone().map(Self::Unix),
            Self::Tcp(stream) => stream.try_clone().map(Self::Tcp),
        }
    }
}

impl Read for DaemonStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Unix(stream) => stream.read(buffer),
            Self::Tcp(stream) => stream.read(buffer),
        }
    }
}

impl Write for DaemonStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Unix(stream) => stream.write(buffer),
            Self::Tcp(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Unix(stream) => stream.flush(),
            Self::Tcp(stream) => stream.flush(),
        }
    }
}

/// Failure to discover or authenticate a daemon connection.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DaemonClientError {
    /// Runtime discovery metadata or its namespace could not be authenticated.
    #[error(transparent)]
    Discovery(#[from] RuntimeDiscoveryError),
    /// A legacy TCP endpoint has no per-instance capability.
    #[error(
        "refusing legacy unauthenticated daemon TCP endpoint '{endpoint}'; stop that daemon with \
         its matching Packet28 version and start it again"
    )]
    LegacyUnauthenticatedTcp {
        /// Rejected endpoint.
        endpoint: String,
    },
    /// A socket operation failed.
    #[error("{operation} '{endpoint}': {source}")]
    Io {
        /// Operation that failed.
        operation: &'static str,
        /// Endpoint being accessed.
        endpoint: String,
        /// Operating-system error.
        #[source]
        source: std::io::Error,
    },
    /// A framed authentication operation failed.
    #[error("{operation} '{endpoint}': {source}")]
    Frame {
        /// Operation that failed.
        operation: &'static str,
        /// Endpoint being accessed.
        endpoint: String,
        /// Framing error.
        #[source]
        source: FrameError,
    },
    /// The daemon explicitly rejected the TCP capability.
    #[error("daemon TCP authentication at '{endpoint}' was rejected: {message}")]
    AuthenticationRejected {
        /// Rejected endpoint.
        endpoint: String,
        /// Daemon response.
        message: String,
    },
    /// The daemon returned a non-authentication response to the prelude.
    #[error("unexpected daemon authentication response from '{endpoint}': {response:?}")]
    UnexpectedAuthenticationResponse {
        /// Endpoint that returned the response.
        endpoint: String,
        /// Unexpected response.
        response: Box<DaemonResponse>,
    },
}

/// Discovers the authoritative endpoint for `root`.
///
/// A missing authenticated runtime publication uses the conventional Unix
/// endpoint for compatibility. Any present but unauthentic discovery state
/// fails closed.
///
/// # Errors
///
/// Returns [`DaemonClientError::Discovery`] when published state cannot be
/// authenticated or decoded, and
/// [`DaemonClientError::LegacyUnauthenticatedTcp`] when an older runtime
/// advertises TCP without a per-instance capability.
pub fn discover_endpoint(root: &Path) -> Result<DaemonEndpoint, DaemonClientError> {
    let Some(runtime) = read_runtime_info_if_present(root)? else {
        return Ok(default_endpoint(root));
    };
    if runtime.socket_path.is_empty() {
        return Ok(default_endpoint(root));
    }
    if runtime.socket_path.starts_with("tcp://") && runtime.transport_auth.is_none() {
        return Err(DaemonClientError::LegacyUnauthenticatedTcp {
            endpoint: runtime.socket_path,
        });
    }
    Ok(DaemonEndpoint {
        address: runtime.socket_path,
        transport_auth: runtime.transport_auth,
    })
}

/// Returns whether a discovered endpoint can leave a stale socket artifact.
pub fn endpoint_may_have_stale_socket(endpoint: &DaemonEndpoint) -> bool {
    endpoint.address.starts_with("tcp://") || Path::new(&endpoint.address).exists()
}

/// Discovers and connects to the authoritative daemon endpoint.
///
/// Unix server credentials or the TCP capability prelude are authenticated
/// before this function returns, so callers cannot send a request frame to an
/// unauthenticated peer.
///
/// # Errors
///
/// Returns [`DaemonClientError`] when discovery, connection setup, Unix peer
/// verification, or the TCP authentication prelude fails.
pub fn connect(root: &Path, timeout: Duration) -> Result<DaemonStream, DaemonClientError> {
    let endpoint = discover_endpoint(root)?;
    connect_endpoint(&endpoint, timeout)
}

/// Connects to a previously authenticated discovery result.
///
/// # Errors
///
/// Returns [`DaemonClientError`] when the socket cannot be connected or
/// configured, the Unix peer has the wrong effective user, or the TCP
/// capability exchange fails.
pub fn connect_endpoint(
    endpoint: &DaemonEndpoint,
    timeout: Duration,
) -> Result<DaemonStream, DaemonClientError> {
    if let Some(address) = endpoint.address.strip_prefix("tcp://") {
        return connect_tcp(address, endpoint, timeout);
    }
    connect_unix(Path::new(&endpoint.address), timeout)
}

fn default_endpoint(root: &Path) -> DaemonEndpoint {
    DaemonEndpoint {
        address: socket_path(root).to_string_lossy().to_string(),
        transport_auth: None,
    }
}

fn connect_unix(path: &Path, timeout: Duration) -> Result<DaemonStream, DaemonClientError> {
    let endpoint = path.to_string_lossy().to_string();
    let stream = UnixStream::connect(path).map_err(|source| DaemonClientError::Io {
        operation: "failed to connect to",
        endpoint: endpoint.clone(),
        source,
    })?;
    verify_unix_server_peer(&stream, effective_uid()).map_err(|source| DaemonClientError::Io {
        operation: "failed to authenticate daemon peer",
        endpoint: endpoint.clone(),
        source,
    })?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|source| DaemonClientError::Io {
            operation: "failed to configure read timeout for",
            endpoint: endpoint.clone(),
            source,
        })?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|source| DaemonClientError::Io {
            operation: "failed to configure write timeout for",
            endpoint,
            source,
        })?;
    Ok(DaemonStream::Unix(stream))
}

fn connect_tcp(
    address: &str,
    endpoint: &DaemonEndpoint,
    timeout: Duration,
) -> Result<DaemonStream, DaemonClientError> {
    let auth = endpoint.transport_auth.as_ref().ok_or_else(|| {
        DaemonClientError::LegacyUnauthenticatedTcp {
            endpoint: endpoint.address.clone(),
        }
    })?;
    let mut stream = TcpStream::connect(address).map_err(|source| DaemonClientError::Io {
        operation: "failed to connect to daemon endpoint",
        endpoint: endpoint.address.clone(),
        source,
    })?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|source| DaemonClientError::Io {
            operation: "failed to configure read timeout for",
            endpoint: endpoint.address.clone(),
            source,
        })?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|source| DaemonClientError::Io {
            operation: "failed to configure write timeout for",
            endpoint: endpoint.address.clone(),
            source,
        })?;
    write_frame(&mut stream, auth).map_err(|source| DaemonClientError::Frame {
        operation: "failed to write authentication prelude to",
        endpoint: endpoint.address.clone(),
        source,
    })?;
    match read_frame(&mut stream).map_err(|source| DaemonClientError::Frame {
        operation: "failed to read authentication response from",
        endpoint: endpoint.address.clone(),
        source,
    })? {
        DaemonResponse::Ack { message } if message == "authenticated" => {
            Ok(DaemonStream::Tcp(stream))
        }
        DaemonResponse::Error { message } => Err(DaemonClientError::AuthenticationRejected {
            endpoint: endpoint.address.clone(),
            message,
        }),
        response => Err(DaemonClientError::UnexpectedAuthenticationResponse {
            endpoint: endpoint.address.clone(),
            response: Box::new(response),
        }),
    }
}

fn verify_unix_server_peer(stream: &UnixStream, expected_uid: u32) -> std::io::Result<()> {
    let peer_uid = unix_peer_uid(stream)?;
    if peer_uid == expected_uid {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "Unix daemon peer uid {peer_uid} does not match client effective uid \
                 {expected_uid}"
            ),
        ))
    }
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

fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and retains no pointers.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::net::UnixListener;
    use std::thread;

    use packet28_daemon_protocol::message::{DaemonRuntimeInfo, DAEMON_TRANSPORT_SECRET_BYTES};
    use packet28_daemon_protocol::paths::{runtime_path, workspace_socket_path};

    fn write_runtime(root: &Path, runtime: &DaemonRuntimeInfo) {
        let path = runtime_path(root);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec(runtime).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn connect_uses_authoritative_workspace_unix_endpoint() {
        let root = tempfile::tempdir().unwrap();
        let socket = workspace_socket_path(root.path());
        fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        write_runtime(
            root.path(),
            &DaemonRuntimeInfo {
                socket_path: socket.to_string_lossy().to_string(),
                ..DaemonRuntimeInfo::default()
            },
        );

        let stream = connect(root.path(), Duration::from_secs(1)).unwrap();
        let _accepted = listener.accept().unwrap();

        assert!(matches!(stream, DaemonStream::Unix(_)));
    }

    #[test]
    fn connect_authenticates_tcp_endpoint_before_returning() {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let auth = DaemonTransportAuth::from_secret_bytes([0x4d; DAEMON_TRANSPORT_SECRET_BYTES]);
        write_runtime(
            root.path(),
            &DaemonRuntimeInfo {
                socket_path: format!("tcp://{}", listener.local_addr().unwrap()),
                transport_auth: Some(auth.clone()),
                ..DaemonRuntimeInfo::default()
            },
        );
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let received: DaemonTransportAuth = read_frame(&mut stream).unwrap();
            assert!(auth.authenticates(&received));
            write_frame(
                &mut stream,
                &DaemonResponse::Ack {
                    message: "authenticated".to_string(),
                },
            )
            .unwrap();
        });

        let stream = connect(root.path(), Duration::from_secs(1)).unwrap();

        assert!(matches!(stream, DaemonStream::Tcp(_)));
        server.join().unwrap();
    }

    #[test]
    fn legacy_tcp_endpoint_without_capability_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        write_runtime(
            root.path(),
            &DaemonRuntimeInfo {
                socket_path: "tcp://127.0.0.1:4242".to_string(),
                ..DaemonRuntimeInfo::default()
            },
        );

        let error = connect(root.path(), Duration::from_secs(1)).unwrap_err();

        assert!(matches!(
            error,
            DaemonClientError::LegacyUnauthenticatedTcp { .. }
        ));
    }

    #[test]
    fn unix_peer_verification_accepts_owner_and_rejects_substituted_uid() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("peer.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let stream = UnixStream::connect(&socket).unwrap();
        let _accepted = listener.accept().unwrap();

        verify_unix_server_peer(&stream, effective_uid()).unwrap();
        let error = verify_unix_server_peer(&stream, effective_uid() ^ 1).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }
}
