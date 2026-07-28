# PER-03 incremental index publication

Date: 2026-07-28

This experiment validates the decision to replace whole-snapshot rewrites with
immutable base generations and incremental overlay segments in `mapy-core` and
`packet28-search-core`.

## Decision gate

Keep the incremental architecture only when the median single-path update:

1. preserves the existing search/index results (covered by the crate regression
   suites);
2. reduces bytes published per update; and
3. does not regress elapsed time on the deterministic release workload.

The repeated measurements below pass all three conditions. The implementation
therefore remains enabled. Compaction is measured separately because its cost is
intentionally amortized over eight segment publications.

## Reproduce

```text
CARGO_TARGET_DIR=/tmp/packet28-per03-bench-target \
  cargo run --offline --release -p mapy-core \
  --example per03_incremental_index
```

Environment:

- Darwin 24.6.0 arm64
- rustc 1.93.1 (01f6ddf75 2026-02-11)
- cargo 1.93.1 (083ac5135 2025-12-15)
- implementation commits `948aae6` (mapy) and `832e40f` (regex)

The example creates and removes isolated repositories under the system temporary
directory. Each reported duration is the median of five updates. The summary
table is the median of three independent example invocations.

## Workload and evidence boundary

The mapy fixture contains 1,024 deterministic Rust files. After seeding 96
changed paths, each measured update modifies one path. `mapy_legacy` executes
the former operation directly: clone the complete `RepoIndexSnapshot`, call
`update_repo_index`, serialize the complete snapshot, and rewrite the complete
artifact. `mapy_incremental` indexes and publishes only the supplied path.

The regex fixture contains 256 deterministic Rust files with 96 live overlay
paths. The former implementation rebuilt all live overlay documents for every
change. `regex_full_overlay_model` reproduces that work by supplying all 96 live
paths; it is a controlled work-model, not a checkout of the historical binary.
`regex_incremental` supplies only the changed path. Correctness parity with the
reducer, deletion/tombstone behavior, corruption recovery, retained-reader
ownership, and threshold compaction are enforced by the search-core tests.

Publication bytes are the sum of new or changed files after an update. Unchanged
immutable base and segment artifacts are deliberately excluded. Mapy's legacy
value is the complete encoded snapshot size.

## Results

| Path | Median update (µs) | Published bytes | Time reduction | Byte reduction |
| --- | ---: | ---: | ---: | ---: |
| Mapy whole snapshot | 5,743 | 3,490,797 | baseline | baseline |
| Mapy incremental generation | 1,615 | 4,483 | 71.88% | 99.87% |
| Regex full-overlay work-model | 301,148 | 223,752 | baseline | baseline |
| Regex incremental segment | 7,004 | 25,442 | 97.67% | 88.63% |

The three run medians were:

```text
mapy whole snapshot:       5514, 5743, 7112 µs
mapy incremental:          1498, 1615, 3839 µs
regex full-overlay model:  349695, 260960, 301148 µs
regex incremental:         48298, 6690, 7004 µs
```

The slowest observed incremental update remained faster than the fastest
corresponding whole-rebuild observation.

## Compaction cost

The eighth segment publication compacts only live overlay entries and retains
the immutable base generation.

| Path | Median compaction (µs) | Published bytes |
| --- | ---: | ---: |
| Mapy | 3,903 | 331,777 |
| Regex | 271,418 | 239,891 |

Mapy compaction observations were 3,903, 2,861, and 11,667 µs. Regex
observations were 370,097, 270,148, and 271,418 µs. The regex compaction cost is
approximately one former full-overlay update, but occurs once per eight segment
publications; ordinary updates retain the measured incremental behavior.

## Interpretation

The result supports immutable generations with manifest-last publication,
bounded overlay segments, and reader-owned `Arc` layers. It does not establish
power-loss durability: publication uses a flushed temporary file followed by an
atomic rename, while recovery trusts only the current or explicitly retained
previous manifest and never promotes orphan artifacts.
