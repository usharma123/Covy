use std::hint::black_box;
use std::time::Instant;

use context_memory_core::{CachePacket, NoopDeltaReuseHooks, PacketCache};
use serde_json::{json, Value};

const NOW: u64 = 10_000;
const ITERATIONS: usize = 100;
const REPEATS: usize = 5;

fn main() {
    let mut measurements = Vec::new();
    for entries in [128, 2_048, 8_192] {
        let mut cache = PacketCache::new();
        let mut lookups = Vec::new();
        for id in 0..entries {
            let lookup =
                cache.lookup_with_hooks("expiry.fixture", &json!(id), &mut NoopDeltaReuseHooks);
            cache.put_at_with_hooks(
                "expiry.fixture",
                &lookup,
                vec![CachePacket {
                    body: json!({"summary": "live cache entry"}),
                    ..CachePacket::default()
                }],
                Value::Null,
                NOW,
                &mut NoopDeltaReuseHooks,
            );
            lookups.push(lookup);
        }
        for ttl_secs in [0, 60] {
            let mut samples = Vec::new();
            for _ in 0..REPEATS {
                let started = Instant::now();
                for _ in 0..ITERATIONS {
                    // The persistent update path checks candidates before admission,
                    // then applies expiration after inserting the accepted entry.
                    assert!(black_box(&cache)
                        .expired_entry_keys_at(black_box(ttl_secs), black_box(NOW))
                        .is_empty());
                    assert!(black_box(&mut cache)
                        .evict_expired_entries_at(black_box(ttl_secs), black_box(NOW))
                        .is_empty());
                }
                samples.push(started.elapsed().as_nanos() / ITERATIONS as u128);
                assert_eq!(cache.len(), entries);
                for lookup in &lookups {
                    assert!(cache
                        .get_by_request("expiry.fixture", &lookup.input_hash)
                        .is_some());
                }
            }
            measurements.push(json!({
                "entries": entries,
                "ttl_secs": ttl_secs,
                "nanoseconds_per_expiration_pair": samples,
            }));
        }
    }
    println!(
        "{}",
        json!({"iterations": ITERATIONS, "repeats": REPEATS, "measurements": measurements})
    );
}
