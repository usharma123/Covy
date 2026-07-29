use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use context_memory_core::{
    CachePacket, CachePersistence, NoopDeltaReuseHooks, PacketCache, PacketCacheEntry,
    PersistConfig,
};
use serde::Serialize;
use serde_json::{json, Value};
use tempfile::tempdir;

const FIXTURE_ENTRIES: usize = 512;
const MEASURED_WRITES: usize = 64;
const PAYLOAD_BYTES: usize = 1_024;
const REPEATS: usize = 3;

#[derive(Serialize)]
struct PathMeasurement {
    median_write_lock_ns: u64,
    median_elapsed_us: u64,
    median_published_bytes: u64,
}

#[derive(Serialize)]
struct ExperimentResult {
    schema_version: u32,
    fixture_entries: usize,
    measured_writes: usize,
    payload_bytes: usize,
    repeats: usize,
    legacy_full_checkpoint_under_lock: PathMeasurement,
    owned_delta_wal_after_lock: PathMeasurement,
    lock_time_reduction_percent: f64,
    write_byte_reduction_percent: f64,
    parity_entries: usize,
}

struct Observation {
    write_lock_ns: u64,
    elapsed_us: u64,
    published_bytes: u64,
    parity_entries: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut legacy = Vec::with_capacity(REPEATS);
    let mut owned = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        legacy.push(run_legacy_model()?);
        owned.push(run_owned_delta_path()?);
    }

    let legacy = summarize(&legacy);
    let owned = summarize(&owned);
    let parity_entries = FIXTURE_ENTRIES + MEASURED_WRITES;
    let result = ExperimentResult {
        schema_version: 1,
        fixture_entries: FIXTURE_ENTRIES,
        measured_writes: MEASURED_WRITES,
        payload_bytes: PAYLOAD_BYTES,
        repeats: REPEATS,
        lock_time_reduction_percent: percent_reduction(
            legacy.median_write_lock_ns,
            owned.median_write_lock_ns,
        ),
        write_byte_reduction_percent: percent_reduction(
            legacy.median_published_bytes,
            owned.median_published_bytes,
        ),
        legacy_full_checkpoint_under_lock: legacy,
        owned_delta_wal_after_lock: owned,
        parity_entries,
    };

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn run_legacy_model() -> Result<Observation, Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let config = PersistConfig::new(dir.path().to_path_buf());
    let cache = Arc::new(Mutex::new(seed_cache()));
    {
        let cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.save_to_disk(&config)?;
    }

    let started = Instant::now();
    let mut lock_samples = Vec::with_capacity(MEASURED_WRITES);
    let mut published_bytes = 0u64;
    for offset in 0..MEASURED_WRITES {
        let lock_started = Instant::now();
        let mut cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        put_entry(&mut cache, FIXTURE_ENTRIES + offset, PAYLOAD_BYTES);
        cache.save_to_disk(&config)?;
        published_bytes = published_bytes
            .saturating_add(std::fs::metadata(PacketCache::persist_file_path(dir.path()))?.len());
        drop(cache);
        lock_samples.push(elapsed_nanos(lock_started));
    }
    let parity_entries = PacketCache::load_from_disk(&config).len();

    Ok(Observation {
        write_lock_ns: median(&mut lock_samples),
        elapsed_us: elapsed_micros(started),
        published_bytes,
        parity_entries,
    })
}

fn run_owned_delta_path() -> Result<Observation, Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let config = PersistConfig::new(dir.path().to_path_buf());
    let initial = seed_cache();
    initial.save_to_disk(&config)?;
    let cache = Arc::new(Mutex::new(initial.clone()));
    let mut owner = CachePersistence::start(config.clone(), initial)?;

    let started = Instant::now();
    let mut lock_samples = Vec::with_capacity(MEASURED_WRITES);
    for offset in 0..MEASURED_WRITES {
        let lock_started = Instant::now();
        let entry = {
            let mut cache = cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            put_entry(&mut cache, FIXTURE_ENTRIES + offset, PAYLOAD_BYTES)
        };
        lock_samples.push(elapsed_nanos(lock_started));
        owner.record_update(&entry, Vec::new())?;
    }
    let metrics = owner.flush(Duration::from_secs(30))?;
    let elapsed_us = elapsed_micros(started);
    let parity_entries = PacketCache::load_from_disk(&config).len();
    owner.shutdown(Duration::from_secs(30))?;

    Ok(Observation {
        write_lock_ns: median(&mut lock_samples),
        elapsed_us,
        published_bytes: metrics.wal_bytes,
        parity_entries,
    })
}

fn seed_cache() -> PacketCache {
    let mut cache = PacketCache::new();
    for id in 0..FIXTURE_ENTRIES {
        put_entry(&mut cache, id, PAYLOAD_BYTES);
    }
    cache
}

fn put_entry(cache: &mut PacketCache, id: usize, payload_bytes: usize) -> PacketCacheEntry {
    let target = format!("benchmark.reducer.{id}");
    let mut hooks = NoopDeltaReuseHooks;
    let lookup = cache.lookup_with_hooks(&target, &json!({"id": id}), &mut hooks);
    cache.put_with_hooks(
        &target,
        &lookup,
        vec![CachePacket {
            packet_id: Some(format!("packet-{id}")),
            body: json!({
                "summary": format!("cache persistence benchmark entry {id}"),
                "payload": "x".repeat(payload_bytes),
                "files": [{"path": format!("src/generated/{id}.rs")}],
            }),
            ..CachePacket::default()
        }],
        Value::Null,
        &mut hooks,
    )
}

fn summarize(observations: &[Observation]) -> PathMeasurement {
    assert!(
        observations
            .iter()
            .all(|observation| observation.parity_entries == FIXTURE_ENTRIES + MEASURED_WRITES),
        "persistence paths must recover the complete fixture"
    );
    let mut locks = observations
        .iter()
        .map(|observation| observation.write_lock_ns)
        .collect::<Vec<_>>();
    let mut elapsed = observations
        .iter()
        .map(|observation| observation.elapsed_us)
        .collect::<Vec<_>>();
    let mut bytes = observations
        .iter()
        .map(|observation| observation.published_bytes)
        .collect::<Vec<_>>();
    PathMeasurement {
        median_write_lock_ns: median(&mut locks),
        median_elapsed_us: median(&mut elapsed),
        median_published_bytes: median(&mut bytes),
    }
}

fn median(values: &mut [u64]) -> u64 {
    values.sort_unstable();
    values[values.len() / 2]
}

fn percent_reduction(before: u64, after: u64) -> f64 {
    if before == 0 {
        return 0.0;
    }
    (before.saturating_sub(after) as f64 / before as f64) * 100.0
}

fn elapsed_nanos(started: Instant) -> u64 {
    started.elapsed().as_nanos().try_into().unwrap_or(u64::MAX)
}

fn elapsed_micros(started: Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}
