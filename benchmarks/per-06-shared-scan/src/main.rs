use packet28_per06_shared_scan::{
    materialize_fixture, measure_scan, scan_separate, scan_shared, AllocationTelemetry,
    FixtureManifest, IoTelemetry, MeasuredScan, ScanOutput, LARGE_FIXTURE, SMALL_FIXTURE,
};
use serde::Serialize;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: &str = "packet28.per06.shared-scan.v1";

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    audit_item: &'static str,
    measured_at_unix_seconds: u64,
    profile: &'static str,
    command: Vec<String>,
    source: SourceMetadata,
    toolchain: ToolchainMetadata,
    host: HostMetadata,
    method: MethodMetadata,
    fixtures: Vec<FixtureReport>,
}

#[derive(Serialize)]
struct SourceMetadata {
    git_head: Option<String>,
    harness_blake3: String,
}

#[derive(Serialize)]
struct ToolchainMetadata {
    rustc: Option<String>,
    cargo: Option<String>,
    target: String,
}

#[derive(Serialize)]
struct HostMetadata {
    os: String,
    arch: String,
    uname: Option<String>,
    cpu: Option<String>,
    memory_bytes: Option<u64>,
}

#[derive(Serialize)]
struct MethodMetadata {
    iterations: usize,
    fixture_roots: &'static str,
    cache_state: &'static str,
    order_control: &'static str,
    consumer_work: &'static str,
    allocation_scope: &'static str,
    io_scope: &'static str,
    content_residency_bound: &'static str,
    evidence_boundary: Vec<&'static str>,
}

#[derive(Serialize)]
struct FixtureReport {
    fixture: FixtureManifest,
    output_parity_asserted: bool,
    output: ScanOutput,
    separate: StrategyReport,
    shared: StrategyReport,
    delta: DeltaReport,
}

#[derive(Serialize)]
struct StrategyReport {
    operation: &'static str,
    exact_io_per_iteration: IoTelemetry,
    exact_io_stable_across_iterations: bool,
    median_allocations: AllocationTelemetry,
    median_elapsed_nanos: u64,
    raw_iterations: Vec<MeasuredScan>,
}

#[derive(Serialize)]
struct DeltaReport {
    walk_passes: i128,
    successful_read_calls: i128,
    bytes_read: i128,
    allocation_calls_basis_points: i128,
    requested_bytes_basis_points: i128,
    median_elapsed_basis_points: i128,
}

fn main() -> io::Result<()> {
    let arguments = env::args().collect::<Vec<_>>();
    let (iterations, output) = parse_args(&arguments)?;
    let temporary = tempfile::tempdir()?;
    let fixtures = [SMALL_FIXTURE, LARGE_FIXTURE]
        .into_iter()
        .map(|spec| measure_fixture(temporary.path(), spec, iterations))
        .collect::<io::Result<Vec<_>>>()?;

    let report = Report {
        schema: SCHEMA,
        audit_item: "PER-06",
        measured_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        profile: if cfg!(debug_assertions) {
            "debug (invalid for decision)"
        } else {
            "release"
        },
        command: arguments,
        source: SourceMetadata {
            git_head: command_output("git", &["rev-parse", "HEAD"]),
            harness_blake3: harness_digest(),
        },
        toolchain: ToolchainMetadata {
            rustc: command_output("rustc", &["-Vv"]),
            cargo: command_output("cargo", &["-V"]),
            target: format!("{}-{}", env::consts::ARCH, env::consts::OS),
        },
        host: HostMetadata {
            os: env::consts::OS.to_string(),
            arch: env::consts::ARCH.to_string(),
            uname: command_output("uname", &["-a"]),
            cpu: command_output("sysctl", &["-n", "machdep.cpu.brand_string"]),
            memory_bytes: command_output("sysctl", &["-n", "hw.memsize"])
                .and_then(|value| value.parse().ok()),
        },
        method: MethodMetadata {
            iterations,
            fixture_roots:
                "separate and shared strategies use independently materialized, byte-identical roots",
            cache_state:
                "steady-state warm page cache after one unmeasured warmup per strategy and fixture",
            order_control: "strategy execution order alternates every iteration",
            consumer_work:
                "both strategies perform identical per-consumer BLAKE3 work; only discovery/read ownership differs",
            allocation_scope:
                "all successful Rust global allocator calls made during each complete scan operation",
            io_scope:
                "exact application-level walk passes, yielded entries, explicit metadata calls, successful fs::read calls, and returned bytes",
            content_residency_bound:
                "the shared prototype retains at most one raw file buffer and drops it after both consumers",
            evidence_boundary: vec![
                "timings are machine-local steady-state observations, not cold-I/O or end-to-end index-build claims",
                "read and byte reductions are explicit harness operations and do not rely on page-cache timing",
                "consumer digests prove equivalent fixture inputs, not equivalence of private production parser/index encodings",
                "production integration requires feature-gated parity and end-to-end validation in both owning crates",
            ],
        },
        fixtures,
    };

    let payload = serde_json::to_vec_pretty(&report).map_err(io::Error::other)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, payload)?;
    println!("{}", output.display());
    Ok(())
}

fn measure_fixture(
    temporary_root: &Path,
    spec: packet28_per06_shared_scan::FixtureSpec,
    iterations: usize,
) -> io::Result<FixtureReport> {
    let fixture_root = temporary_root.join(spec.name);
    let separate_root = fixture_root.join("separate");
    let shared_root = fixture_root.join("shared");
    let separate_manifest = materialize_fixture(&separate_root, spec)?;
    let shared_manifest = materialize_fixture(&shared_root, spec)?;
    if separate_manifest != shared_manifest {
        return Err(io::Error::other("fixture materialization drifted"));
    }

    let (warm_separate, _) = scan_separate(&separate_root)?;
    let (warm_shared, _) = scan_shared(&shared_root)?;
    if warm_separate != warm_shared {
        return Err(io::Error::other(
            "shared prototype changed consumer-visible output during warmup",
        ));
    }

    let mut separate_runs = Vec::with_capacity(iterations);
    let mut shared_runs = Vec::with_capacity(iterations);
    for iteration in 0..iterations {
        let (separate, shared) = if iteration % 2 == 0 {
            (
                measure_scan(|| scan_separate(&separate_root))?,
                measure_scan(|| scan_shared(&shared_root))?,
            )
        } else {
            let shared = measure_scan(|| scan_shared(&shared_root))?;
            let separate = measure_scan(|| scan_separate(&separate_root))?;
            (separate, shared)
        };
        if separate.output != shared.output {
            return Err(io::Error::other(format!(
                "consumer output drifted in {} iteration {}",
                spec.name,
                iteration + 1
            )));
        }
        separate_runs.push(separate);
        shared_runs.push(shared);
    }
    summarize_fixture(separate_manifest, warm_shared, separate_runs, shared_runs)
}

fn parse_args(arguments: &[String]) -> io::Result<(usize, PathBuf)> {
    let mut iterations = 9;
    let mut output = PathBuf::from("result.local.json");
    let mut cursor = 1;
    while cursor < arguments.len() {
        match arguments[cursor].as_str() {
            "--iterations" => {
                cursor += 1;
                let value = arguments.get(cursor).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "--iterations needs a value")
                })?;
                iterations = value.parse().map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("invalid --iterations value: {error}"),
                    )
                })?;
            }
            "--output" => {
                cursor += 1;
                output = arguments.get(cursor).map(PathBuf::from).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "--output needs a path")
                })?;
            }
            unknown => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown argument: {unknown}"),
                ));
            }
        }
        cursor += 1;
    }
    if iterations < 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--iterations must be at least 3",
        ));
    }
    Ok((iterations, output))
}

fn summarize_fixture(
    fixture: FixtureManifest,
    output: ScanOutput,
    separate_runs: Vec<MeasuredScan>,
    shared_runs: Vec<MeasuredScan>,
) -> io::Result<FixtureReport> {
    let separate = summarize_strategy("two independent discovery/read passes", separate_runs)?;
    let shared = summarize_strategy(
        "one union discovery pass with one-file shared content residency",
        shared_runs,
    )?;
    let delta = DeltaReport {
        walk_passes: signed_delta(
            separate.exact_io_per_iteration.walk_passes,
            shared.exact_io_per_iteration.walk_passes,
        ),
        successful_read_calls: signed_delta(
            separate.exact_io_per_iteration.successful_read_calls,
            shared.exact_io_per_iteration.successful_read_calls,
        ),
        bytes_read: signed_delta(
            separate.exact_io_per_iteration.bytes_read,
            shared.exact_io_per_iteration.bytes_read,
        ),
        allocation_calls_basis_points: basis_point_delta(
            separate.median_allocations.allocation_calls,
            shared.median_allocations.allocation_calls,
        ),
        requested_bytes_basis_points: basis_point_delta(
            separate.median_allocations.requested_bytes,
            shared.median_allocations.requested_bytes,
        ),
        median_elapsed_basis_points: basis_point_delta(
            separate.median_elapsed_nanos,
            shared.median_elapsed_nanos,
        ),
    };
    Ok(FixtureReport {
        fixture,
        output_parity_asserted: true,
        output,
        separate,
        shared,
        delta,
    })
}

fn summarize_strategy(
    operation: &'static str,
    runs: Vec<MeasuredScan>,
) -> io::Result<StrategyReport> {
    let first = runs
        .first()
        .ok_or_else(|| io::Error::other("strategy has no measured runs"))?;
    let exact_io_per_iteration = first.io;
    let exact_io_stable_across_iterations = runs.iter().all(|run| run.io == exact_io_per_iteration);
    if !exact_io_stable_across_iterations {
        return Err(io::Error::other(
            "application-level I/O telemetry changed across identical iterations",
        ));
    }
    let median_allocations = AllocationTelemetry {
        allocation_calls: median(runs.iter().map(|run| run.allocations.allocation_calls)),
        reallocation_calls: median(runs.iter().map(|run| run.allocations.reallocation_calls)),
        deallocation_calls: median(runs.iter().map(|run| run.allocations.deallocation_calls)),
        requested_bytes: median(runs.iter().map(|run| run.allocations.requested_bytes)),
        deallocated_bytes: median(runs.iter().map(|run| run.allocations.deallocated_bytes)),
    };
    let median_elapsed_nanos = median(runs.iter().map(|run| run.elapsed_nanos));
    Ok(StrategyReport {
        operation,
        exact_io_per_iteration,
        exact_io_stable_across_iterations,
        median_allocations,
        median_elapsed_nanos,
        raw_iterations: runs,
    })
}

fn median(values: impl Iterator<Item = u64>) -> u64 {
    let mut values = values.collect::<Vec<_>>();
    values.sort_unstable();
    values[values.len() / 2]
}

fn signed_delta(before: u64, after: u64) -> i128 {
    i128::from(after) - i128::from(before)
}

fn basis_point_delta(before: u64, after: u64) -> i128 {
    if before == 0 {
        0
    } else {
        signed_delta(before, after) * 10_000 / i128::from(before)
    }
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn harness_digest() -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(include_bytes!("lib.rs"));
    hasher.update(include_bytes!("main.rs"));
    hasher.update(include_bytes!("../Cargo.toml"));
    hasher.finalize().to_hex().to_string()
}

#[allow(dead_code)]
fn _assert_output_path_is_relative_to_harness(_path: &Path) {}
