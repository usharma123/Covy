# PER-01 daemon task-persistence owner

Date: 2026-07-30

This release-mode experiment validates replacing a full task/watch registry
checkpoint for every event while holding the daemon state mutex with one
persistence owner, revisioned keyed-delta WAL, durable event-log sequencing,
and debounced full checkpoints.

## Decision gate

Keep the owned path only when the deterministic fixture:

1. recovers the same task registry and event count through both paths;
2. reduces median daemon-state lock hold time per event;
3. reduces bytes published for the measured events;
4. keeps measured publication bytes independent of total registry cardinality;
5. writes fewer full-registry checkpoints than measured events; and
6. passes startup reconciliation, corruption, concurrent ordering, lock-split,
   retained-failure, bounded-shutdown, and single-owner architecture tests.

The captured run passes every condition.

## Reproduce

```text
cargo test --release --locked -p packet28d --lib \
  benchmark_task_persistence_owner -- --ignored --nocapture
python3 benchmarks/per-01-task-persistence/verify.py
```

Environment:

- Darwin 24.6.0 arm64
- rustc 1.93.0 (254b59607 2026-01-19)
- cargo 1.93.0
- exact integration base `48024d28a2a6aa3a11a406230a61fcfb0af66356`
- measured implementation head `869581bf69eb0610285810fe43c425036d8c535d`

The result artifact records checksums for every measured product and benchmark
source. Those file identities are the mechanically enforced provenance. The
integration base and implementation head identify the exact-base rehearsal;
the hashes continue to reject later source drift even after documentation
commits change repository HEAD.

## Workload and evidence boundary

Each of three repeats seeds exactly 300 tasks. The pretty-encoded registry is
1,848,325 bytes, within 132 bytes of the audit's volatile 1,848,193-byte live
observation. After one setup event, each path publishes 32 measured events.

`legacy_full_checkpoint_under_lock` is a controlled current-source model of the
confirmed former operation: hold one state mutex while appending the event,
serializing and atomically saving the watch registry, and serializing and
atomically saving the complete task registry. It is not a historical binary.

`owned_wal_with_coalesced_checkpoint_after_lock` exercises the product event
and persistence-owner path. While holding the daemon state mutex, callers stage
only changed task/watch records and receive an ordered revision. The owner
coalesces same-key mutations, appends and synchronizes one checksummed WAL batch
outside that mutex, then appends the causally ordered event. Request barriers
wait for the requested WAL revision; full paired checkpoints remain debounced.
Published bytes combine event-frame, WAL, and any measured checkpoint telemetry.
Setup and shutdown checkpoint writes are excluded from both measurements.

The owned path is repeated with one task (6,186-byte fixture) and 300 tasks
(1,848,325-byte fixture), using identical target-record padding. Both publish
193,096 measured bytes, a large/small ratio of 1.0. This is the decision evidence
that steady-state event persistence scales with changed records rather than
total registry cardinality.

The benchmark establishes parity and local write-amplification/lock-boundary
behavior. It does not simulate power loss or claim timing portability across
filesystems. Crash correctness is instead guarded by authenticated append-tail,
startup reconciliation, torn-tail, lease, and failure-path tests.

## Results

| Path | Median event state-lock hold | Median elapsed, 32 events | Published bytes | Full checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Full checkpoint under lock | 87,428,083 ns | 2,801,016 µs | 59,150,656 | 32 |
| Owned keyed WAL after lock | 2,541 ns | 652,998 µs | 193,096 | 0 |

- Median daemon-state lock-hold reduction: **99.997%**
- Published-byte reduction: **99.674%**
- Wall-clock improvement on the capture host: **4.29×**
- Owned published-byte ratio, 300 tasks / 1 task: **1.00×**
- Recovered events on both paths: **33**
