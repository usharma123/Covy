use std::cell::Cell;
use std::ffi::{c_char, CString};
use std::fs;
use std::io::{BufReader, BufWriter};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use packet28_daemon_core::{
    read_socket_message, resolve_workspace_root, socket_path, write_socket_message,
    ContextBackendKind, ContextResolveOutcome, ContextResolveRequest, ContextSourceKind,
    DaemonRequest, DaemonResponse, InstructionFileResolveOutcome,
};
use sha2::{Digest, Sha256};

type OpenFn = unsafe extern "C" fn(*const c_char, libc::c_int, libc::mode_t) -> libc::c_int;
type OpenAtFn =
    unsafe extern "C" fn(libc::c_int, *const c_char, libc::c_int, libc::mode_t) -> libc::c_int;

static REAL_OPEN: OnceLock<OpenFn> = OnceLock::new();
static REAL_OPEN64: OnceLock<Option<OpenFn>> = OnceLock::new();
static REAL_OPENAT: OnceLock<OpenAtFn> = OnceLock::new();
static REAL_OPENAT64: OnceLock<Option<OpenAtFn>> = OnceLock::new();

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
pub unsafe extern "C" fn open(
    path: *const c_char,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> libc::c_int {
    if let Some(fd) = maybe_virtualize(path, None) {
        return fd;
    }
    call_real_open(real_open(), path, flags, mode)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn open64(
    path: *const c_char,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> libc::c_int {
    if let Some(fd) = maybe_virtualize(path, None) {
        return fd;
    }
    let real = real_open64().unwrap_or_else(real_open);
    call_real_open(real, path, flags, mode)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn openat(
    dirfd: libc::c_int,
    path: *const c_char,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> libc::c_int {
    if let Some(fd) = maybe_virtualize(path, Some(dirfd)) {
        return fd;
    }
    call_real_openat(real_openat(), dirfd, path, flags, mode)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn openat64(
    dirfd: libc::c_int,
    path: *const c_char,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> libc::c_int {
    if let Some(fd) = maybe_virtualize(path, Some(dirfd)) {
        return fd;
    }
    let real = real_openat64().unwrap_or_else(real_openat);
    call_real_openat(real, dirfd, path, flags, mode)
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
            let fd = create_memfd("context-instruct-shim", content.as_bytes())?;
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
    let raw_path = unsafe { std::ffi::CStr::from_ptr(path) }.to_str().ok()?;
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
    fs::read_link(format!("/proc/self/fd/{dirfd}")).ok()
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
            backend_kind: ContextBackendKind::LinuxPreload,
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

fn create_memfd(name: &str, content: &[u8]) -> Option<libc::c_int> {
    let cname = CString::new(name).ok()?;
    let fd = unsafe {
        libc::syscall(
            libc::SYS_memfd_create,
            cname.as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        ) as libc::c_int
    };
    if fd < 0 {
        return None;
    }
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

fn flags_require_mode(flags: libc::c_int) -> bool {
    (flags & libc::O_CREAT) != 0 || (flags & libc::O_TMPFILE) != 0
}

unsafe fn call_real_open(
    real: OpenFn,
    path: *const c_char,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> libc::c_int {
    real(path, flags, mode)
}

unsafe fn call_real_openat(
    real: OpenAtFn,
    dirfd: libc::c_int,
    path: *const c_char,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> libc::c_int {
    real(dirfd, path, flags, mode)
}

fn real_open() -> OpenFn {
    *REAL_OPEN.get_or_init(|| unsafe { load_symbol(b"open\0") })
}

fn real_open64() -> Option<OpenFn> {
    *REAL_OPEN64.get_or_init(|| unsafe { load_optional_symbol(b"open64\0") })
}

fn real_openat() -> OpenAtFn {
    *REAL_OPENAT.get_or_init(|| unsafe { load_symbol(b"openat\0") })
}

fn real_openat64() -> Option<OpenAtFn> {
    *REAL_OPENAT64.get_or_init(|| unsafe { load_optional_symbol(b"openat64\0") })
}

unsafe fn load_symbol<T: Copy>(symbol: &[u8]) -> T {
    let ptr = libc::dlsym(libc::RTLD_NEXT, symbol.as_ptr().cast());
    assert!(!ptr.is_null(), "missing required libc symbol");
    std::mem::transmute_copy(&ptr)
}

unsafe fn load_optional_symbol<T: Copy>(symbol: &[u8]) -> Option<T> {
    let ptr = libc::dlsym(libc::RTLD_NEXT, symbol.as_ptr().cast());
    if ptr.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&ptr))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_repo_root_targets_only() {
        let root = PathBuf::from("/tmp/demo");
        let candidate = normalize_path(&root.join("AGENTS.md"));
        let expected = normalize_path(&root.join("AGENTS.md"));
        assert_eq!(candidate, expected);
    }

    #[test]
    fn ignores_nested_instruction_files() {
        let root = PathBuf::from("/tmp/demo");
        let nested = root.join("docs").join("AGENTS.md");
        let expected = root.join("AGENTS.md");
        assert_ne!(normalize_path(&nested), normalize_path(&expected));
    }
}
