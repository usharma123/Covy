# PER-06 shared repository scan experiment

This experiment revalidates the remaining experiment-gated part of PER-06:
the map and regex full-index builders independently discover and read
repository content. It compares the current two-pass ownership model with a
bounded prototype that walks the union once, reads each requested file once,
and lends one raw buffer to both modeled consumers before dropping it.

The experiment does not claim a production or cold-I/O speedup from page-cache
effects. The deterministic evidence is the explicit walk, read, byte, and
allocator telemetry. Timings are secondary, machine-local, steady-state
observations.

## Reproduction

The harness is a standalone workspace so it cannot mutate the repository
`Cargo.lock`. Its dependency versions and transitive graph are pinned by the
lockfile in this directory.

```text
cargo test --manifest-path benchmarks/per-06-shared-scan/Cargo.toml --locked --offline
cargo clippy --manifest-path benchmarks/per-06-shared-scan/Cargo.toml --all-targets --locked --offline -- -D warnings
cargo run --manifest-path benchmarks/per-06-shared-scan/Cargo.toml --locked --offline --release -- --iterations 9 --output benchmarks/per-06-shared-scan/result.json
```

The feature-gated production coordinator has a separate release-only binary:

```text
cargo test --manifest-path benchmarks/per-06-shared-scan/Cargo.toml --locked --offline --bin production
cargo clippy --manifest-path benchmarks/per-06-shared-scan/Cargo.toml --locked --offline --bin production -- -D warnings
rustup run 1.88.0 cargo check --manifest-path benchmarks/per-06-shared-scan/Cargo.toml --locked --offline --bin production
cargo run --manifest-path benchmarks/per-06-shared-scan/Cargo.toml --locked --offline --release --bin production -- --iterations 6 --output benchmarks/per-06-shared-scan/production-result.json
```

The production run requires an even count of at least six measured pairs. It
warms each strategy on throwaway roots, alternates AB/BA execution order, and
reports the median of the paired latency deltas. Each measured pair is dropped
and its root removed before the next pair, so immutable runtimes and mapped
index files do not accumulate across samples.

The separate arm is deliberately an instrumented counterfactual: it reproduces
the two standalone discovery policies while holding the feature-gated engine
builder and publication mechanics constant. Before timing, an untimed build
through the literal default `rebuild_repo_index_runtime` and
`rebuild_full_index` entry points must match that counterfactual. Every measured
pair then proves map snapshot, regex artifact, query-result, ignore policy,
malformed-input, symlink, size-boundary, and buffer-residency invariants.

Production I/O counters cover repository inputs only. They exclude index
artifact, manifest, scan-cache, filesystem cache, and physical-kernel I/O.
Host, toolchain, clean-tree state, source-file digests, raw pair times, and
execution order are recorded with the result.

The release run uses two independently materialized, byte-identical roots for
each fixture. It performs one unmeasured warmup per strategy, then alternates
strategy order across nine iterations. Both strategies execute the same
per-consumer BLAKE3 work, so the prototype removes only duplicate discovery,
read, and buffer-allocation work. Consumer counts, logical bytes, and digests
must match on every iteration.

The counting allocator reports every successful Rust allocation,
reallocation, and deallocation within the complete scan window, plus requested
and deallocated bytes. The I/O counters report application-level walk passes,
walker entries, explicit metadata calls, successful `fs::read` calls, and
bytes returned by those reads. They are not kernel syscall or peak-RSS
counters.

## Fixtures

- `small-files`: 527 written files and 2,752,586 bytes, dominated by 4–8 KiB
  Rust source, tests, Markdown, and binary files.
- `large-files`: 111 written files and 79,741,002 bytes, including 256 KiB and
  512 KiB source, 1 MiB text, binary files, and source/text files above the
  regex index's 2 MiB limit.

Both fixtures also cover empty source, invalid UTF-8, UTF-8 containing NUL,
test paths, ignored state/vendor paths, and oversize boundaries. Three harness
tests assert output parity, boundary parity, deterministic materialization,
one-pass ownership, and reduced reads/bytes.

## Recorded release results

### Bounded prototype

The complete nine raw iterations, output digests, method metadata, toolchain,
host, and source identity are preserved in `result.json`.

| Metric | Small separate | Small shared | Change | Large separate | Large shared | Change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Walk passes | 2 | 1 | -1 | 2 | 1 | -1 |
| Walker entries | 1,071 | 534 | -537 | 239 | 118 | -121 |
| Metadata calls | 928 | 525 | -403 | 178 | 109 | -69 |
| Successful reads | 928 | 525 | -403 | 166 | 103 | -63 |
| Bytes read | 4,522,099 | 2,719,818 | -1,802,281 | 82,854,003 | 63,979,594 | -18,874,409 |
| Allocation calls | 7,351 | 3,752 | -48.95% | 2,591 | 1,331 | -48.62% |
| Requested bytes | 5,624,483 | 3,233,255 | -42.51% | 83,324,045 | 64,205,049 | -22.94% |
| Median elapsed | 37,792,959 ns | 23,777,750 ns | -37.08% | 64,918,833 ns | 58,171,250 ns | -10.39% |

Allocation calls and requested bytes were exact and stable across all nine
iterations for each strategy/fixture. Application-level I/O counters were also
stable. The shared prototype retained at most one content buffer; its largest
buffer was 16,384 bytes in the small fixture and 2,621,440 bytes in the large
fixture, matching a currently required map-only oversize source file.

### Feature-gated production coordinator

`production-result.json` preserves the six balanced release pairs from a clean
checkout of `c65a5dcbfdeaa79a115f7e8361aada7bbba1d97a`. The run used Rust
1.93.0 on an Apple M4 Pro host with 24 GB of memory and 14 available hardware
threads. Before measurement, the literal default entry points matched the
instrumented two-pass counterfactual. Map snapshots, regex
documents/postings/lookup artifacts, query results, fixture-policy oracles,
and repository-input counters then remained stable and equal where required
for every measured pair.

| Metric | Small separate | Small shared | Change | Large separate | Large shared | Change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Walk passes | 2 | 1 | -1 | 2 | 1 | -1 |
| Walker entries | 836 | 415 | -421 | 198 | 96 | -102 |
| Classification metadata queries | 836 | 2 | -834 | 198 | 2 | -196 |
| Content metadata calls | 734 | 408 | -326 | 148 | 89 | -59 |
| Successful reads | 734 | 408 | -326 | 140 | 85 | -55 |
| Bytes read | 3,612,932 | 2,134,171 | -1,478,761 | 71,827,716 | 53,739,675 | -18,088,041 |
| Peak retained content buffer | 8,192 B | 8,192 B | 0 | 2,621,440 B | 2,621,440 B | 0 |
| Raw-arm median elapsed | 844,991,854 ns | 829,551,416 ns | secondary | 11,662,904,750 ns | 11,735,124,396 ns | secondary |
| Median paired elapsed delta | — | — | -0.88% | — | — | +0.94% |

The production coordinator therefore removes one repository walk and
deterministically reduces successful repository reads and input bytes without
increasing the largest live raw-content buffer. The paired timing result is
mixed: the small-file fixture favored sharing slightly, while the large-file
fixture slightly favored the separate counterfactual. These are
recently-written-cache, machine-local observations and are not treated as a
cold-I/O or default-enablement result.

## Evidence boundary and decision

Accepted: the experiment justifies a non-default, feature-gated production
parity implementation, and the production harness verifies that implementation
against the literal default builders on controlled fixtures. The conclusion
rests on exact output and policy parity plus deterministic repository-input
reductions on both fixture shapes, not on warm-cache latency alone.

The smallest sound production seam is:

1. one low-level discovery plan yielding normalized path plus metadata;
2. map and regex builders independently declare interest before a read;
3. one owned file buffer is borrowed by both interested builders and dropped;
4. each builder retains its own filtering, cache, derived-data, error, progress,
   generation, and publication semantics;
5. incremental changed-path updates remain direct because they do not perform
   duplicate repository walks.

The evidence does not justify enabling the path by default. Keep
`shared-repository-scan` non-default until the same production parity harness
passes on Linux and opt-in daemon field telemetry demonstrates a stable
latency/resource benefit on real repositories. The checked-in result covers
one macOS filesystem/host, controlled fixture policies, production tree-sitter
and regex construction, publication, and recently written roots; it does not
claim parity for every filesystem or a physical-kernel-I/O speedup.
