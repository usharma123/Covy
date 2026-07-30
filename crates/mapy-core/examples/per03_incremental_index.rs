//! Deterministic release benchmark for PER-03 incremental index generations.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mapy_core::{
    build_repo_index, rebuild_repo_index_runtime, update_repo_index, update_repo_index_runtime,
};
use packet28_search_core::{rebuild_full_index, update_overlay_index};

const MAPY_FILE_COUNT: usize = 1_024;
const REGEX_FILE_COUNT: usize = 256;
const SEEDED_OVERLAY_FILES: usize = 96;
const MEASURED_UPDATES: usize = 5;

fn main() -> AnyResult<()> {
    let mapy_fixture = BenchmarkFixture::new("mapy")?;
    mapy_fixture.populate(MAPY_FILE_COUNT)?;
    let (mapy_legacy, mapy_incremental) = measure_mapy_pair(&mapy_fixture)?;
    let mapy_compaction_fixture = BenchmarkFixture::new("mapy-compaction")?;
    mapy_compaction_fixture.populate(MAPY_FILE_COUNT)?;
    let mapy_compaction = measure_mapy_compaction(&mapy_compaction_fixture)?;

    let regex_legacy_fixture = BenchmarkFixture::new("regex-legacy")?;
    regex_legacy_fixture.populate(REGEX_FILE_COUNT)?;
    let regex_legacy = measure_regex_full_overlay_model(&regex_legacy_fixture)?;

    let regex_incremental_fixture = BenchmarkFixture::new("regex-incremental")?;
    regex_incremental_fixture.populate(REGEX_FILE_COUNT)?;
    let regex_incremental = measure_regex_incremental(&regex_incremental_fixture)?;
    let regex_compaction_fixture = BenchmarkFixture::new("regex-compaction")?;
    regex_compaction_fixture.populate(REGEX_FILE_COUNT)?;
    let regex_compaction = measure_regex_compaction(&regex_compaction_fixture)?;

    print_measurement("mapy_legacy", mapy_legacy);
    print_measurement("mapy_incremental", mapy_incremental);
    print_measurement("mapy_compaction", mapy_compaction);
    print_measurement("regex_full_overlay_model", regex_legacy);
    print_measurement("regex_incremental", regex_incremental);
    print_measurement("regex_compaction", regex_compaction);
    println!(
        "mapy_time_delta_pct={:+.2} mapy_bytes_delta_pct={:+.2}",
        duration_delta(mapy_legacy.elapsed, mapy_incremental.elapsed),
        signed_delta(mapy_legacy.bytes, mapy_incremental.bytes)
    );
    println!(
        "regex_time_delta_pct={:+.2} regex_bytes_delta_pct={:+.2}",
        duration_delta(regex_legacy.elapsed, regex_incremental.elapsed),
        signed_delta(regex_legacy.bytes, regex_incremental.bytes)
    );
    Ok(())
}

fn measure_mapy_compaction(fixture: &BenchmarkFixture) -> AnyResult<Measurement> {
    let mut runtime = rebuild_repo_index_runtime(&fixture.root, true)?;
    let seeded_paths = seeded_paths();
    for idx in 0..SEEDED_OVERLAY_FILES {
        fixture.write_source(idx, 100)?;
    }
    runtime = update_repo_index_runtime(&fixture.root, &runtime, &seeded_paths, true)?.0;
    let target = String::from("src/file_000.rs");
    for revision in 0..6 {
        fixture.write_source(0, 300 + revision)?;
        runtime = update_repo_index_runtime(
            &fixture.root,
            &runtime,
            std::slice::from_ref(&target),
            true,
        )?
        .0;
    }
    fixture.write_source(0, 400)?;
    let before = snapshot_tree(&fixture.mapy_index_dir())?;
    let started = Instant::now();
    let compacted =
        update_repo_index_runtime(&fixture.root, &runtime, std::slice::from_ref(&target), true)?.0;
    let elapsed = started.elapsed();
    let after = snapshot_tree(&fixture.mapy_index_dir())?;
    if compacted.manifest.segment_count != 1 {
        return Err("mapy compaction did not publish one segment".into());
    }
    Ok(Measurement {
        elapsed,
        bytes: changed_file_bytes(&before, &after),
        work: None,
    })
}

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy)]
struct Measurement {
    elapsed: Duration,
    bytes: u64,
    work: Option<UpdateWork>,
}

#[derive(Clone, Copy)]
struct UpdateWork {
    publication_metadata_bytes_decoded: usize,
    repository_artifact_bytes_decoded: usize,
    repository_artifacts_decoded: usize,
    repository_artifact_bytes_hashed: usize,
    repository_artifact_metadata_checks: usize,
    changed_paths_considered: usize,
}

fn measure_mapy_pair(fixture: &BenchmarkFixture) -> AnyResult<(Measurement, Measurement)> {
    let mut legacy = build_repo_index(&fixture.root, true)?;
    let mut incremental = rebuild_repo_index_runtime(&fixture.root, true)?;
    let seeded_paths = seeded_paths();
    for idx in 0..SEEDED_OVERLAY_FILES {
        fixture.write_source(idx, 100)?;
    }
    update_repo_index(&fixture.root, &mut legacy, &seeded_paths, true)?;
    incremental = update_repo_index_runtime(&fixture.root, &incremental, &seeded_paths, true)?.0;

    let target = String::from("src/file_000.rs");
    let mut legacy_samples = Vec::with_capacity(MEASURED_UPDATES);
    let mut incremental_samples = Vec::with_capacity(MEASURED_UPDATES);
    let mut legacy_bytes = 0;
    let mut incremental_bytes = 0;
    let mut incremental_work = None;

    for revision in 0..MEASURED_UPDATES {
        fixture.write_source(0, 200 + revision)?;

        let legacy_started = Instant::now();
        let mut next = legacy.clone();
        update_repo_index(
            &fixture.root,
            &mut next,
            std::slice::from_ref(&target),
            true,
        )?;
        let encoded = packet28_binary_codec::serialize(&next)?;
        let legacy_path = fixture.root.join(".packet28/index/mapy-legacy.bin");
        if let Some(parent) = legacy_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(legacy_path, &encoded)?;
        legacy_samples.push(legacy_started.elapsed());
        legacy_bytes = encoded.len() as u64;
        legacy = next;

        let before = snapshot_tree(&fixture.mapy_index_dir())?;
        let incremental_started = Instant::now();
        let (next_incremental, summary) = update_repo_index_runtime(
            &fixture.root,
            &incremental,
            std::slice::from_ref(&target),
            true,
        )?;
        incremental = next_incremental;
        incremental_work = Some(UpdateWork {
            publication_metadata_bytes_decoded: summary.work.publication_metadata_bytes_decoded,
            repository_artifact_bytes_decoded: summary.work.repository_artifact_bytes_decoded,
            repository_artifacts_decoded: summary.work.repository_artifacts_decoded,
            repository_artifact_bytes_hashed: summary.work.repository_artifact_bytes_hashed,
            repository_artifact_metadata_checks: summary.work.repository_artifact_metadata_checks,
            changed_paths_considered: summary.work.changed_paths_considered,
        });
        incremental_samples.push(incremental_started.elapsed());
        let after = snapshot_tree(&fixture.mapy_index_dir())?;
        incremental_bytes = changed_file_bytes(&before, &after);
    }

    Ok((
        Measurement {
            elapsed: median(legacy_samples),
            bytes: legacy_bytes,
            work: None,
        },
        Measurement {
            elapsed: median(incremental_samples),
            bytes: incremental_bytes,
            work: incremental_work,
        },
    ))
}

fn measure_regex_full_overlay_model(fixture: &BenchmarkFixture) -> AnyResult<Measurement> {
    let mut runtime = rebuild_full_index(&fixture.root, true)?;
    let seeded_paths = seeded_paths();
    for idx in 0..SEEDED_OVERLAY_FILES {
        fixture.write_source(idx, 100)?;
    }
    runtime = update_overlay_index(&fixture.root, Some(&runtime), &seeded_paths)?;

    let mut samples = Vec::with_capacity(MEASURED_UPDATES);
    let mut changed_bytes = 0;
    for revision in 0..MEASURED_UPDATES {
        fixture.write_source(0, 200 + revision)?;
        let before = snapshot_tree(&fixture.regex_index_dir())?;
        let started = Instant::now();
        // This intentionally supplies every live overlay path. It models the
        // pre-PER-03 algorithm, which reread and rebuilt the complete overlay
        // even when only one path changed.
        runtime = update_overlay_index(&fixture.root, Some(&runtime), &seeded_paths)?;
        samples.push(started.elapsed());
        let after = snapshot_tree(&fixture.regex_index_dir())?;
        changed_bytes = changed_file_bytes(&before, &after);
    }
    Ok(Measurement {
        elapsed: median(samples),
        bytes: changed_bytes,
        work: None,
    })
}

fn measure_regex_incremental(fixture: &BenchmarkFixture) -> AnyResult<Measurement> {
    let mut runtime = rebuild_full_index(&fixture.root, true)?;
    let seeded_paths = seeded_paths();
    for idx in 0..SEEDED_OVERLAY_FILES {
        fixture.write_source(idx, 100)?;
    }
    runtime = update_overlay_index(&fixture.root, Some(&runtime), &seeded_paths)?;

    let target = String::from("src/file_000.rs");
    let mut samples = Vec::with_capacity(MEASURED_UPDATES);
    let mut changed_bytes = 0;
    for revision in 0..MEASURED_UPDATES {
        fixture.write_source(0, 200 + revision)?;
        let before = snapshot_tree(&fixture.regex_index_dir())?;
        let started = Instant::now();
        runtime =
            update_overlay_index(&fixture.root, Some(&runtime), std::slice::from_ref(&target))?;
        samples.push(started.elapsed());
        let after = snapshot_tree(&fixture.regex_index_dir())?;
        changed_bytes = changed_file_bytes(&before, &after);
    }
    Ok(Measurement {
        elapsed: median(samples),
        bytes: changed_bytes,
        work: None,
    })
}

fn measure_regex_compaction(fixture: &BenchmarkFixture) -> AnyResult<Measurement> {
    let mut runtime = rebuild_full_index(&fixture.root, true)?;
    let seeded_paths = seeded_paths();
    for idx in 0..SEEDED_OVERLAY_FILES {
        fixture.write_source(idx, 100)?;
    }
    runtime = update_overlay_index(&fixture.root, Some(&runtime), &seeded_paths)?;
    let target = String::from("src/file_000.rs");
    for revision in 0..6 {
        fixture.write_source(0, 300 + revision)?;
        runtime =
            update_overlay_index(&fixture.root, Some(&runtime), std::slice::from_ref(&target))?;
    }
    fixture.write_source(0, 400)?;
    let before = snapshot_tree(&fixture.regex_index_dir())?;
    let started = Instant::now();
    let compacted =
        update_overlay_index(&fixture.root, Some(&runtime), std::slice::from_ref(&target))?;
    let elapsed = started.elapsed();
    let after = snapshot_tree(&fixture.regex_index_dir())?;
    if compacted.manifest.overlay_segments != 1 {
        return Err("regex compaction did not publish one segment".into());
    }
    Ok(Measurement {
        elapsed,
        bytes: changed_file_bytes(&before, &after),
        work: None,
    })
}

fn seeded_paths() -> Vec<String> {
    (0..SEEDED_OVERLAY_FILES)
        .map(|idx| format!("src/file_{idx:03}.rs"))
        .collect()
}

fn snapshot_tree(root: &Path) -> std::io::Result<BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(
        root: &Path,
        current: &Path,
        out: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> std::io::Result<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, out)?;
            } else {
                let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                out.insert(relative, fs::read(path)?);
            }
        }
        Ok(())
    }

    let mut out = BTreeMap::new();
    visit(root, root, &mut out)?;
    Ok(out)
}

fn changed_file_bytes(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> u64 {
    after
        .iter()
        .filter(|(path, bytes)| before.get(*path) != Some(*bytes))
        .map(|(_, bytes)| bytes.len() as u64)
        .sum()
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn duration_delta(before: Duration, after: Duration) -> f64 {
    signed_delta(before.as_nanos(), after.as_nanos())
}

fn signed_delta(before: impl Into<u128>, after: impl Into<u128>) -> f64 {
    let before = before.into();
    let after = after.into();
    if before == 0 {
        return 0.0;
    }
    ((after as f64 - before as f64) / before as f64) * 100.0
}

fn print_measurement(label: &str, measurement: Measurement) {
    println!(
        "{label}_median_us={} {label}_publication_bytes={}",
        measurement.elapsed.as_micros(),
        measurement.bytes
    );
    if let Some(work) = measurement.work {
        println!(
            "{label}_publication_metadata_bytes_decoded={} \
             {label}_repository_artifact_bytes_decoded={} \
             {label}_repository_artifacts_decoded={} \
             {label}_repository_artifact_bytes_hashed={} \
             {label}_repository_artifact_metadata_checks={} \
             {label}_changed_paths_considered={}",
            work.publication_metadata_bytes_decoded,
            work.repository_artifact_bytes_decoded,
            work.repository_artifacts_decoded,
            work.repository_artifact_bytes_hashed,
            work.repository_artifact_metadata_checks,
            work.changed_paths_considered,
        );
    }
}

struct BenchmarkFixture {
    root: PathBuf,
}

impl BenchmarkFixture {
    fn new(label: &str) -> std::io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "packet28-per03-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src"))?;
        Ok(Self { root })
    }

    fn populate(&self, file_count: usize) -> std::io::Result<()> {
        for idx in 0..file_count {
            self.write_source(idx, 0)?;
        }
        Ok(())
    }

    fn write_source(&self, idx: usize, revision: usize) -> std::io::Result<()> {
        let mut body = format!(
            "pub fn file_{idx:03}_revision_{revision}(value: usize) -> usize {{\n    value + {revision}\n}}\n"
        );
        for line in 0..32 {
            body.push_str(&format!(
                "pub const FILE_{idx:03}_LINE_{line:02}: &str = \"packet28 deterministic index benchmark\";\n"
            ));
        }
        fs::write(self.root.join(format!("src/file_{idx:03}.rs")), body)
    }

    fn mapy_index_dir(&self) -> PathBuf {
        self.root.join(".packet28").join("index").join("mapy-v1")
    }

    fn regex_index_dir(&self) -> PathBuf {
        self.root.join(".packet28").join("index").join("regex-v1")
    }
}

impl Drop for BenchmarkFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
