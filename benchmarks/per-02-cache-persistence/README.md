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
- source base `a7c21073c1f1b37fd00822a4ba5ab75ed879480e`

The result artifact records checksums for the measured context-memory source and
example. The source base identifies the committed lifecycle implementation used
for this capture; the per-file checksums remain the mechanically enforced
identity.

## Workload and evidence boundary

Each of three repeats seeds 512 distinct cache entries with 1 KiB payloads,
then publishes 64 additional distinct entries.

`legacy_full_checkpoint_under_lock` reproduces the confirmed former operation:
acquire the live cache mutex, mutate the cache, collect/serialize the complete
checkpoint and all indexes, write it, and release the mutex. This is a
controlled work model in the current source, not a checkout of a historical
binary.

`owned_delta_wal_after_lock` (the retained schema field name) exercises the
product `CachePersistence` path: prepare and JSON-encode the owned entry outside
the root-shared live cache mutex; then cross one owner-controlled commit
boundary that samples one timestamp under the mutex, reserves bounded
persistence capacity, moves the exact delta into the queue, and exposes the
live mutation only after acceptance. The low-level reservation and
publish-token sequence is not public. Rejected payloads and superseded raw or
encoded entries are destroyed after the live-cache guard is released. A
bounded flush then awaits the debounced WAL owner. Published bytes combine
actual WAL frame bytes and durable eight-byte coordination-generation writes
reported by product telemetry.

The legacy path counts both authenticated checkpoint payload copies (backup
and primary) written for every measured mutation. The owned path reports its
WAL payload and coordination bytes separately. Initial fixture checkpoints are
excluded from both measured write totals. Metadata-only filesystem operations
such as rename and truncation are not assigned synthetic byte counts.

The experiment measures local filesystem behavior, not power-loss behavior on
every filesystem. Crash boundaries are established mechanically by the
checksummed valid-prefix replay and checkpoint sequence-watermark tests.

## Results

| Path | Median write-lock hold | Median elapsed, 64 writes | Payload bytes | Coordination bytes | Published bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Full checkpoint under lock | 49,159,083 ns | 3,255,867 µs | 533,971,344 | 0 | 533,971,344 |
| Owned delta WAL after lock | 40,125 ns | 15,905 µs | 101,461 | 8 | 101,469 |

- Median cache-lock hold reduction: **99.918%**
- Published-byte reduction: **99.981%**
- Recovered entries on both paths: **576**

The timing values are machine-specific. The architectural conclusion rests on
the invariant that payload encoding occurs before the live cache mutex, while
WAL framing, `sync_data`, and checkpoint I/O occur in the owner after that mutex
is released. Telemetry and the deterministic byte comparison guard against
reintroducing full-envelope writes per mutation.
