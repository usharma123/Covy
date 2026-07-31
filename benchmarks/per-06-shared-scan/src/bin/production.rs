use anyhow::{anyhow, Context, Result};
use ignore::WalkBuilder;
use mapy_core::shared_scan::RepoIndexScanSession;
use mapy_core::RepoIndexRuntime;
use packet28_search_core::shared_scan::RegexIndexScanSession;
use packet28_search_core::RegexIndexRuntime;
use packet28d::shared_repository_scan::{
    rebuild_full_indexes_with_shared_scan, SharedIndexRuntimes, SharedScanTelemetry,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use suite_packet_core::search::SearchRequest;

const SCHEMA: &str = "packet28.per06.production-shared-scan.v1";
const FIXED_MTIME_UNIX: u64 = 1_700_000_000;
const MIN_ITERATIONS: usize = 6;

struct Arguments {
    iterations: usize,
    output: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct FixtureSpec {
    name: &'static str,
    source_files: usize,
    source_bytes: usize,
    test_files: usize,
    test_bytes: usize,
    text_files: usize,
    text_bytes: usize,
    oversize_source_files: usize,
    oversize_text_files: usize,
    exact_regex_boundary: bool,
}

const SMALL: FixtureSpec = FixtureSpec {
    name: "small-files",
    source_files: 240,
    source_bytes: 4 * 1024,
    test_files: 80,
    test_bytes: 6 * 1024,
    text_files: 80,
    text_bytes: 8 * 1024,
    oversize_source_files: 0,
    oversize_text_files: 0,
    exact_regex_boundary: false,
};

const LARGE: FixtureSpec = FixtureSpec {
    name: "large-files",
    source_files: 36,
    source_bytes: 256 * 1024,
    test_files: 12,
    test_bytes: 512 * 1024,
    text_files: 24,
    text_bytes: 1024 * 1024,
    oversize_source_files: 4,
    oversize_text_files: 4,
    exact_regex_boundary: true,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
struct IoEvidence {
    walk_passes: u64,
    walked_entries: u64,
    ignored_walk_errors: u64,
    classification_metadata_queries: u64,
    content_metadata_calls: u64,
    successful_read_calls: u64,
    bytes_read: u64,
    peak_retained_content_files: u64,
    peak_retained_content_bytes: u64,
}

impl IoEvidence {
    fn merge(&mut self, other: Self) {
        self.walk_passes = self.walk_passes.saturating_add(other.walk_passes);
        self.walked_entries = self.walked_entries.saturating_add(other.walked_entries);
        self.ignored_walk_errors = self
            .ignored_walk_errors
            .saturating_add(other.ignored_walk_errors);
        self.classification_metadata_queries = self
            .classification_metadata_queries
            .saturating_add(other.classification_metadata_queries);
        self.content_metadata_calls = self
            .content_metadata_calls
            .saturating_add(other.content_metadata_calls);
        self.successful_read_calls = self
            .successful_read_calls
            .saturating_add(other.successful_read_calls);
        self.bytes_read = self.bytes_read.saturating_add(other.bytes_read);
        self.peak_retained_content_files = self
            .peak_retained_content_files
            .max(other.peak_retained_content_files);
        self.peak_retained_content_bytes = self
            .peak_retained_content_bytes
            .max(other.peak_retained_content_bytes);
    }

    fn observe_read(&mut self, bytes: &[u8]) {
        self.successful_read_calls = self.successful_read_calls.saturating_add(1);
        self.bytes_read = self
            .bytes_read
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.peak_retained_content_files = 1;
        self.peak_retained_content_bytes = self
            .peak_retained_content_bytes
            .max(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    }
}

impl From<SharedScanTelemetry> for IoEvidence {
    fn from(value: SharedScanTelemetry) -> Self {
        Self {
            walk_passes: value.walk_passes,
            walked_entries: value.walked_entries,
            ignored_walk_errors: value.ignored_walk_errors,
            classification_metadata_queries: value.classification_metadata_queries,
            content_metadata_calls: value.content_metadata_calls,
            successful_read_calls: value.successful_read_calls,
            bytes_read: value.bytes_read,
            peak_retained_content_files: value.peak_retained_content_files,
            peak_retained_content_bytes: value.peak_retained_content_bytes,
        }
    }
}

struct MeasuredIndexes {
    elapsed_nanos: u64,
    io: IoEvidence,
    repo: RepoIndexRuntime,
    regex: RegexIndexRuntime,
}

#[derive(Debug, Clone, Copy)]
struct MeasurementSample {
    elapsed_nanos: u64,
    input_io: IoEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct OutputEvidence {
    map_snapshot: String,
    regex: RegexOutputEvidence,
    query_results: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RegexOutputEvidence {
    lookup: String,
    postings: String,
    documents: String,
}

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    audit_item: &'static str,
    measured_at_unix_seconds: u64,
    profile: &'static str,
    command: Vec<String>,
    source: SourceEvidence,
    toolchain: ToolchainEvidence,
    host: HostEvidence,
    method: MethodEvidence,
    decision: DecisionEvidence,
    fixtures: Vec<FixtureReport>,
}

#[derive(Serialize)]
struct SourceEvidence {
    git_head: String,
    worktree_clean: bool,
    status_porcelain: Vec<String>,
    files_blake3: BTreeMap<&'static str, String>,
}

#[derive(Serialize)]
struct ToolchainEvidence {
    rustc: Option<String>,
    cargo: Option<String>,
    target: String,
}

#[derive(Serialize)]
struct HostEvidence {
    uname: Option<String>,
    cpu_model: Option<String>,
    memory_bytes: Option<u64>,
    available_parallelism: Option<usize>,
}

#[derive(Serialize)]
struct MethodEvidence {
    iterations: usize,
    warmup: &'static str,
    separate_baseline: &'static str,
    fixture_roots: &'static str,
    cache_state: &'static str,
    order_control: &'static str,
    timed_scope: &'static str,
    io_scope: &'static str,
    parity: &'static str,
    content_residency_bound: &'static str,
}

#[derive(Serialize)]
struct DecisionEvidence {
    state: &'static str,
    reason: Vec<&'static str>,
}

#[derive(Serialize)]
struct FixtureReport {
    fixture: FixtureEvidence,
    direct_standard_parity_asserted_before_measurement: bool,
    semantic_oracles_asserted: bool,
    parity_asserted_every_iteration: bool,
    output: OutputEvidence,
    separate: StrategyReport,
    shared: StrategyReport,
    input_io_delta: InputIoDeltaEvidence,
    paired_timing: PairedTimingEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct FixtureEvidence {
    name: String,
    filesystem_entries_written: u64,
    bytes_written: u64,
}

#[derive(Serialize)]
struct StrategyReport {
    operation: &'static str,
    repository_input_io_per_iteration: IoEvidence,
    repository_input_io_stable_across_iterations: bool,
    median_elapsed_nanos: u64,
    raw_elapsed_nanos: Vec<u64>,
}

#[derive(Serialize)]
struct InputIoDeltaEvidence {
    walk_passes: i128,
    classification_metadata_queries: i128,
    content_metadata_calls: i128,
    successful_read_calls: i128,
    bytes_read: i128,
    peak_retained_content_bytes: i128,
}

#[derive(Serialize)]
struct PairedTimingEvidence {
    order: Vec<&'static str>,
    elapsed_basis_points: Vec<i128>,
    median_elapsed_basis_points: i128,
}

fn main() -> Result<()> {
    let command = env::args().collect::<Vec<_>>();
    let arguments = parse_args(&command)?;
    if cfg!(debug_assertions) {
        return Err(anyhow!(
            "production evidence must run with a release profile"
        ));
    }
    let temporary = tempfile::tempdir()?;
    let fixtures = [SMALL, LARGE]
        .into_iter()
        .map(|spec| measure_fixture(temporary.path(), spec, arguments.iterations))
        .collect::<Result<Vec<_>>>()?;
    let report = build_report(command, arguments.iterations, fixtures)?;
    let payload = serde_json::to_vec_pretty(&report)?;
    if let Some(parent) = arguments.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&arguments.output, payload)
        .with_context(|| format!("failed to write '{}'", arguments.output.display()))?;
    println!("{}", arguments.output.display());
    Ok(())
}

fn build_report(
    command: Vec<String>,
    iterations: usize,
    fixtures: Vec<FixtureReport>,
) -> Result<Report> {
    Ok(Report {
        schema: SCHEMA,
        audit_item: "PER-06",
        measured_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        profile: "release",
        command,
        source: collect_source_evidence()?,
        toolchain: ToolchainEvidence {
            rustc: command_output("rustc", &["-Vv"]),
            cargo: command_output("cargo", &["-V"]),
            target: format!("{}-{}", env::consts::ARCH, env::consts::OS),
        },
        host: collect_host_evidence(),
        method: MethodEvidence {
            iterations,
            warmup:
                "one unmeasured direct-standard, instrumented-separate, and shared build per fixture",
            separate_baseline:
                "instrumented counterfactual: the two standalone discovery policies and feature-gated builder/publication sessions; an untimed direct-standard build proves output parity before measurement",
            fixture_roots:
                "each strategy receives an independently materialized byte- and mtime-identical root",
            cache_state:
                "new roots are materialized immediately before each timed build; recently written data may be in the host page cache",
            order_control:
                "balanced AB/BA pairs alternate separate-first and shared-first across an even iteration count",
            timed_scope:
                "discovery, repository-content input, parsing/gram construction, immutable artifact preparation, each strategy's publication mechanics, and commit",
            io_scope:
                "exact application-level repository-input telemetry only: walker entries, classification/content metadata queries, successful content fs::read calls, returned bytes, and largest retained raw content buffer; excludes cache/manifest/artifact I/O and physical kernel I/O",
            parity:
                "direct-standard versus instrumented-separate parity is asserted before timing; map snapshots, regex lookup/postings/document digests, semantic policy oracles, and indexed query results are then asserted for every measured pair",
            content_residency_bound:
                "both strategies retain at most one raw content buffer; the shared strategy lends it to both builders; runtimes and fixture roots are dropped after each pair",
        },
        decision: DecisionEvidence {
            state: "keep_non_default_feature_gate",
            reason: vec![
                "production parity is demonstrated on controlled fixtures, not every filesystem and platform",
                "the measured release results are machine-local and recently-written-cache observations",
                "default enablement should follow Linux parity and field telemetry from opt-in daemon runs",
            ],
        },
        fixtures,
    })
}

fn collect_source_evidence() -> Result<SourceEvidence> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository_root = manifest_dir.join("../..");
    let source_files = [
        (
            "benchmarks/per-06-shared-scan/Cargo.lock",
            manifest_dir.join("Cargo.lock"),
        ),
        (
            "benchmarks/per-06-shared-scan/Cargo.toml",
            manifest_dir.join("Cargo.toml"),
        ),
        (
            "benchmarks/per-06-shared-scan/src/bin/production.rs",
            manifest_dir.join("src/bin/production.rs"),
        ),
        (
            "crates/mapy-core/Cargo.toml",
            manifest_dir.join("../../crates/mapy-core/Cargo.toml"),
        ),
        (
            "crates/mapy-core/src/generation.rs",
            manifest_dir.join("../../crates/mapy-core/src/generation.rs"),
        ),
        (
            "crates/mapy-core/src/scan.rs",
            manifest_dir.join("../../crates/mapy-core/src/scan.rs"),
        ),
        (
            "crates/mapy-core/src/shared_scan.rs",
            manifest_dir.join("../../crates/mapy-core/src/shared_scan.rs"),
        ),
        (
            "crates/packet28-search-core/Cargo.toml",
            manifest_dir.join("../../crates/packet28-search-core/Cargo.toml"),
        ),
        (
            "crates/packet28-search-core/src/lib.rs",
            manifest_dir.join("../../crates/packet28-search-core/src/lib.rs"),
        ),
        (
            "crates/packet28-search-core/src/shared_scan.rs",
            manifest_dir.join("../../crates/packet28-search-core/src/shared_scan.rs"),
        ),
        (
            "crates/packet28d/Cargo.toml",
            manifest_dir.join("../../crates/packet28d/Cargo.toml"),
        ),
        (
            "crates/packet28d/src/shared_repository_scan.rs",
            manifest_dir.join("../../crates/packet28d/src/shared_repository_scan.rs"),
        ),
    ];
    let files_blake3 = source_files
        .into_iter()
        .map(|(label, path)| Ok((label, file_digest(&path)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let status = command_output_at(
        &repository_root,
        "git",
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .context("benchmark source status could not be inspected")?;
    Ok(SourceEvidence {
        git_head: command_output_at(&repository_root, "git", &["rev-parse", "HEAD"])
            .context("benchmark source must be inside a Git checkout")?,
        worktree_clean: status.is_empty(),
        status_porcelain: status.lines().map(ToOwned::to_owned).collect(),
        files_blake3,
    })
}

fn measure_fixture(
    temporary: &Path,
    spec: FixtureSpec,
    iterations: usize,
) -> Result<FixtureReport> {
    let (expected_fixture, expected_output) = warmup_and_verify(temporary, spec)?;
    let mut separate_runs = Vec::with_capacity(iterations);
    let mut shared_runs = Vec::with_capacity(iterations);
    let mut order = Vec::with_capacity(iterations);
    for iteration in 0..iterations {
        let iteration_root = temporary
            .join(spec.name)
            .join(format!("measured-{iteration:02}"));
        let separate_root = iteration_root.join("separate");
        let shared_root = iteration_root.join("shared");
        let separate_fixture = materialize_fixture(&separate_root, spec)?;
        let shared_fixture = materialize_fixture(&shared_root, spec)?;
        if separate_fixture != expected_fixture || shared_fixture != expected_fixture {
            return Err(anyhow!("fixture materialization drifted for {}", spec.name));
        }
        let (separate, shared) = if iteration % 2 == 0 {
            order.push("separate_then_shared");
            (
                measure_separate(&separate_root)?,
                measure_shared(&shared_root)?,
            )
        } else {
            order.push("shared_then_separate");
            let shared = measure_shared(&shared_root)?;
            let separate = measure_separate(&separate_root)?;
            (separate, shared)
        };
        assert_semantic_oracles(&separate_root, &separate, spec)?;
        assert_semantic_oracles(&shared_root, &shared, spec)?;
        let output = assert_parity(&separate_root, &separate, &shared_root, &shared)?;
        if output != expected_output {
            return Err(anyhow!("output evidence changed between iterations"));
        }
        separate_runs.push(separate.sample());
        shared_runs.push(shared.sample());
        drop(separate);
        drop(shared);
        fs::remove_dir_all(&iteration_root).with_context(|| {
            format!(
                "failed to remove measured fixture root '{}'",
                iteration_root.display()
            )
        })?;
    }
    let separate_io = stable_io(&separate_runs, spec.name, "separate")?;
    let shared_io = stable_io(&shared_runs, spec.name, "shared")?;
    let separate_elapsed = separate_runs
        .iter()
        .map(|run| run.elapsed_nanos)
        .collect::<Vec<_>>();
    let shared_elapsed = shared_runs
        .iter()
        .map(|run| run.elapsed_nanos)
        .collect::<Vec<_>>();
    let paired_deltas = separate_elapsed
        .iter()
        .zip(&shared_elapsed)
        .map(|(separate, shared)| basis_point_delta(*shared, *separate))
        .collect::<Vec<_>>();
    Ok(FixtureReport {
        fixture: expected_fixture,
        direct_standard_parity_asserted_before_measurement: true,
        semantic_oracles_asserted: true,
        parity_asserted_every_iteration: true,
        output: expected_output,
        separate: StrategyReport {
            operation:
                "instrumented two-pass counterfactual preserving standalone discovery policies and engine builders/publication",
            repository_input_io_per_iteration: separate_io,
            repository_input_io_stable_across_iterations: true,
            median_elapsed_nanos: median_u64(&separate_elapsed),
            raw_elapsed_nanos: separate_elapsed,
        },
        shared: StrategyReport {
            operation: "production shared coordinator with one union discovery/read pass",
            repository_input_io_per_iteration: shared_io,
            repository_input_io_stable_across_iterations: true,
            median_elapsed_nanos: median_u64(&shared_elapsed),
            raw_elapsed_nanos: shared_elapsed,
        },
        input_io_delta: InputIoDeltaEvidence {
            walk_passes: i128::from(shared_io.walk_passes) - i128::from(separate_io.walk_passes),
            classification_metadata_queries: i128::from(shared_io.classification_metadata_queries)
                - i128::from(separate_io.classification_metadata_queries),
            content_metadata_calls: i128::from(shared_io.content_metadata_calls)
                - i128::from(separate_io.content_metadata_calls),
            successful_read_calls: i128::from(shared_io.successful_read_calls)
                - i128::from(separate_io.successful_read_calls),
            bytes_read: i128::from(shared_io.bytes_read) - i128::from(separate_io.bytes_read),
            peak_retained_content_bytes: i128::from(shared_io.peak_retained_content_bytes)
                - i128::from(separate_io.peak_retained_content_bytes),
        },
        paired_timing: PairedTimingEvidence {
            order,
            median_elapsed_basis_points: median_i128(&paired_deltas),
            elapsed_basis_points: paired_deltas,
        },
    })
}

fn warmup_and_verify(
    temporary: &Path,
    spec: FixtureSpec,
) -> Result<(FixtureEvidence, OutputEvidence)> {
    let warmup_root = temporary.join(spec.name).join("warmup");
    let direct_root = warmup_root.join("direct-standard");
    let separate_root = warmup_root.join("instrumented-separate");
    let shared_root = warmup_root.join("shared");
    let direct_fixture = materialize_fixture(&direct_root, spec)?;
    let separate_fixture = materialize_fixture(&separate_root, spec)?;
    let shared_fixture = materialize_fixture(&shared_root, spec)?;
    if direct_fixture != separate_fixture || direct_fixture != shared_fixture {
        return Err(anyhow!("warmup fixture materialization drifted"));
    }

    let direct = rebuild_direct_standard(&direct_root)?;
    let separate = measure_separate(&separate_root)?;
    let shared = measure_shared(&shared_root)?;
    for (root, indexes) in [
        (direct_root.as_path(), &direct),
        (separate_root.as_path(), &separate),
        (shared_root.as_path(), &shared),
    ] {
        assert_semantic_oracles(root, indexes, spec)?;
    }
    let direct_output = assert_parity(&direct_root, &direct, &separate_root, &separate)?;
    let shared_output = assert_parity(&separate_root, &separate, &shared_root, &shared)?;
    if direct_output != shared_output {
        return Err(anyhow!(
            "direct-standard, instrumented-separate, and shared outputs differ"
        ));
    }
    drop(direct);
    drop(separate);
    drop(shared);
    fs::remove_dir_all(&warmup_root)
        .with_context(|| format!("failed to remove warmup root '{}'", warmup_root.display()))?;
    Ok((direct_fixture, direct_output))
}

fn rebuild_direct_standard(root: &Path) -> Result<MeasuredIndexes> {
    let repo = mapy_core::rebuild_repo_index_runtime(root, true)?;
    let regex = packet28_search_core::rebuild_full_index(root, true)?;
    Ok(MeasuredIndexes {
        elapsed_nanos: 0,
        io: IoEvidence::default(),
        repo,
        regex,
    })
}

impl MeasuredIndexes {
    fn sample(&self) -> MeasurementSample {
        MeasurementSample {
            elapsed_nanos: self.elapsed_nanos,
            input_io: self.io,
        }
    }
}

fn measure_separate(root: &Path) -> Result<MeasuredIndexes> {
    let started = Instant::now();
    let (repo, mut io) = rebuild_map_pass(root)?;
    let (regex, regex_io) = rebuild_regex_pass(root)?;
    io.merge(regex_io);
    Ok(MeasuredIndexes {
        elapsed_nanos: nanos(started),
        io,
        repo,
        regex,
    })
}

fn measure_shared(root: &Path) -> Result<MeasuredIndexes> {
    let started = Instant::now();
    let SharedIndexRuntimes {
        repo,
        regex,
        telemetry,
    } = rebuild_full_indexes_with_shared_scan(root, true, || false, |_| {})?;
    Ok(MeasuredIndexes {
        elapsed_nanos: nanos(started),
        io: telemetry.into(),
        repo,
        regex,
    })
}

fn rebuild_map_pass(root: &Path) -> Result<(RepoIndexRuntime, IoEvidence)> {
    let mut io = IoEvidence {
        walk_passes: 1,
        ..IoEvidence::default()
    };
    let root_owned = root.to_path_buf();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            entry
                .path()
                .strip_prefix(&root_owned)
                .ok()
                .is_none_or(|relative| {
                    mapy_core::shared_scan::wants_traversal(
                        &relative.to_string_lossy().replace('\\', "/"),
                    )
                })
        });
    let mut paths = Vec::new();
    for entry in builder.build() {
        let entry = entry?;
        io.walked_entries = io.walked_entries.saturating_add(1);
        let path = entry.path();
        io.classification_metadata_queries = io.classification_metadata_queries.saturating_add(1);
        if !path.is_file() {
            continue;
        }
        let relative_path = path.strip_prefix(root).unwrap_or(path);
        let Some(relative) = relative_path.to_str().map(|path| path.replace('\\', "/")) else {
            continue;
        };
        if mapy_core::shared_scan::wants_path(&relative, true) {
            paths.push(relative);
        }
    }
    paths.sort();
    let mut session = RepoIndexScanSession::begin(root, true, &paths)?;
    for relative in &paths {
        let path = root.join(relative);
        io.content_metadata_calls = io.content_metadata_calls.saturating_add(1);
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        io.observe_read(&bytes);
        session.ingest(relative, &metadata, &bytes)?;
    }
    let mut prepared = session.prepare()?;
    prepared.publish()?;
    Ok((prepared.commit()?, io))
}

fn rebuild_regex_pass(root: &Path) -> Result<(RegexIndexRuntime, IoEvidence)> {
    let mut io = IoEvidence {
        walk_passes: 1,
        ..IoEvidence::default()
    };
    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true);
    let mut paths = Vec::new();
    for entry in walker.build() {
        let Ok(entry) = entry else {
            io.ignored_walk_errors = io.ignored_walk_errors.saturating_add(1);
            continue;
        };
        io.walked_entries = io.walked_entries.saturating_add(1);
        let path = entry.into_path();
        io.classification_metadata_queries = io.classification_metadata_queries.saturating_add(1);
        if path.is_dir() {
            continue;
        }
        let Some(relative) = path.strip_prefix(root).ok() else {
            continue;
        };
        let normalized = relative.to_string_lossy().replace('\\', "/");
        if packet28_search_core::shared_scan::wants_path(&normalized) {
            paths.push((path, normalized));
        }
    }
    paths.sort_by(|left, right| left.0.cmp(&right.0));
    let discovered = paths
        .iter()
        .map(|(_, relative)| relative.clone())
        .collect::<Vec<_>>();
    let mut session = RegexIndexScanSession::begin(root, true, &discovered)?;
    for (path, relative) in paths {
        io.content_metadata_calls = io.content_metadata_calls.saturating_add(1);
        let metadata = fs::metadata(&path)?;
        if packet28_search_core::shared_scan::wants_content(&metadata) {
            let bytes = fs::read(&path)?;
            io.observe_read(&bytes);
            session.ingest(&relative, &metadata, &bytes)?;
        } else {
            session.ingest(&relative, &metadata, &[])?;
        }
    }
    let mut prepared = session.prepare()?;
    prepared.publish()?;
    Ok((prepared.commit()?, io))
}

fn assert_parity(
    separate_root: &Path,
    separate: &MeasuredIndexes,
    shared_root: &Path,
    shared: &MeasuredIndexes,
) -> Result<OutputEvidence> {
    let separate_snapshot = separate
        .repo
        .materialize_snapshot()
        .context("separate map snapshot missing")?;
    let shared_snapshot = shared
        .repo
        .materialize_snapshot()
        .context("shared map snapshot missing")?;
    if separate_snapshot != shared_snapshot {
        return Err(anyhow!("map snapshots differ"));
    }
    let separate_digests = separate
        .regex
        .shared_scan_content_digests()
        .context("separate regex digests missing")?;
    let shared_digests = shared
        .regex
        .shared_scan_content_digests()
        .context("shared regex digests missing")?;
    if separate_digests != shared_digests {
        return Err(anyhow!("regex content digests differ"));
    }
    let mut query_hasher = blake3::Hasher::new();
    for query in [
        "bench_symbol_7",
        "benchmark documentation needle",
        "generated_benchmark_symbol",
        "HiddenBenchmark",
        "ignored_benchmark_symbol",
        "before_nul",
        "exact_regex_boundary_symbol",
        "utf8_replacement_filename_symbol",
        "non_utf8_filename_symbol",
    ] {
        let request = SearchRequest {
            query: query.to_string(),
            fixed_string: true,
            ..SearchRequest::default()
        };
        let separate_result =
            packet28_search_core::indexed_search(separate_root, &separate.regex, &request)?;
        let shared_result =
            packet28_search_core::indexed_search(shared_root, &shared.regex, &request)?;
        if separate_result != shared_result {
            return Err(anyhow!("indexed query parity failed for '{query}'"));
        }
        query_hasher.update(&serde_json::to_vec(&shared_result)?);
    }
    Ok(OutputEvidence {
        map_snapshot: blake3::hash(&serde_json::to_vec(&shared_snapshot)?)
            .to_hex()
            .to_string(),
        regex: RegexOutputEvidence {
            lookup: shared_digests.lookup,
            postings: shared_digests.postings,
            documents: shared_digests.documents,
        },
        query_results: query_hasher.finalize().to_hex().to_string(),
    })
}

fn assert_semantic_oracles(
    root: &Path,
    indexes: &MeasuredIndexes,
    spec: FixtureSpec,
) -> Result<()> {
    let map = indexes
        .repo
        .materialize_snapshot()
        .context("map snapshot missing for semantic oracle")?;
    let regex = indexes
        .regex
        .shared_scan_document_paths()
        .context("regex documents missing for semantic oracle")?;

    assert_membership(map.files.contains_key(".hidden.rs"), "map hidden file")?;
    assert_membership(
        map.files.contains_key("src/empty.rs"),
        "map empty UTF-8 source",
    )?;
    assert_membership(map.files.contains_key("src/nul.rs"), "map NUL source")?;
    assert_membership(
        map.files.contains_key("src/collision_\u{fffd}.rs"),
        "map replacement-character source",
    )?;
    assert_membership(
        !map.files.contains_key("build/generated.rs"),
        "map generated exclusion",
    )?;
    assert_membership(
        !map.files.contains_key("ignored.rs"),
        "map gitignore exclusion",
    )?;
    assert_membership(
        !map.files.contains_key("src/invalid.rs"),
        "map invalid UTF-8 content exclusion",
    )?;

    assert_membership(regex_contains(&regex, ".hidden.rs"), "regex hidden file")?;
    assert_membership(
        regex_contains(&regex, "build/generated.rs"),
        "regex generated file",
    )?;
    assert_membership(
        regex_contains(&regex, "src/invalid.rs"),
        "regex invalid UTF-8 bytes",
    )?;
    for excluded in ["ignored.rs", "src/empty.rs", "src/nul.rs"] {
        assert_membership(
            !regex_contains(&regex, excluded),
            "regex ignored/empty/NUL exclusion",
        )?;
    }
    assert_fixture_specific_oracles(&map.files, &regex, spec)?;
    assert_query_oracle(root, &indexes.regex, "generated_benchmark_symbol", true)?;
    assert_query_oracle(root, &indexes.regex, "HiddenBenchmark", true)?;
    assert_query_oracle(root, &indexes.regex, "ignored_benchmark_symbol", false)?;
    assert_query_oracle(root, &indexes.regex, "before_nul", false)?;
    Ok(())
}

fn assert_fixture_specific_oracles(
    map: &BTreeMap<String, mapy_core::RepoIndexFileEntry>,
    regex: &[String],
    spec: FixtureSpec,
) -> Result<()> {
    if spec.oversize_source_files > 0 {
        assert_membership(
            map.contains_key("src/oversize_0000.rs"),
            "map oversize source",
        )?;
        assert_membership(
            !regex_contains(regex, "src/oversize_0000.rs"),
            "regex oversize source exclusion",
        )?;
        assert_membership(
            !regex_contains(regex, "docs/oversize_0000.md"),
            "regex oversize text exclusion",
        )?;
    }
    if spec.exact_regex_boundary {
        assert_membership(
            map.contains_key("src/exact-regex-boundary.rs"),
            "map exact regex boundary source",
        )?;
        assert_membership(
            regex_contains(regex, "src/exact-regex-boundary.rs"),
            "regex exact size boundary source",
        )?;
    }
    #[cfg(unix)]
    {
        assert_membership(map.contains_key("src/module-link.rs"), "map symlink source")?;
        assert_membership(
            regex_contains(regex, "src/module-link.rs"),
            "regex symlink source",
        )?;
    }
    #[cfg(target_os = "linux")]
    assert_membership(
        regex
            .iter()
            .filter(|path| path.as_str() == "src/collision_\u{fffd}.rs")
            .count()
            == 2,
        "regex lossy-key collision multiplicity",
    )?;
    Ok(())
}

fn assert_query_oracle(
    root: &Path,
    regex: &RegexIndexRuntime,
    query: &str,
    expected_match: bool,
) -> Result<()> {
    let result = packet28_search_core::indexed_search(
        root,
        regex,
        &SearchRequest {
            query: query.to_string(),
            fixed_string: true,
            ..SearchRequest::default()
        },
    )?;
    assert_membership(
        (result.returned_match_count > 0) == expected_match,
        "indexed query policy",
    )
}

fn regex_contains(paths: &[String], expected: &str) -> bool {
    paths.iter().any(|path| path == expected)
}

fn assert_membership(actual: bool, policy: &str) -> Result<()> {
    if actual {
        Ok(())
    } else {
        Err(anyhow!("semantic oracle failed: {policy}"))
    }
}

fn materialize_fixture(root: &Path, spec: FixtureSpec) -> Result<FixtureEvidence> {
    let mut files_written = 0u64;
    let mut bytes_written = 0u64;
    for index in 0..spec.source_files {
        write_fixture_file(
            root,
            &format!("src/module_{index:04}.rs"),
            &source_bytes(index, spec.source_bytes),
            &mut files_written,
            &mut bytes_written,
        )?;
    }
    for index in 0..spec.test_files {
        write_fixture_file(
            root,
            &format!("tests/case_{index:04}.rs"),
            &source_bytes(index + spec.source_files, spec.test_bytes),
            &mut files_written,
            &mut bytes_written,
        )?;
    }
    for index in 0..spec.text_files {
        write_fixture_file(
            root,
            &format!("docs/guide_{index:04}.md"),
            &text_bytes(index, spec.text_bytes),
            &mut files_written,
            &mut bytes_written,
        )?;
    }
    for index in 0..spec.oversize_source_files {
        write_fixture_file(
            root,
            &format!("src/oversize_{index:04}.rs"),
            &source_bytes(
                index + spec.source_files + spec.test_files,
                packet28_search_core::shared_scan::MAX_SHARED_SCAN_CONTENT_BYTES + 512 * 1024,
            ),
            &mut files_written,
            &mut bytes_written,
        )?;
    }
    for index in 0..spec.oversize_text_files {
        write_fixture_file(
            root,
            &format!("docs/oversize_{index:04}.md"),
            &text_bytes(
                index + spec.text_files,
                packet28_search_core::shared_scan::MAX_SHARED_SCAN_CONTENT_BYTES + 512 * 1024,
            ),
            &mut files_written,
            &mut bytes_written,
        )?;
    }
    if spec.exact_regex_boundary {
        write_fixture_file(
            root,
            "src/exact-regex-boundary.rs",
            &padded_bytes(
                b"pub fn exact_regex_boundary_symbol() {}\n",
                packet28_search_core::shared_scan::MAX_SHARED_SCAN_CONTENT_BYTES,
                b' ',
            ),
            &mut files_written,
            &mut bytes_written,
        )?;
    }
    for (path, bytes) in [
        (".git/HEAD", b"ref: refs/heads/benchmark\n".as_slice()),
        (
            "build/generated.rs",
            b"pub fn generated_benchmark_symbol() {}\n".as_slice(),
        ),
        (".hidden.rs", b"pub struct HiddenBenchmark;\n".as_slice()),
        ("src/empty.rs", b"".as_slice()),
        ("src/nul.rs", b"pub fn before_nul() {}\0after\n".as_slice()),
        ("src/invalid.rs", &[0xff, 0xfe, b'a']),
        (".gitignore", b"ignored.rs\n".as_slice()),
        (
            "ignored.rs",
            b"pub fn ignored_benchmark_symbol() {}\n".as_slice(),
        ),
        (
            "src/collision_\u{fffd}.rs",
            b"pub fn utf8_replacement_filename_symbol() {}\n".as_slice(),
        ),
    ] {
        write_fixture_file(root, path, bytes, &mut files_written, &mut bytes_written)?;
    }
    #[cfg(target_os = "linux")]
    write_non_utf8_fixture(root, &mut files_written, &mut bytes_written)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("module_0000.rs", root.join("src/module-link.rs"))?;
        files_written = files_written.saturating_add(1);
    }
    Ok(FixtureEvidence {
        name: spec.name.to_string(),
        filesystem_entries_written: files_written,
        bytes_written,
    })
}

fn source_bytes(index: usize, size: usize) -> Vec<u8> {
    let header =
        format!("pub fn bench_symbol_{index}() -> usize {{ {index} }}\n// benchmark padding\n");
    padded_bytes(header.as_bytes(), size, b' ')
}

fn text_bytes(index: usize, size: usize) -> Vec<u8> {
    let header = format!("benchmark documentation needle {index}\n");
    padded_bytes(header.as_bytes(), size, b'x')
}

fn padded_bytes(prefix: &[u8], size: usize, fill: u8) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(size.max(prefix.len()));
    bytes.extend_from_slice(prefix);
    bytes.resize(size.max(prefix.len()), fill);
    bytes
}

fn write_fixture_file(
    root: &Path,
    relative: &str,
    bytes: &[u8],
    files_written: &mut u64,
    bytes_written: &mut u64,
) -> Result<()> {
    write_fixture_path(
        root,
        Path::new(relative),
        bytes,
        files_written,
        bytes_written,
    )
}

fn write_fixture_path(
    root: &Path,
    relative: &Path,
    bytes: &[u8],
    files_written: &mut u64,
    bytes_written: &mut u64,
) -> Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, bytes)?;
    let file = fs::OpenOptions::new().write(true).open(&path)?;
    file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(FIXED_MTIME_UNIX))?;
    *files_written = files_written.saturating_add(1);
    *bytes_written = bytes_written.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    Ok(())
}

#[cfg(target_os = "linux")]
fn write_non_utf8_fixture(
    root: &Path,
    files_written: &mut u64,
    bytes_written: &mut u64,
) -> Result<()> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let mut filename = b"collision_".to_vec();
    filename.push(0xff);
    filename.extend_from_slice(b".rs");
    let relative = Path::new("src").join(OsString::from_vec(filename));
    write_fixture_path(
        root,
        &relative,
        b"pub fn non_utf8_filename_symbol() {}\n",
        files_written,
        bytes_written,
    )
}

fn stable_io(runs: &[MeasurementSample], fixture: &str, strategy: &str) -> Result<IoEvidence> {
    let first = runs
        .first()
        .map(|run| run.input_io)
        .context("benchmark needs at least one iteration")?;
    if runs.iter().any(|run| run.input_io != first) {
        return Err(anyhow!(
            "{fixture} {strategy} I/O counters changed between iterations"
        ));
    }
    Ok(first)
}

fn median_u64(values: &[u64]) -> u64 {
    let mut values = values.to_vec();
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        u64::try_from(u128::midpoint(
            u128::from(values[middle - 1]),
            u128::from(values[middle]),
        ))
        .unwrap_or(u64::MAX)
    } else {
        values[middle]
    }
}

fn median_i128(values: &[i128]) -> i128 {
    let mut values = values.to_vec();
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        i128::midpoint(values[middle - 1], values[middle])
    } else {
        values[middle]
    }
}

fn nanos(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn basis_point_delta(current: u64, baseline: u64) -> i128 {
    if baseline == 0 {
        return 0;
    }
    (i128::from(current) - i128::from(baseline)) * 10_000 / i128::from(baseline)
}

fn parse_args(arguments: &[String]) -> Result<Arguments> {
    let mut iterations = MIN_ITERATIONS;
    let mut output = PathBuf::from("production-result.local.json");
    let mut cursor = 1usize;
    while cursor < arguments.len() {
        match arguments[cursor].as_str() {
            "--iterations" => {
                cursor += 1;
                iterations = arguments
                    .get(cursor)
                    .context("--iterations needs a value")?
                    .parse()
                    .context("invalid --iterations value")?;
            }
            "--output" => {
                cursor += 1;
                output = PathBuf::from(arguments.get(cursor).context("--output needs a value")?);
            }
            unknown => return Err(anyhow!("unknown argument '{unknown}'")),
        }
        cursor += 1;
    }
    if iterations < MIN_ITERATIONS || !iterations.is_multiple_of(2) {
        return Err(anyhow!(
            "iterations must be an even number greater than or equal to {MIN_ITERATIONS}"
        ));
    }
    Ok(Arguments { iterations, output })
}

fn file_digest(path: &Path) -> Result<String> {
    Ok(blake3::hash(
        &fs::read(path).with_context(|| format!("failed to read '{}'", path.display()))?,
    )
    .to_hex()
    .to_string())
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn command_output_at(directory: &Path, program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .current_dir(directory)
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn collect_host_evidence() -> HostEvidence {
    let mac_hardware = command_output(
        "system_profiler",
        &["SPHardwareDataType", "-detailLevel", "mini"],
    );
    HostEvidence {
        uname: command_output("uname", &["-a"]),
        cpu_model: mac_hardware
            .as_deref()
            .and_then(|profile| hardware_field(profile, "Chip"))
            .or_else(|| command_output("sysctl", &["-n", "machdep.cpu.brand_string"]))
            .filter(|value| !value.is_empty())
            .or_else(|| command_output("sysctl", &["-n", "hw.model"]))
            .filter(|value| !value.is_empty())
            .or_else(linux_cpu_model),
        memory_bytes: mac_hardware
            .as_deref()
            .and_then(|profile| hardware_field(profile, "Memory"))
            .and_then(|value| parse_memory_bytes(&value))
            .or_else(|| {
                command_output("sysctl", &["-n", "hw.memsize"]).and_then(|value| value.parse().ok())
            })
            .or_else(linux_memory_bytes),
        available_parallelism: std::thread::available_parallelism()
            .ok()
            .map(std::num::NonZero::get),
    }
}

fn hardware_field(profile: &str, field: &str) -> Option<String> {
    profile.lines().find_map(|line| {
        let (key, value) = line.trim().split_once(':')?;
        (key == field).then(|| value.trim().to_string())
    })
}

fn parse_memory_bytes(value: &str) -> Option<u64> {
    let mut components = value.split_whitespace();
    let amount = components.next()?.parse::<u64>().ok()?;
    let multiplier = match components.next()?.to_ascii_uppercase().as_str() {
        "B" => 1,
        "KB" => 1_000,
        "KIB" => 1_024,
        "MB" => 1_000_000,
        "MIB" => 1_048_576,
        "GB" => 1_000_000_000,
        "GIB" => 1_073_741_824,
        _ => return None,
    };
    amount.checked_mul(multiplier)
}

fn linux_cpu_model() -> Option<String> {
    fs::read_to_string("/proc/cpuinfo")
        .ok()?
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == "model name").then(|| value.trim().to_string())
        })
}

fn linux_memory_bytes() -> Option<u64> {
    let memory = fs::read_to_string("/proc/meminfo").ok()?;
    let kibibytes = memory.lines().find_map(|line| {
        let remainder = line.strip_prefix("MemTotal:")?;
        remainder.split_whitespace().next()?.parse::<u64>().ok()
    })?;
    kibibytes.checked_mul(1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_reproducible_and_nonempty() {
        let arguments = parse_args(&["production".to_string()]).unwrap();

        assert_eq!(arguments.iterations, MIN_ITERATIONS);
        assert_eq!(
            arguments.output,
            PathBuf::from("production-result.local.json")
        );
    }

    #[test]
    fn invalid_iterations_and_unknown_arguments_are_rejected() {
        for invalid in ["0", "5", "7"] {
            let invalid_iterations = parse_args(&[
                "production".to_string(),
                "--iterations".to_string(),
                invalid.to_string(),
            ]);
            assert!(invalid_iterations.is_err());
        }

        let unknown = parse_args(&["production".to_string(), "--unknown".to_string()]);
        assert!(unknown.is_err());
    }

    #[test]
    fn basis_point_delta_and_median_are_integer_stable() {
        assert_eq!(median_u64(&[5, 1, 3]), 3);
        assert_eq!(median_u64(&[2, 4, 8, 10]), 6);
        assert_eq!(median_i128(&[-3, -1, 5, 9]), 2);
        assert_eq!(basis_point_delta(75, 100), -2_500);
        assert_eq!(basis_point_delta(125, 100), 2_500);
        assert_eq!(basis_point_delta(1, 0), 0);
    }

    #[test]
    fn direct_instrumented_and_shared_warmups_match() {
        let temporary = tempfile::tempdir().unwrap();
        let tiny = FixtureSpec {
            name: "unit",
            source_files: 2,
            source_bytes: 256,
            test_files: 1,
            test_bytes: 256,
            text_files: 1,
            text_bytes: 256,
            oversize_source_files: 0,
            oversize_text_files: 0,
            exact_regex_boundary: false,
        };

        let (fixture, output) = warmup_and_verify(temporary.path(), tiny).unwrap();

        assert!(fixture.filesystem_entries_written > 4);
        assert!(!output.map_snapshot.is_empty());
        assert!(!output.regex.documents.is_empty());
        assert!(!output.query_results.is_empty());
    }

    #[test]
    fn mac_hardware_profile_fields_are_parsed_without_sysctl() {
        let profile = "Hardware:\n\n    Hardware Overview:\n\n      Chip: Apple M4 Pro\n      Memory: 24 GB\n";

        assert_eq!(
            hardware_field(profile, "Chip").as_deref(),
            Some("Apple M4 Pro")
        );
        assert_eq!(
            hardware_field(profile, "Memory")
                .as_deref()
                .and_then(parse_memory_bytes),
            Some(24_000_000_000)
        );
        assert_eq!(parse_memory_bytes("32 GiB"), Some(34_359_738_368));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_host_evidence_is_populated() {
        let host = collect_host_evidence();

        assert!(host.cpu_model.is_some());
        assert!(host.memory_bytes.is_some());
        assert!(host.available_parallelism.is_some());
    }
}
