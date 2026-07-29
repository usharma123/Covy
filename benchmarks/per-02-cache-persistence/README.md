# PER-02 cache-persistence owner

Date: 2026-07-28

This release-mode experiment validates the replacement of full context-cache
checkpoint writes under the live cache mutex with a single persistence owner,
coalesced per-key dirty state, checksummed WAL deltas, and debounced
checkpoints.

## Decision gate

Keep the owned delta path only when the deterministic fixture:

1. recovers the same number of entries through each persistence path;
2. reduces median cache-mutex hold time per write;
3. reduces bytes published for the measured writes; and
4. passes the WAL torn-tail, checkpoint-watermark, concurrent writer,
   backpressure, bounded-flush, shutdown-ownership, and filesystem-failure
   regression tests.

The captured run passes all four conditions.

## Reproduce

```text
cargo run --offline --release --locked -p context-memory-core \
  --example cache_persistence_experiment
python3 benchmarks/per-02-cache-persistence/verify.py
```

Environment:

- Darwin 24.6.0 arm64
- rustc 1.93.1 (01f6ddf75 2026-02-11)
- cargo 1.93.1 (083ac5135 2025-12-15)
- source base `829ebe7042130da8f6b976cf400dd4d848928ca6`

The result artifact records checksums for the measured context-memory source and
example. The source base is informational because the measured implementation
was an uncommitted atomic remediation slice at capture time.

## Workload and evidence boundary

Each of three repeats seeds 512 distinct cache entries with 1 KiB payloads,
then publishes 64 additional distinct entries.

`legacy_full_checkpoint_under_lock` reproduces the confirmed former operation:
acquire the live cache mutex, mutate the cache, collect/serialize the complete
checkpoint and all indexes, write it, and release the mutex. This is a
controlled work model in the current source, not a checkout of a historical
binary.

`owned_delta_wal_after_lock` exercises the product `CachePersistence` path:
mutate under the live cache mutex, release it, mark the entry dirty, then use a
bounded flush to await the debounced WAL owner. Published bytes are actual WAL
frame bytes reported by product telemetry. Initial fixture checkpoints are
excluded from both measured write totals.

The experiment measures local filesystem behavior, not power-loss behavior on
every filesystem. Crash boundaries are established mechanically by the
checksummed valid-prefix replay and checkpoint sequence-watermark tests.

## Results

| Path | Median write-lock hold | Median elapsed, 64 writes | Published bytes |
| --- | ---: | ---: | ---: |
| Full checkpoint under lock | 20,131,584 ns | 1,344,839 µs | 266,982,600 |
| Owned delta WAL after lock | 39,459 ns | 10,838 µs | 101,461 |

- Median cache-lock hold reduction: **99.804%**
- Published-byte reduction: **99.962%**
- Recovered entries on both paths: **576**

The timing values are machine-specific. The architectural conclusion rests on
the invariant that encoding, `sync_data`, and checkpoint I/O occur in the owner
after the live cache mutex is released; telemetry and the deterministic byte
comparison guard against reintroducing full-envelope writes per mutation.
