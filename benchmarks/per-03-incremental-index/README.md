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
  cargo run --offline --release --locked -p mapy-core \
  --example per03_incremental_index
```

Environment:

- Darwin 24.6.0 arm64
- rustc 1.93.1 (01f6ddf75 2026-02-11)
- cargo 1.93.1 (083ac5135 2025-12-15)
- implementation commits `948aae6` (mapy), `832e40f` (regex),
  `bd9fe24` (writer ownership, integrity, and retention), and `b474315`
  (daemon adoption)

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
| Mapy whole snapshot | 9,062 | 3,490,797 | baseline | baseline |
| Mapy incremental generation | 3,145 | 5,027 | 65.29% | 99.86% |
| Regex full-overlay work-model | 261,103 | 225,693 | baseline | baseline |
| Regex incremental segment | 7,712 | 27,383 | 97.05% | 87.87% |

The three run medians were:

```text
mapy whole snapshot:       9062, 9512, 6315 µs
mapy incremental:          3245, 3145, 2383 µs
regex full-overlay model:  255386, 425949, 261103 µs
regex incremental:         5979, 9163, 7712 µs
```

The slowest observed incremental update remained faster than the fastest
corresponding whole-rebuild observation.

## BR-17 publication-fence revalidation

Date: 2026-07-30

The benchmark was rerun at baseline `43753c58` after the blind review found
that the incremental writer reloaded, hashed, and decoded the complete
published base and every segment while holding the publication lock. The
benchmark also used `saturating_sub`, so a regression was printed as a
`0.00%` reduction.

Commit `9fc911d8` replaces that reload with a generation/digest comparison
under the publication lock. The already authenticated in-memory generation is
retained; immutable artifact metadata is checked, and only an artifact whose
metadata changed is rehashed. Benchmark deltas are now signed
`(after - before) / before` values, where a positive time delta is a
regression.

The exact release command under [Reproduce](#reproduce) was run three times
before and after the change:

| Revision/path | Invocation medians (µs) | Median (µs) | Delta versus paired legacy |
| --- | --- | ---: | ---: |
| `43753c58` whole snapshot | 6,506; 5,251; 5,466 | 5,466 | baseline |
| `43753c58` incremental generation | 16,965; 15,732; 18,205 | 16,965 | +210.37% |
| `9fc911d8` whole snapshot | 4,471; 5,068; 4,751 | 4,751 | baseline |
| `9fc911d8` incremental generation | 2,073; 2,210; 2,314 | 2,210 | -53.48% |

The incremental invocation median improved from 16,965 to 2,210 µs
(-86.97%, or 7.68× faster). Each final run published 5,323 bytes versus
3,490,797 bytes for the whole-snapshot model and reported the same bounded
work:

```text
publication_metadata_bytes_decoded=315
repository_artifact_bytes_decoded=0
repository_artifacts_decoded=0
repository_artifact_bytes_hashed=0
repository_artifact_metadata_checks=6
changed_paths_considered=1
```

The focused invariant seeds four retained segments and asserts zero
repository-artifact decoding/hashing, five bounded metadata checks (base plus
four segments), and exactly one considered changed path.

## Compaction cost

The eighth segment publication compacts only live overlay entries and retains
the immutable base generation.

| Path | Median compaction (µs) | Published bytes |
| --- | ---: | ---: |
| Mapy | 10,848 | 328,522 |
| Regex | 307,572 | 222,954 |

Mapy compaction observations were 10,848, 25,525, and 5,705 µs. Regex
observations were 301,916, 348,383, and 307,572 µs. The regex compaction cost is
approximately one former full-overlay update, but occurs once per eight segment
publications; ordinary updates retain the measured incremental behavior.

## Daemon adoption

The packet28d regression benchmark exercises the production orchestration path:
a 256-file full build followed by five one-file updates through
`perform_incremental_index_update`. It asserts shared base ownership, records
all changed files beneath `.packet28/index`, and fails if the legacy aggregate
snapshot is written.

```text
CARGO_TARGET_DIR=/tmp/packet28-per03-daemon-final \
  cargo test --offline --locked -p packet28d \
  index::tests::daemon_incremental_publication_benchmark -- --nocapture
```

This is a debug-profile integration measurement, so elapsed time is diagnostic;
the artifact-size invariant is the decision evidence.

| Path | Median update (µs) | Published bytes | Reference bytes | Byte reduction |
| --- | ---: | ---: | ---: | ---: |
| Daemon incremental publication | 29,767 | 27,048 | 1,410,270 initial generation | 98.08% |

The three benchmark invocations reported 29,536, 44,033, and 29,767 µs. All
three published 27,048 bytes for the measured update and reported
`legacy_snapshot_written=false`.

## Integrity and ownership validation

The final measurements include BLAKE3 artifact digests, a repository-local
exclusive writer lock, generation compare-and-swap, and current-plus-previous
artifact retention. Regression tests cover stale concurrent writers,
structurally valid byte mutation, traversal attempts, bounded retention,
restart recovery, deletion tombstones, retained readers, and daemon clear.
These checks deliberately preserve the evidence boundary: pruning is
best-effort after publication, and the persistence protocol does not claim
power-loss durability.

## Interpretation

The result supports immutable generations with manifest-last publication,
bounded overlay segments, and reader-owned `Arc` layers. It does not establish
power-loss durability: publication uses a flushed temporary file followed by an
atomic rename, while recovery trusts only the current or explicitly retained
previous manifest and never promotes orphan artifacts.
