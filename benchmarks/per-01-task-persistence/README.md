# PER-01 daemon task-persistence owner

Date: 2026-07-29

This release-mode experiment validates replacing a full task/watch registry
checkpoint for every event while holding the daemon state mutex with one
persistence owner, durable event-log sequencing, a replaceable pending
snapshot, and a debounced checkpoint.

## Decision gate

Keep the owned path only when the deterministic fixture:

1. recovers the same task registry and event count through both paths;
2. reduces median daemon-state lock hold time per event;
3. reduces bytes published for the measured events;
4. writes fewer full-registry checkpoints than measured events; and
5. passes startup reconciliation, corruption, concurrent ordering, lock-split,
   retained-failure, bounded-shutdown, and single-owner architecture tests.

The captured run passes every condition.

## Reproduce

```text
cargo test --release --locked -p packet28d --bin packet28d \
  benchmark_task_persistence_owner -- --ignored --nocapture
python3 benchmarks/per-01-task-persistence/verify.py
```

Environment:

- Darwin 24.6.0 arm64
- rustc 1.93.0 (254b59607 2026-01-19)
- cargo 1.93.0
- implementation base `14e1f4eadea91961c46a5fecb62e5cbc96778040`

The result artifact records checksums for every measured product and benchmark
source. Those file identities, rather than the pre-implementation base commit,
are the mechanically enforced provenance after replaying the atomic change
onto a newer integration parent.

## Workload and evidence boundary

Each of three repeats seeds exactly 300 tasks. The pretty-encoded registry is
1,848,325 bytes, within 132 bytes of the audit's volatile 1,848,193-byte live
observation. After one setup event, each path publishes 32 measured events.

`legacy_full_checkpoint_under_lock` is a controlled current-source model of the
confirmed former operation: hold one state mutex while appending the event,
serializing and atomically saving the watch registry, and serializing and
atomically saving the complete task registry. It is not a historical binary.

`owned_coalesced_checkpoint_after_lock` exercises the product event and
persistence-owner path. The authenticated event append runs on the owner
without the daemon state mutex. Each event updates the in-memory high-water and
replaces the pending immutable snapshot; one final request barrier waits for
the coalesced checkpoint. Published bytes combine product telemetry for event
frames with actual task/watch checkpoint file lengths. Setup writes are
excluded from both measurements.

The benchmark establishes parity and local write-amplification/lock-boundary
behavior. It does not simulate power loss or claim timing portability across
filesystems. Crash correctness is instead guarded by authenticated append-tail,
startup reconciliation, torn-tail, lease, and failure-path tests.

## Results

| Path | Median event state-lock hold | Median elapsed, 32 events | Published bytes | Full checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Full checkpoint under lock | 69,370,084 ns | 2,229,331 µs | 59,150,656 | 32 |
| Owned/coalesced after lock | 129,166 ns | 525,906 µs | 1,852,233 | 1 |

- Median daemon-state lock-hold reduction: **99.814%**
- Published-byte reduction: **96.869%**
- Wall-clock improvement on the capture host: **4.24×**
- Recovered events on both paths: **33**
