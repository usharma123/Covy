use std::cell::Cell;
use std::ffi::{c_char, CStr, CString};
use std::fs;
use std::io::{BufReader, BufWriter};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use packet28_daemon_core::{
    read_socket_message, resolve_workspace_root, socket_path, write_socket_message,
    ContextBackendKind, ContextResolveOutcome, ContextResolveRequest, ContextSourceKind,
    DaemonRequest, DaemonResponse, InstructionFileResolveOutcome,
};
use sha2::{Digest, Sha256};

static INITIALIZED: AtomicBool = AtomicBool::new(false);

#[cfg(not(test))]
unsafe extern "C" {
    fn context_instruct_shim_macos_interpose_anchor();
}

#[cfg(not(test))]
#[used]
static FORCE_INTERPOSE_LINK: unsafe extern "C" fn() = context_instruct_shim_macos_interpose_anchor;

#[unsafe(no_mangle)]
pub extern "C" fn context_instruct_shim_set_initialized(value: libc::c_int) {
    INITIALIZED.store(value != 0, Ordering::Relaxed);
}

thread_local! {
    static INTERCEPT_DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct InterceptGuard;

impl Drop for InterceptGuard {
    fn drop(&mut self) {
        INTERCEPT_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

struct InterceptCandidate {
    root: PathBuf,
    relative_path: String,
    absolute_path: PathBuf,
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must be a valid C string pointer accepted by `open(2)`, and the
/// caller must uphold the platform ABI contract for `flags` and `mode`.
pub unsafe extern "C" fn context_instruct_shim_open(
    path: *const c_char,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> libc::c_int {
    if !INITIALIZED.load(Ordering::Relaxed) {
        return unsafe { libc::open(path, flags, libc::c_uint::from(mode)) };
    }
    if let Some(fd) = maybe_virtualize(path, None) {
        return fd;
    }
    unsafe { libc::open(path, flags, libc::c_uint::from(mode)) }
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must be a valid C string pointer accepted by `openat(2)`, `dirfd`
/// must be a valid directory file descriptor or platform sentinel, and the
/// caller must uphold the platform ABI contract for `flags` and `mode`.
pub unsafe extern "C" fn context_instruct_shim_openat(
    dirfd: libc::c_int,
    path: *const c_char,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> libc::c_int {
    if !INITIALIZED.load(Ordering::Relaxed) {
        return unsafe { libc::openat(dirfd, path, flags, libc::c_uint::from(mode)) };
    }
    if let Some(fd) = maybe_virtualize(path, Some(dirfd)) {
        return fd;
    }
    unsafe { libc::openat(dirfd, path, flags, libc::c_uint::from(mode)) }
}

fn maybe_virtualize(path: *const c_char, dirfd: Option<libc::c_int>) -> Option<libc::c_int> {
    if path.is_null() || intercept_disabled() {
        return None;
    }

    let candidate = with_intercept_disabled(|| detect_candidate(path, dirfd))?;
    let bytes = with_intercept_disabled(|| fs::read(&candidate.absolute_path).ok())?;
    let content = String::from_utf8(bytes).ok()?;
    let content_sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
    let response = with_intercept_disabled(|| {
        resolve_instruction_file(
            &candidate.root,
            &candidate.relative_path,
            &content,
            &content_sha256,
        )
    })?;

    match response.outcome {
        ContextResolveOutcome::Rewrite {
            content,
            task_label,
            original_bytes,
            rewritten_bytes,
            ..
        } => {
            let fd = create_temp_fd("context-instruct-shim", content.as_bytes())?;
            debug_log(&format!(
                "p28 virtualized path={} task={} original_bytes={} rewritten_bytes={}",
                candidate.absolute_path.display(),
                task_label,
                original_bytes,
                rewritten_bytes
            ));
            Some(fd)
        }
        ContextResolveOutcome::Passthrough {
            reason,
            original_bytes,
            ..
        } => {
            debug_log(&format!(
                "p28 passthrough path={} reason={} original_bytes={}",
                candidate.absolute_path.display(),
                reason,
                original_bytes.unwrap_or(content.len())
            ));
            None
        }
    }
}

fn detect_candidate(path: *const c_char, dirfd: Option<libc::c_int>) -> Option<InterceptCandidate> {
    let raw_path = unsafe { CStr::from_ptr(path) }.to_str().ok()?;
    let absolute_path = resolve_absolute_path(raw_path, dirfd)?;
    let file_name = absolute_path.file_name()?.to_str()?;
    if !matches!(file_name, "AGENTS.md" | "AGENTS.MD" | "CLAUDE.md") {
        return None;
    }
    let root_probe = absolute_path.parent().unwrap_or_else(|| Path::new("/"));
    let root = resolve_workspace_root(root_probe);
    let expected = root.join(file_name);
    if normalize_path(&absolute_path) != normalize_path(&expected) {
        return None;
    }
    Some(InterceptCandidate {
        root,
        relative_path: file_name.to_string(),
        absolute_path: expected,
    })
}

fn resolve_absolute_path(raw_path: &str, dirfd: Option<libc::c_int>) -> Option<PathBuf> {
    let path = PathBuf::from(raw_path);
    let absolute = if path.is_absolute() {
        path
    } else {
        let base = match dirfd {
            Some(fd) if fd != libc::AT_FDCWD => resolve_dirfd_path(fd)?,
            _ => std::env::current_dir().ok()?,
        };
        let base_dir = if base.is_dir() {
            base
        } else {
            base.parent()?.to_path_buf()
        };
        base_dir.join(path)
    };
    Some(normalize_path(&absolute))
}

fn resolve_dirfd_path(dirfd: libc::c_int) -> Option<PathBuf> {
    let mut buffer = [0 as c_char; libc::PATH_MAX as usize];
    let result = unsafe { libc::fcntl(dirfd, libc::F_GETPATH, buffer.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    Some(PathBuf::from(
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    ))
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn resolve_instruction_file(
    root: &Path,
    relative_path: &str,
    content: &str,
    content_sha256: &str,
) -> Option<packet28_daemon_core::ContextResolveResponse> {
    let socket = socket_path(root);
    if !socket.exists() {
        debug_log(&format!(
            "p28 passthrough path={} reason=daemon_socket_missing",
            root.join(relative_path).display()
        ));
        return None;
    }
    let stream = UnixStream::connect(&socket).ok()?;
    let timeout = Duration::from_millis(50);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let reader_stream = stream.try_clone().ok()?;
    let mut writer = BufWriter::new(stream);
    let mut reader = BufReader::new(reader_stream);
    let request = DaemonRequest::ContextResolve {
        request: ContextResolveRequest {
            workspace_root: root.to_string_lossy().to_string(),
            source_kind: ContextSourceKind::InstructionFile,
            source_path: Some(relative_path.to_string()),
            source_sha256: content_sha256.to_string(),
            source_content: content.to_string(),
            task_id: None,
            task_label: None,
            budget_tokens: Some(512),
            schema_version: 1,
            agent_family: Some(detect_agent_family()),
            backend_kind: ContextBackendKind::Unknown,
        },
    };
    write_socket_message(&mut writer, &request).ok()?;
    match read_socket_message::<_, DaemonResponse>(&mut reader).ok()? {
        DaemonResponse::ContextResolve { response } => Some(response),
        DaemonResponse::InstructionFileResolve { response } => {
            Some(packet28_daemon_core::ContextResolveResponse {
                source_kind: ContextSourceKind::InstructionFile,
                source_path: Some(response.path.clone()),
                outcome: match response.outcome {
                    InstructionFileResolveOutcome::Rewrite {
                        content,
                        content_sha256,
                        task_label,
                        original_bytes,
                        rewritten_bytes,
                        cache_hit,
                        matched_terms,
                        section_titles,
                    } => ContextResolveOutcome::Rewrite {
                        content,
                        content_sha256,
                        task_label,
                        original_bytes,
                        rewritten_bytes,
                        cache_hit,
                        matched_terms,
                        section_titles,
                        schema_version: 1,
                    },
                    InstructionFileResolveOutcome::Passthrough {
                        reason,
                        content_sha256,
                        task_label,
                        original_bytes,
                    } => ContextResolveOutcome::Passthrough {
                        reason,
                        content_sha256,
                        task_label,
                        original_bytes,
                    },
                },
            })
        }
        DaemonResponse::Error { message } => {
            debug_log(&format!(
                "p28 passthrough path={} reason=daemon_error:{}",
                root.join(relative_path).display(),
                message
            ));
            None
        }
        _ => None,
    }
}

fn create_temp_fd(name: &str, content: &[u8]) -> Option<libc::c_int> {
    let template = format!("{}/{}-XXXXXX", std::env::temp_dir().to_string_lossy(), name);
    let mut bytes = CString::new(template).ok()?.into_bytes_with_nul();
    let fd = unsafe { libc::mkstemp(bytes.as_mut_ptr().cast()) };
    if fd < 0 {
        return None;
    }
    let _ = unsafe { libc::unlink(bytes.as_ptr().cast()) };
    if !write_all_fd(fd, content) {
        unsafe {
            libc::close(fd);
        }
        return None;
    }
    unsafe {
        libc::lseek(fd, 0, libc::SEEK_SET);
    }
    Some(fd)
}

fn write_all_fd(fd: libc::c_int, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written <= 0 {
            return false;
        }
        bytes = &bytes[written as usize..];
    }
    true
}

fn with_intercept_disabled<T>(f: impl FnOnce() -> T) -> T {
    let _guard = disable_intercept();
    f()
}

fn disable_intercept() -> InterceptGuard {
    INTERCEPT_DEPTH.with(|depth| depth.set(depth.get() + 1));
    InterceptGuard
}

fn intercept_disabled() -> bool {
    INTERCEPT_DEPTH.with(|depth| depth.get() > 0)
}

fn debug_log(message: &str) {
    if !debug_enabled() {
        return;
    }
    let _guard = disable_intercept();
    let mut line = String::from(message);
    line.push('\n');
    let _ = unsafe { libc::write(libc::STDERR_FILENO, line.as_ptr().cast(), line.len()) };
}

fn debug_enabled() -> bool {
    std::env::var_os("P28_DEBUG").is_some_and(|value| {
        let normalized = value.to_string_lossy().trim().to_ascii_lowercase();
        !matches!(normalized.as_str(), "" | "0" | "false" | "no" | "off")
    })
}

fn detect_agent_family() -> String {
    std::env::var("PACKET28_AGENT_FAMILY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "generic".to_string())
}
