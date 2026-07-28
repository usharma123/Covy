use ignore::WalkBuilder;
use serde::Serialize;
use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

pub const MAX_REGEX_FILE_BYTES: usize = 2 * 1024 * 1024;

struct CountingAllocator;

static COUNT_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static REALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static DEALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static REQUESTED_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: Every operation is delegated to the system allocator with the
// original pointer and layout. The atomics only observe successful operations
// and do not alter allocation behavior.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplied `layout` under the GlobalAlloc contract.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
            REQUESTED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            DEALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
            DEALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        // SAFETY: The caller supplied the pointer/layout pair under the
        // GlobalAlloc contract, unchanged by this wrapper.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller supplied the pointer/layout pair and new size
        // under the GlobalAlloc contract.
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        if !resized.is_null() && COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            REALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
            REQUESTED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        }
        resized
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AllocationTelemetry {
    pub allocation_calls: u64,
    pub reallocation_calls: u64,
    pub deallocation_calls: u64,
    pub requested_bytes: u64,
    pub deallocated_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct IoTelemetry {
    pub walk_passes: u64,
    pub walked_entries: u64,
    pub metadata_calls: u64,
    pub successful_read_calls: u64,
    pub bytes_read: u64,
    pub peak_retained_content_files: u64,
    pub peak_retained_content_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsumerSummary {
    pub documents: u64,
    pub logical_bytes: u64,
    pub digest_blake3: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanOutput {
    pub map: ConsumerSummary,
    pub regex: ConsumerSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MeasuredScan {
    pub elapsed_nanos: u64,
    pub allocations: AllocationTelemetry,
    pub io: IoTelemetry,
    pub output: ScanOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FixtureManifest {
    pub name: String,
    pub files_written: u64,
    pub bytes_written: u64,
    pub regular_source_files: u64,
    pub test_source_files: u64,
    pub regex_only_text_files: u64,
    pub oversize_source_files: u64,
    pub oversize_regex_only_files: u64,
    pub binary_files: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct FixtureSpec {
    pub name: &'static str,
    pub source_files: usize,
    pub source_bytes: usize,
    pub test_files: usize,
    pub test_bytes: usize,
    pub text_files: usize,
    pub text_bytes: usize,
    pub oversize_source_files: usize,
    pub oversize_text_files: usize,
    pub binary_files: usize,
    pub binary_bytes: usize,
}

pub const SMALL_FIXTURE: FixtureSpec = FixtureSpec {
    name: "small-files",
    source_files: 320,
    source_bytes: 4 * 1024,
    test_files: 80,
    test_bytes: 6 * 1024,
    text_files: 100,
    text_bytes: 8 * 1024,
    oversize_source_files: 0,
    oversize_text_files: 0,
    binary_files: 20,
    binary_bytes: 4 * 1024,
};

pub const LARGE_FIXTURE: FixtureSpec = FixtureSpec {
    name: "large-files",
    source_files: 48,
    source_bytes: 256 * 1024,
    test_files: 12,
    test_bytes: 512 * 1024,
    text_files: 24,
    text_bytes: 1024 * 1024,
    oversize_source_files: 6,
    oversize_text_files: 6,
    binary_files: 8,
    binary_bytes: 512 * 1024,
};

struct AllocationWindow;

impl AllocationWindow {
    fn start() -> Self {
        COUNT_ALLOCATIONS.store(false, Ordering::SeqCst);
        ALLOCATION_CALLS.store(0, Ordering::Relaxed);
        REALLOCATION_CALLS.store(0, Ordering::Relaxed);
        DEALLOCATION_CALLS.store(0, Ordering::Relaxed);
        REQUESTED_BYTES.store(0, Ordering::Relaxed);
        DEALLOCATED_BYTES.store(0, Ordering::Relaxed);
        COUNT_ALLOCATIONS.store(true, Ordering::SeqCst);
        Self
    }

    fn snapshot() -> AllocationTelemetry {
        AllocationTelemetry {
            allocation_calls: ALLOCATION_CALLS.load(Ordering::Relaxed),
            reallocation_calls: REALLOCATION_CALLS.load(Ordering::Relaxed),
            deallocation_calls: DEALLOCATION_CALLS.load(Ordering::Relaxed),
            requested_bytes: REQUESTED_BYTES.load(Ordering::Relaxed),
            deallocated_bytes: DEALLOCATED_BYTES.load(Ordering::Relaxed),
        }
    }
}

impl Drop for AllocationWindow {
    fn drop(&mut self) {
        COUNT_ALLOCATIONS.store(false, Ordering::SeqCst);
    }
}

/// Measures one complete scan while the counting allocator is enabled.
///
/// # Errors
///
/// Returns any filesystem error produced by the supplied scan.
pub fn measure_scan(
    scan: impl FnOnce() -> io::Result<(ScanOutput, IoTelemetry)>,
) -> io::Result<MeasuredScan> {
    let allocations = AllocationWindow::start();
    let started = Instant::now();
    let result = scan();
    let elapsed_nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let allocation_snapshot = AllocationWindow::snapshot();
    drop(allocations);
    let (output, io) = result?;
    Ok(MeasuredScan {
        elapsed_nanos,
        allocations: allocation_snapshot,
        io,
        output,
    })
}

#[derive(Default)]
struct ConsumerDigest {
    hasher: blake3::Hasher,
    documents: u64,
    logical_bytes: u64,
}

impl ConsumerDigest {
    fn consume(&mut self, relative_path: &str, bytes: &[u8]) {
        self.hasher
            .update(&(relative_path.len() as u64).to_le_bytes());
        self.hasher.update(relative_path.as_bytes());
        self.hasher.update(&(bytes.len() as u64).to_le_bytes());
        self.hasher.update(bytes);
        self.documents += 1;
        self.logical_bytes += bytes.len() as u64;
    }

    fn finish(self) -> ConsumerSummary {
        ConsumerSummary {
            documents: self.documents,
            logical_bytes: self.logical_bytes,
            digest_blake3: self.hasher.finalize().to_hex().to_string(),
        }
    }
}

/// Models the current two independent discovery and content-read passes.
///
/// # Errors
///
/// Returns filesystem traversal, metadata, or content-read errors that the
/// corresponding production path treats as fatal.
pub fn scan_separate(root: &Path) -> io::Result<(ScanOutput, IoTelemetry)> {
    let mut telemetry = IoTelemetry::default();
    let map_paths = discover_map_paths(root, &mut telemetry)?;
    let mut map = ConsumerDigest::default();
    for relative_path in map_paths {
        let full_path = root.join(&relative_path);
        telemetry.metadata_calls += 1;
        if fs::metadata(&full_path).is_err() {
            continue;
        }
        let Ok(bytes) = read_counted(&full_path, &mut telemetry) else {
            continue;
        };
        update_peak_content(&mut telemetry, bytes.len());
        if let Ok(content) = String::from_utf8(bytes) {
            map.consume(&relative_path, content.as_bytes());
        }
    }

    let regex_paths = discover_regex_paths(root, &mut telemetry);
    let mut regex = ConsumerDigest::default();
    for full_path in regex_paths {
        telemetry.metadata_calls += 1;
        let metadata = fs::metadata(&full_path)?;
        if metadata.len() > MAX_REGEX_FILE_BYTES as u64 {
            continue;
        }
        let bytes = read_counted(&full_path, &mut telemetry)?;
        update_peak_content(&mut telemetry, bytes.len());
        if bytes.is_empty() || bytes.contains(&0) {
            continue;
        }
        let relative_path = normalized_relative(root, &full_path)?;
        regex.consume(&relative_path, &bytes);
    }

    Ok((
        ScanOutput {
            map: map.finish(),
            regex: regex.finish(),
        },
        telemetry,
    ))
}

/// Models a union discovery pass whose raw buffer is borrowed by both consumers.
///
/// # Errors
///
/// Returns filesystem traversal, metadata, or content-read errors.
pub fn scan_shared(root: &Path) -> io::Result<(ScanOutput, IoTelemetry)> {
    let mut telemetry = IoTelemetry::default();
    let paths = discover_union_paths(root, &mut telemetry);
    let mut map = ConsumerDigest::default();
    let mut regex = ConsumerDigest::default();
    for full_path in paths {
        let relative_path = normalized_relative(root, &full_path)?;
        let map_candidate =
            is_source_path(&relative_path) && !is_generated_or_vendor_path(&relative_path);
        telemetry.metadata_calls += 1;
        let metadata = fs::metadata(&full_path)?;
        let regex_candidate = !is_regex_excluded_path(&relative_path)
            && metadata.len() <= MAX_REGEX_FILE_BYTES as u64;
        if !map_candidate && !regex_candidate {
            continue;
        }

        let bytes = read_counted(&full_path, &mut telemetry)?;
        update_peak_content(&mut telemetry, bytes.len());
        if map_candidate && std::str::from_utf8(&bytes).is_ok() {
            map.consume(&relative_path, &bytes);
        }
        if regex_candidate && !bytes.is_empty() && !bytes.contains(&0) {
            regex.consume(&relative_path, &bytes);
        }
    }

    Ok((
        ScanOutput {
            map: map.finish(),
            regex: regex.finish(),
        },
        telemetry,
    ))
}

fn update_peak_content(telemetry: &mut IoTelemetry, bytes: usize) {
    telemetry.peak_retained_content_files = telemetry.peak_retained_content_files.max(1);
    telemetry.peak_retained_content_bytes = telemetry.peak_retained_content_bytes.max(bytes as u64);
}

fn read_counted(path: &Path, telemetry: &mut IoTelemetry) -> io::Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    telemetry.successful_read_calls += 1;
    telemetry.bytes_read += bytes.len() as u64;
    Ok(bytes)
}

fn discover_map_paths(root: &Path, telemetry: &mut IoTelemetry) -> io::Result<Vec<String>> {
    telemetry.walk_passes += 1;
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .parents(true)
        .ignore(true)
        .git_ignore(true);
    let root_owned = root.to_path_buf();
    builder.filter_entry(move |entry| {
        if entry.depth() == 0 {
            return true;
        }
        let relative = entry
            .path()
            .strip_prefix(&root_owned)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        !is_generated_or_vendor_path(&relative)
    });

    let mut paths = Vec::new();
    for entry in builder.build() {
        telemetry.walked_entries += 1;
        let entry = entry.map_err(io::Error::other)?;
        if !entry.path().is_file() || !is_source_path(&entry.path().to_string_lossy()) {
            continue;
        }
        paths.push(normalized_relative(root, entry.path())?);
    }
    paths.sort();
    Ok(paths)
}

fn discover_regex_paths(root: &Path, telemetry: &mut IoTelemetry) -> Vec<PathBuf> {
    let paths = discover_broad_paths(root, telemetry, false);
    paths
        .into_iter()
        .filter(|path| {
            normalized_relative(root, path)
                .map(|relative| !is_regex_excluded_path(&relative))
                .unwrap_or(false)
        })
        .collect()
}

fn discover_union_paths(root: &Path, telemetry: &mut IoTelemetry) -> Vec<PathBuf> {
    discover_broad_paths(root, telemetry, true)
}

fn discover_broad_paths(
    root: &Path,
    telemetry: &mut IoTelemetry,
    prune_paths_excluded_by_both: bool,
) -> Vec<PathBuf> {
    telemetry.walk_passes += 1;
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true);
    if prune_paths_excluded_by_both {
        let root_owned = root.to_path_buf();
        builder.filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            let relative = entry
                .path()
                .strip_prefix(&root_owned)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            !(is_regex_excluded_path(&relative) && is_generated_or_vendor_path(&relative))
        });
    }
    let mut paths = Vec::new();
    for entry in builder.build() {
        telemetry.walked_entries += 1;
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.into_path();
        if path.is_dir() {
            continue;
        }
        paths.push(path);
    }
    paths.sort();
    paths
}

fn normalized_relative(root: &Path, path: &Path) -> io::Result<String> {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(io::Error::other)
}

fn is_regex_excluded_path(path: &str) -> bool {
    path.starts_with(".git/")
        || path.starts_with(".packet28/")
        || path.starts_with("target/")
        || path.starts_with("node_modules/")
}

fn is_source_path(path: &str) -> bool {
    [
        ".java", ".rs", ".py", ".tsx", ".ts", ".js", ".jsx", ".go", ".cpp", ".cc", ".cxx", ".hpp",
        ".hh", ".h", ".c",
    ]
    .iter()
    .any(|extension| path.ends_with(extension))
}

fn is_generated_or_vendor_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower
        .split('/')
        .any(|segment| segment == ".tmp" || segment == ".temp" || segment.starts_with(".tmp-"))
    {
        return true;
    }
    lower.starts_with(".git/")
        || lower.contains("/.git/")
        || lower.starts_with("target/")
        || lower.contains("/target/")
        || lower.starts_with("build/")
        || lower.contains("/build/")
        || lower.starts_with("dist/")
        || lower.contains("/dist/")
        || lower.starts_with("out/")
        || lower.contains("/out/")
        || lower.starts_with("coverage/")
        || lower.contains("/coverage/")
        || lower.starts_with("node_modules/")
        || lower.contains("/node_modules/")
        || lower.contains("/jacoco-resources/")
}

/// Creates a deterministic repository-shaped fixture beneath `root`.
///
/// # Errors
///
/// Returns filesystem errors from directory creation or fixture writes.
pub fn materialize_fixture(root: &Path, spec: FixtureSpec) -> io::Result<FixtureManifest> {
    fs::create_dir_all(root)?;
    let mut manifest = FixtureManifest {
        name: spec.name.to_string(),
        files_written: 0,
        bytes_written: 0,
        regular_source_files: spec.source_files as u64,
        test_source_files: spec.test_files as u64,
        regex_only_text_files: spec.text_files as u64,
        oversize_source_files: spec.oversize_source_files as u64,
        oversize_regex_only_files: spec.oversize_text_files as u64,
        binary_files: spec.binary_files as u64,
    };
    write_fixture_file(
        root,
        ".gitignore",
        b"target/\nnode_modules/\n.packet28/\n",
        &mut manifest,
    )?;

    for index in 0..spec.source_files {
        let path = format!("src/module_{index:04}.rs");
        let bytes = deterministic_text(spec.source_bytes, index as u64);
        write_fixture_file(root, &path, &bytes, &mut manifest)?;
    }
    for index in 0..spec.test_files {
        let path = format!("tests/case_{index:04}_test.rs");
        let bytes = deterministic_text(spec.test_bytes, 10_000 + index as u64);
        write_fixture_file(root, &path, &bytes, &mut manifest)?;
    }
    for index in 0..spec.text_files {
        let path = format!("docs/guide_{index:04}.md");
        let bytes = deterministic_text(spec.text_bytes, 20_000 + index as u64);
        write_fixture_file(root, &path, &bytes, &mut manifest)?;
    }
    for index in 0..spec.oversize_source_files {
        let path = format!("src/oversize_{index:04}.rs");
        let bytes = deterministic_text(MAX_REGEX_FILE_BYTES + 512 * 1024, 30_000 + index as u64);
        write_fixture_file(root, &path, &bytes, &mut manifest)?;
    }
    for index in 0..spec.oversize_text_files {
        let path = format!("docs/oversize_{index:04}.md");
        let bytes = deterministic_text(MAX_REGEX_FILE_BYTES + 512 * 1024, 40_000 + index as u64);
        write_fixture_file(root, &path, &bytes, &mut manifest)?;
    }
    for index in 0..spec.binary_files {
        let path = format!("assets/blob_{index:04}.bin");
        let bytes = deterministic_binary(spec.binary_bytes, 50_000 + index as u64);
        write_fixture_file(root, &path, &bytes, &mut manifest)?;
    }

    write_fixture_file(root, "src/empty.rs", b"", &mut manifest)?;
    write_fixture_file(
        root,
        "src/invalid_utf8.rs",
        &[0xff, 0xfe, b'r', b's'],
        &mut manifest,
    )?;
    write_fixture_file(
        root,
        "src/nul_but_utf8.rs",
        b"pub fn before() {}\0pub fn after() {}\n",
        &mut manifest,
    )?;
    write_fixture_file(
        root,
        "target/generated.rs",
        &deterministic_text(16 * 1024, 60_000),
        &mut manifest,
    )?;
    write_fixture_file(
        root,
        "node_modules/package/index.js",
        &deterministic_text(16 * 1024, 60_001),
        &mut manifest,
    )?;
    write_fixture_file(
        root,
        ".packet28/runtime/internal.rs",
        &deterministic_text(16 * 1024, 60_002),
        &mut manifest,
    )?;
    Ok(manifest)
}

fn write_fixture_file(
    root: &Path,
    relative_path: &str,
    bytes: &[u8],
    manifest: &mut FixtureManifest,
) -> io::Result<()> {
    let path = root.join(relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    manifest.files_written += 1;
    manifest.bytes_written += bytes.len() as u64;
    Ok(())
}

fn deterministic_text(size: usize, seed: u64) -> Vec<u8> {
    let line = format!(
        "pub fn generated_{seed}() -> u64 {{ {seed} }} // deterministic Packet28 fixture\n"
    );
    repeat_to_size(line.as_bytes(), size)
}

fn deterministic_binary(size: usize, seed: u64) -> Vec<u8> {
    let mut bytes = repeat_to_size(&seed.to_le_bytes(), size);
    if !bytes.is_empty() {
        let middle = bytes.len() / 2;
        bytes[middle] = 0;
    }
    bytes
}

fn repeat_to_size(pattern: &[u8], size: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(size);
    while bytes.len() < size {
        let remaining = size - bytes.len();
        bytes.extend_from_slice(&pattern[..pattern.len().min(remaining)]);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boundary_fixture(root: &Path) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("src/normal.rs"), b"pub fn normal() {}\n").unwrap();
        fs::write(root.join("src/empty.rs"), b"").unwrap();
        fs::write(root.join("src/nul.rs"), b"fn before() {}\0fn after() {}\n").unwrap();
        fs::write(root.join("src/invalid.rs"), [0xff, 0xfe, b'x']).unwrap();
        fs::write(
            root.join("src/oversize.rs"),
            deterministic_text(MAX_REGEX_FILE_BYTES + 1, 1),
        )
        .unwrap();
        fs::write(
            root.join("docs/oversize.md"),
            deterministic_text(MAX_REGEX_FILE_BYTES + 1, 2),
        )
        .unwrap();
        fs::write(root.join("docs/readme.md"), b"searchable text\n").unwrap();
    }

    #[test]
    fn shared_scan_preserves_consumer_outputs_and_reduces_io() {
        let directory = tempfile::tempdir().unwrap();
        materialize_fixture(
            directory.path(),
            FixtureSpec {
                source_files: 8,
                test_files: 2,
                text_files: 3,
                binary_files: 2,
                source_bytes: 1024,
                test_bytes: 1024,
                text_bytes: 2048,
                binary_bytes: 1024,
                oversize_source_files: 1,
                oversize_text_files: 1,
                name: "test",
            },
        )
        .unwrap();

        let (separate, separate_io) = scan_separate(directory.path()).unwrap();
        let (shared, shared_io) = scan_shared(directory.path()).unwrap();

        assert_eq!(shared, separate);
        assert_eq!(separate_io.walk_passes, 2);
        assert_eq!(shared_io.walk_passes, 1);
        assert!(shared_io.successful_read_calls < separate_io.successful_read_calls);
        assert!(shared_io.bytes_read < separate_io.bytes_read);
        assert_eq!(shared_io.peak_retained_content_files, 1);
    }

    #[test]
    fn shared_scan_matches_empty_binary_invalid_and_oversize_boundaries() {
        let directory = tempfile::tempdir().unwrap();
        boundary_fixture(directory.path());

        let (separate, separate_io) = scan_separate(directory.path()).unwrap();
        let (shared, shared_io) = scan_shared(directory.path()).unwrap();

        assert_eq!(shared, separate);
        assert_eq!(shared.map.documents, 4);
        assert_eq!(shared.regex.documents, 3);
        assert_eq!(shared_io.successful_read_calls, 6);
        assert_eq!(separate_io.successful_read_calls, 10);
    }

    #[test]
    fn generated_fixture_is_byte_for_byte_reproducible() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_manifest = materialize_fixture(first.path(), SMALL_FIXTURE).unwrap();
        let second_manifest = materialize_fixture(second.path(), SMALL_FIXTURE).unwrap();

        let (first_output, _) = scan_shared(first.path()).unwrap();
        let (second_output, _) = scan_shared(second.path()).unwrap();

        assert_eq!(first_manifest, second_manifest);
        assert_eq!(first_output, second_output);
    }
}
