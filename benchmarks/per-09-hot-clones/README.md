# PER-09 measured allocation cleanup

This experiment revalidates the audit's broad clone/allocation warning against
the committed tree at `eef4db1b2cfa25788c282c126782b731979a7686`.
It separates measurable hot-path work, mechanically safe ownership cleanup, and
collections that are required by concurrency or borrowing invariants.

## Current-source lint inventory

The focused all-target/all-feature Clippy run reported:

| Focused lint | Emitted diagnostics |
|---|---:|
| `redundant_clone` | 1 |
| `needless_collect` | 3 |
| `trivially_copy_pass_by_ref` | 4 |
| `large_types_passed_by_value` | 0 |

The inventory fell from 65 redundant clones and five needless collections. The
remaining clone is intentional: the state-filesystem regression must retain two
directory-lease values from the same cloned authority to prove that they
contend. The remaining collections are also mechanically required:

- scoped search workers must all be spawned before any worker joins, because
  the workers synchronize on a barrier;
- the concurrent index-writer test has the same spawn-before-join invariant;
- watch pruning must copy the keys before mutating the same `HashMap`.

The large-by-value claim is not reproducible with the current compiler and
default lint threshold. The four copy-by-reference diagnostics are one-byte
`WcMode` parameters, not the 48–88-byte query/budget structs described by the
audit, so changing those public interfaces was rejected.

The selected performance change removed the largest verified clone: the
complete broker section vector was duplicated immediately before a consuming
budget-prune operation. The later workspace cleanup removes only proven
last-use copies and does not claim an additional runtime improvement.

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
`metadata.json`. Strict Clippy and the full all-feature workspace test suite
passed against the cleanup commit.
