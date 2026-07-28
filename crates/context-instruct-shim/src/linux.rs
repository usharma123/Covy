use std::cell::Cell;
use std::ffi::{c_char, CString};
use std::fs;
use std::io::{BufReader, BufWriter};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use packet28_daemon_protocol::{
    frame::{read_frame, write_frame},
    message::{
        ContextBackendKind, ContextResolveOutcome, ContextResolveRequest, ContextResolveResponse,
        ContextSourceKind, DaemonRequest, DaemonResponse, InstructionFileResolveOutcome,
    },
    paths::{resolve_workspace_root, socket_path},
};
use sha2::{Digest, Sha256};

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

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("the Linux preload shim supports only x86_64 and aarch64");

#[cfg(not(test))]
macro_rules! interpose_trampoline {
    ($name:ident, $bridge:literal) => {
        /// ELF-exported tail trampoline into a C variadic bridge.
        ///
        /// The naked body never reads, writes, or retypes argument registers;
        /// C remains the sole owner of the variadic ABI.
        ///
        /// # Safety
        ///
        /// This symbol is for the dynamic loader, not Rust callers. It must be
        /// invoked using the platform libc contract for the correspondingly
        /// named variadic function.
        #[doc(hidden)]
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name() -> libc::c_int {
            #[cfg(target_arch = "x86_64")]
            core::arch::naked_asm!(concat!("jmp ", $bridge));
            #[cfg(target_arch = "aarch64")]
            core::arch::naked_asm!(concat!("b ", $bridge));
        }
    };
}

#[cfg(not(test))]
interpose_trampoline!(open, "context_instruct_shim_linux_open");
#[cfg(not(test))]
interpose_trampoline!(open64, "context_instruct_shim_linux_open64");
#[cfg(not(test))]
interpose_trampoline!(openat, "context_instruct_shim_linux_openat");
#[cfg(not(test))]
interpose_trampoline!(openat64, "context_instruct_shim_linux_openat64");

/// Attempt to virtualize a path intercepted by the Linux `open` or `open64`
/// bridge.
///
/// Returns `1` and writes a replacement descriptor to `replacement_fd` when
/// the path was virtualized. Returns `0` without modifying `replacement_fd`
/// when the C bridge must call the real libc symbol.
///
/// # Safety
///
/// `path` must point to a valid NUL-terminated C string for the duration of
/// this call. `replacement_fd` must be non-null, properly aligned, and valid
/// for writing one `libc::c_int`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn context_instruct_shim_linux_try_open(
    path: *const c_char,
    replacement_fd: *mut libc::c_int,
) -> libc::c_int {
    if replacement_fd.is_null() {
        return 0;
    }
    let Some(fd) = maybe_virtualize(path, None) else {
        return 0;
    };
    // SAFETY: The caller guarantees that `replacement_fd` is writable, and
    // the null case was rejected above.
    unsafe {
        replacement_fd.write(fd);
    }
    1
}

/// Attempt to virtualize a path intercepted by the Linux `openat` or
/// `openat64` bridge.
///
/// Returns `1` and writes a replacement descriptor to `replacement_fd` when
/// the path was virtualized. Returns `0` without modifying `replacement_fd`
/// when the C bridge must call the real libc symbol.
///
/// # Safety
///
/// `path` must point to a valid NUL-terminated C string for the duration of
/// this call. `dirfd` must be `AT_FDCWD` or a descriptor suitable for
/// resolving `path`. `replacement_fd` must be non-null, properly aligned, and
/// valid for writing one `libc::c_int`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn context_instruct_shim_linux_try_openat(
    dirfd: libc::c_int,
    path: *const c_char,
    replacement_fd: *mut libc::c_int,
) -> libc::c_int {
    if replacement_fd.is_null() {
        return 0;
    }
    let Some(fd) = maybe_virtualize(path, Some(dirfd)) else {
        return 0;
    };
    // SAFETY: The caller guarantees that `replacement_fd` is writable, and
    // the null case was rejected above.
    unsafe {
        replacement_fd.write(fd);
    }
    1
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
    // SAFETY: The fixed C callback contract requires a valid NUL-terminated
    // path pointer for the duration of this call.
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
) -> Option<ContextResolveResponse> {
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
            render_mode: crate::configured_instruction_mode(),
            stable_config: None,
            task_id: None,
            task_label: None,
            budget_tokens: Some(512),
            schema_version: 1,
            agent_family: Some(detect_agent_family()),
            backend_kind: ContextBackendKind::LinuxPreload,
        },
    };
    write_frame(&mut writer, &request).ok()?;
    match read_frame::<_, DaemonResponse>(&mut reader).ok()? {
        DaemonResponse::ContextResolve { response } => Some(response),
        DaemonResponse::InstructionFileResolve { response } => Some(ContextResolveResponse {
            source_kind: ContextSourceKind::InstructionFile,
            source_path: Some(response.path.clone()),
            outcome: match response.outcome {
                InstructionFileResolveOutcome::Rewrite {
                    content,
                    content_sha256,
                    render_mode,
                    stable_config_sha256,
                    snapshot_sha256,
                    rendered_sha256,
                    task_label,
                    original_bytes,
                    rewritten_bytes,
                    cache_hit,
                    matched_terms,
                    section_titles,
                } => ContextResolveOutcome::Rewrite {
                    content,
                    content_sha256,
                    render_mode,
                    stable_config_sha256,
                    snapshot_sha256,
                    rendered_sha256,
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
        }),
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
    // SAFETY: `cname` is a live NUL-terminated string, and the flags are valid
    // for Linux `memfd_create(2)`.
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
        // SAFETY: `fd` was returned by `memfd_create` and remains owned here.
        unsafe {
            libc::close(fd);
        }
        return None;
    }
    // SAFETY: `fd` is an open descriptor owned by this function.
    unsafe {
        libc::lseek(fd, 0, libc::SEEK_SET);
    }
    Some(fd)
}

fn write_all_fd(fd: libc::c_int, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        // SAFETY: `bytes` is valid for `bytes.len()` reads and `fd` is an open
        // descriptor supplied by `create_memfd`.
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
    // SAFETY: `line` is live for the duration of the write and stderr is a
    // process-owned descriptor. Logging is intentionally best-effort.
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
