use context_memory_core::{ContextStoreStats, RecallHit, RecallMode};
use suite_packet_core::memory;

#[test]
fn legacy_memory_paths_are_the_shared_wire_types() {
    let shared_hit = memory::RecallHit {
        cache_key: "cache-1".to_string(),
        ..memory::RecallHit::default()
    };
    let legacy_hit: RecallHit = shared_hit;
    let legacy_mode: RecallMode = memory::RecallMode::Conceptual;
    let legacy_stats: ContextStoreStats = memory::ContextStoreStats::default();

    assert_eq!(legacy_hit.cache_key, "cache-1");
    assert_eq!(
        serde_json::to_string(&legacy_mode).unwrap(),
        "\"conceptual\""
    );
    assert_eq!(legacy_stats.entries, 0);
}
