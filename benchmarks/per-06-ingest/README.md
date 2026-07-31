# PER-06 ingestion allocation experiment

This report measures the two ingestion allocation claims from the architecture audit before selecting an implementation:

- diagnostic path ingestion reads the complete file and copies it into a second complete buffer;
- LCOV `DA`, `BRDA`, and `FNDA` records materialize a temporary `Vec<&str>`.

The raw outputs are preserved in:

- `before-allocation.json` and `after-allocation.json`;
- `before-criterion.txt` and `after-criterion.txt`;
- `metadata.json`.

## Method

The allocation probe uses a counting wrapper around Rust's system allocator. It warms the parser and Rayon pool before enabling counters, then reports successful allocation calls, reallocation calls, total requested bytes, and elapsed time across ten release-mode iterations. Deallocations are deliberately not counted.

The Criterion suite measures:

- the public LCOV parser over 50,000 `DA` records;
- legacy `splitn(...).collect::<Vec<_>>()` against iterator field extraction;
- the public auto-detected diagnostics path against an otherwise equivalent single-buffer reference over an 8 MiB SARIF document;
- the existing small-format benchmarks as noise controls.

The synthetic SARIF has no results and one ignored 8 MiB string. This isolates whole-input movement from result-model allocations. Filesystem reads use the warm operating-system cache. Benchmarks were run on an Apple M4 Pro with 24 GiB RAM using Rust 1.93.1 and optimized profiles. See `metadata.json` for exact source and host metadata.

The baseline production files matched revision `803a8e9aa781328a351c72be83a41a67fed263ec`. Other remediation agents advanced the shared branch during the experiment; the after-state production diff is therefore additionally identified by the SHA-256 in `metadata.json`.

## Results

Allocation totals below are normalized to one operation.

| Workload | Before | After | Change |
| --- | ---: | ---: | ---: |
| LCOV allocation calls | 50,012 | 12 | -99.98% |
| LCOV reallocations | 22 | 22 | unchanged |
| LCOV requested bytes | 3,300,152 | 100,152 | -96.97% |
| LCOV Criterion time | 2,427,487 ns | 1,603,821 ns | -33.93% |
| Diagnostic allocation calls | 2 | 1 | -50.00% |
| Diagnostic requested bytes | 16,777,318 | 8,388,659 | -50.00% |
| Diagnostic probe time | 1,331,888 ns | 1,245,196 ns | -6.51% |
| Diagnostic Criterion time | 1,206,949 ns | 1,180,668 ns | -2.18% |

The baseline side-by-side diagnostic reference took 1,021,997 ns versus 1,206,949 ns for the copying path, a 15.3% difference. The after Criterion timing had higher system noise, but the allocation probe is deterministic: the public path now exactly matches the reference at one input-sized allocation per operation.

The isolated LCOV field benchmark remained stable across runs: iterator extraction took 709,080 ns before and 714,340 ns after, while the allocating reference took 1,575,131 ns and 1,578,186 ns respectively. This confirms that the full-parser improvement comes from selecting the measured strategy rather than benchmark drift.

## Decisions

Accepted:

- Parse path-loaded diagnostics directly from the buffer already used for format detection. Reader ingestion still buffers once because the SARIF parser requires a slice.
- Replace temporary LCOV field vectors with borrowed iterator extraction for `DA`, `BRDA`, and `FNDA`.
- Preserve legacy behavior for missing fields, invalid counts, integer overflow, extra commas, branch `-` values, and comma-containing function names through generated boundary parity tests.

No measured candidate was rejected: both targeted candidates reduced allocations and did not regress the target benchmark.

Deferred rather than implemented:

- A fully streaming SARIF redesign. The current parser borrows `RawValue` slices from the document and parallelizes per-run decoding. Once the redundant second buffer is removed, this experiment provides no evidence that replacing that ownership model is worth the broader behavioral and architectural change.
- Memory mapping. It adds platform and lifetime complexity and was not needed to remove the confirmed duplicate input buffer.

`suite-ingest` requires no implementation change because its diagnostics facade delegates to `covy-ingest`; its direct tests exercise the optimized path.

## Reproduction and validation

```text
cargo run --quiet --locked --release -p covy-ingest --example ingest_allocation_probe -- --iterations 10
cargo bench --locked -p covy-ingest --bench ingest_bench -- --output-format bencher
cargo test --locked -p covy-ingest
cargo test --locked -p suite-ingest
cargo clippy --locked -p covy-ingest -p suite-ingest --all-targets -- -D warnings
```

Criterion used its Plotters backend because gnuplot was unavailable.
