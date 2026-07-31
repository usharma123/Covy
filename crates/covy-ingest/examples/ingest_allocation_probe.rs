use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use covy_core::diagnostics::{DiagnosticsData, DiagnosticsFormat};
use covy_core::model::{CoverageData, CoverageFormat};
use covy_core::CovyError;

const LCOV_RECORDS: usize = 50_000;
const DIAGNOSTICS_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_ITERATIONS: usize = 10;

static TRACK_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static REALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static REQUESTED_BYTES: AtomicU64 = AtomicU64::new(0);

struct CountingAllocator;

// SAFETY: Every operation delegates to `System` with the original pointer and
// layout. The atomics only observe successful allocation requests.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Delegating the allocation request unchanged satisfies
        // `GlobalAlloc::alloc`'s contract.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && TRACK_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
            REQUESTED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Delegating the allocation request unchanged satisfies
        // `GlobalAlloc::alloc_zeroed`'s contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() && TRACK_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
            REQUESTED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` and `layout` come from the corresponding `System`
        // allocation operation and are forwarded unchanged.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: Delegating the reallocation request unchanged satisfies
        // `GlobalAlloc::realloc`'s contract.
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() && TRACK_ALLOCATIONS.load(Ordering::Relaxed) {
            REALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
            REQUESTED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        }
        new_pointer
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Debug)]
struct Measurement {
    name: &'static str,
    input_bytes: usize,
    logical_records: usize,
    iterations: usize,
    allocation_calls: u64,
    reallocation_calls: u64,
    requested_bytes: u64,
    elapsed: Duration,
}

struct TemporaryFile {
    path: PathBuf,
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let iterations = parse_iterations()?;
    let lcov = scaled_lcov(LCOV_RECORDS);
    let diagnostics = scaled_sarif(DIAGNOSTICS_PAYLOAD_BYTES);
    let diagnostics_file = write_temporary_sarif(&diagnostics)?;

    warm_up(&lcov, &diagnostics_file.path)?;

    let lcov_measurement = measure("lcov_parse", lcov.len(), LCOV_RECORDS, iterations, || {
        parse_lcov(&lcov)
    })?;
    let diagnostics_path_measurement =
        measure("diagnostics_path", diagnostics.len(), 0, iterations, || {
            covy_ingest::ingest_diagnostics_path(&diagnostics_file.path)
        })?;
    let diagnostics_reference_measurement = measure(
        "diagnostics_single_buffer_reference",
        diagnostics.len(),
        0,
        iterations,
        || ingest_diagnostics_single_buffer(&diagnostics_file.path),
    )?;

    print_measurements(&[
        lcov_measurement,
        diagnostics_path_measurement,
        diagnostics_reference_measurement,
    ]);
    Ok(())
}

fn parse_iterations() -> Result<usize, Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let Some(argument) = arguments.next() else {
        return Ok(DEFAULT_ITERATIONS);
    };
    if argument != "--iterations" {
        return Err(format!("unexpected argument '{argument}'; expected --iterations N").into());
    }
    let iterations = arguments
        .next()
        .ok_or("missing value after --iterations")?
        .parse::<usize>()?;
    if iterations == 0 {
        return Err("iterations must be greater than zero".into());
    }
    if let Some(extra) = arguments.next() {
        return Err(format!("unexpected trailing argument '{extra}'").into());
    }
    Ok(iterations)
}

fn scaled_lcov(records: usize) -> Vec<u8> {
    let mut content = String::with_capacity(records.saturating_mul(28));
    content.push_str("TN:allocation-benchmark\nSF:src/generated.rs\n");
    for line in 1..=records {
        content.push_str(&format!("DA:{line},{},checksum\n", line % 2));
    }
    content.push_str("end_of_record\n");
    content.into_bytes()
}

fn scaled_sarif(payload_bytes: usize) -> Vec<u8> {
    let mut content = String::with_capacity(payload_bytes.saturating_add(128));
    content.push_str(r#"{"version":"2.1.0","runs":[],"benchmarkPadding":""#);
    content.extend(std::iter::repeat_n('a', payload_bytes));
    content.push_str("\"}");
    content.into_bytes()
}

fn write_temporary_sarif(content: &[u8]) -> Result<TemporaryFile, std::io::Error> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "covy-ingest-allocation-probe-{}-{nonce}.sarif",
        std::process::id()
    ));
    std::fs::write(&path, content)?;
    Ok(TemporaryFile { path })
}

fn warm_up(lcov: &[u8], diagnostics_path: &Path) -> Result<(), CovyError> {
    black_box(parse_lcov(lcov)?);
    black_box(covy_ingest::ingest_diagnostics_path(diagnostics_path)?);
    black_box(ingest_diagnostics_single_buffer(diagnostics_path)?);
    Ok(())
}

fn parse_lcov(content: &[u8]) -> Result<CoverageData, CovyError> {
    covy_ingest::get_ingestor(CoverageFormat::Lcov).parse(content)
}

fn ingest_diagnostics_single_buffer(path: &Path) -> Result<DiagnosticsData, CovyError> {
    let content = std::fs::read(path)?;
    let format = covy_ingest::detect_diagnostics_format(path, &content)?;
    match format {
        DiagnosticsFormat::Sarif => covy_ingest::sarif::parse_sarif(&content),
    }
}

fn measure<T>(
    name: &'static str,
    input_bytes: usize,
    logical_records: usize,
    iterations: usize,
    mut operation: impl FnMut() -> Result<T, CovyError>,
) -> Result<Measurement, CovyError> {
    ALLOCATION_CALLS.store(0, Ordering::Relaxed);
    REALLOCATION_CALLS.store(0, Ordering::Relaxed);
    REQUESTED_BYTES.store(0, Ordering::Relaxed);
    TRACK_ALLOCATIONS.store(true, Ordering::SeqCst);
    let started = Instant::now();
    for _ in 0..iterations {
        black_box(operation()?);
    }
    let elapsed = started.elapsed();
    TRACK_ALLOCATIONS.store(false, Ordering::SeqCst);

    Ok(Measurement {
        name,
        input_bytes,
        logical_records,
        iterations,
        allocation_calls: ALLOCATION_CALLS.load(Ordering::Relaxed),
        reallocation_calls: REALLOCATION_CALLS.load(Ordering::Relaxed),
        requested_bytes: REQUESTED_BYTES.load(Ordering::Relaxed),
        elapsed,
    })
}

fn print_measurements(measurements: &[Measurement]) {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!("{{\"schema_version\":1,\"profile\":\"{profile}\",\"workloads\":[");
    for (index, measurement) in measurements.iter().enumerate() {
        if index > 0 {
            println!(",");
        }
        let elapsed_ns = measurement.elapsed.as_nanos();
        println!(
            concat!(
                "{{\"name\":\"{}\",\"input_bytes\":{},\"logical_records\":{},",
                "\"iterations\":{},\"allocation_calls\":{},\"reallocation_calls\":{},",
                "\"requested_bytes\":{},\"elapsed_ns\":{}}}"
            ),
            measurement.name,
            measurement.input_bytes,
            measurement.logical_records,
            measurement.iterations,
            measurement.allocation_calls,
            measurement.reallocation_calls,
            measurement.requested_bytes,
            elapsed_ns
        );
    }
    println!("]}}");
}
