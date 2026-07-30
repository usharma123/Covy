# ASY-04 runtime-starvation boundary

Date: 2026-07-30

This release-mode experiment measures how synchronous work affects a Tokio
timer when it runs directly on a single runtime worker and when the same work
is isolated through the daemon's bounded blocking pool.

## Decision gate

The bounded blocking architecture remains acceptable when:

1. the checked-in result is tied to the exact benchmark source;
2. both paths execute 32 identical iterations with a 1 ms timer and 10 ms
   blocking operation;
3. the blocking-pool p95 timer lateness is lower than direct synchronous work;
4. the blocking-pool p95 timer lateness is at most 25% of direct synchronous
   work; and
5. the runtime architecture guard and non-starvation regression tests pass.

The captured run passes every condition. The p95 timer lateness fell from
14,103 µs to 2,083 µs, an 85.23% reduction (6.77×).

## Reproduce

```text
cargo test -p packet28d --release \
  benchmark_runtime_timer_starvation_boundary --locked -- \
  --ignored --nocapture --test-threads=1
python3 benchmarks/asy-04-runtime-starvation/verify.py
python3 scripts/check_architecture.py
```

Environment:

- Darwin 24.6.0 arm64
- rustc 1.93.1 (01f6ddf75 2026-02-11)
- cargo 1.93.1 (083ac5135 2025-12-15)
- integration tree `cf9485939d22d64008e39623d0234bfeb0f8e455`

## Evidence boundary

This is a controlled current-source microbenchmark, not a production latency
claim. It proves that the fixture detects starvation and that the bounded
blocking path preserves timer progress on the capture host. The ordinary
multi-thread regression separately enforces a 250 ms upper bound while a
blocking operation is held. Persistence correctness and lock ownership remain
covered by their dedicated PER-01 and PER-02 experiments.

## Results

| Path | p50 lateness | p95 lateness | Maximum lateness |
| --- | ---: | ---: | ---: |
| Direct synchronous work | 14,012 µs | 14,103 µs | 14,192 µs |
| Bounded blocking pool | 2,030 µs | 2,083 µs | 2,089 µs |

- Iterations per path: **32**
- Timer delay: **1,000 µs**
- Blocking duration: **10,000 µs**
- p95 reduction: **85.230%**
- p95 improvement: **6.771×**
