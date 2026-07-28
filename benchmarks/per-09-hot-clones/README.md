# PER-09 measured allocation cleanup

This experiment revalidates the audit's broad clone/allocation warning against
the committed tree at `d1da15d9caf2b0662df06cbbe36de48c3f98f9fe`.
It deliberately separates mechanically actionable hot work from cold ownership
cleanup that has no measured product value.

## Current-source lint inventory

The focused all-target/all-feature Clippy run reported:

| Focused lint | Emitted diagnostics |
|---|---:|
| `redundant_clone` | 65 |
| `needless_collect` | 5 |
| `trivially_copy_pass_by_ref` | 4 |
| `large_types_passed_by_value` | 0 |

Counts include duplicate lib/test diagnostics. The large-by-value claim is not
reproducible with the current compiler and default lint threshold. The four
copy-by-reference diagnostics are one-byte `WcMode` parameters, not the
48–88-byte query/budget structs described by the audit, so changing those
interfaces was rejected.

The selected changes remove the three production `needless_collect` sites
identified by the audit and the largest verified clone: the complete broker
section vector was duplicated immediately before a consuming budget-prune
operation. Focused module lints keep those sites from regressing. Other
`redundant_clone` findings are ownership cleanups in cold CLI/test/error paths;
they are not represented as performance wins without measurements.

## Release benchmark

The ignored `packet28d` benchmark exercises the real
`prune_sections_for_budget` function. Each iteration constructs 32 broker
sections with 32 KiB bodies outside the timed region. The baseline clones that
1 MiB vector before calling the production function; the selected path moves
the owned vector directly. The benchmark first asserts output parity.

Run:

```sh
cargo test -p packet28d --release --locked \
  benchmark_budget_pruning_clone_elision -- --ignored --nocapture
```

Across 128 operations on the recorded host:

| Path | Aggregate time |
|---|---:|
| audited clone-before-prune baseline | 18,219,921 ns |
| move-owned-vector implementation | 9,981,125 ns |

The isolated operation improved by 45.2% (1.83×). This is a synthetic
allocation benchmark, not an end-to-end latency claim; it establishes that the
removed clone is material at the scale where broker section bodies are large.

Raw values and environment data are in `result.json`, `lint-summary.json`, and
`metadata.json`.
