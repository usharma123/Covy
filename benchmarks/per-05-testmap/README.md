# PER-05 sparse test-map benchmark

This deterministic release benchmark models 2,000 tests, 1,000 source files,
24,000 non-empty test/file coverage cells, and a 40-file diff containing
10,240 changed lines. Run it from the repository root:

```text
cargo run --release -p testy-core --example testmap_scale --locked -- --iterations 8
```

The fixture and greedy plan are held constant by the plan signature
`dd7c2c02c56fa050d879ebd12b71e367982b96dac192187c2a0ed61028fe6171`.
The v3 sample plans against the serialized-and-reloaded artifact; the historical
sample planned against the old in-memory dense artifact and did not record a
separate deserialize timer.
On the recorded arm64 machine, sparse v3 planning reduced the median from
654,174 µs to 10,091 µs (64.8×), serialization from 12,332 µs to 3,796 µs
(3.2×), and the persisted artifact from 17,613,076 bytes to 1,613,085 bytes
(10.9×). Synthetic fixture construction increased from 3,898 µs to 8,078 µs;
that number is reported rather than hidden and is not the production coverage
ingestion path.

The raw measurements are in `before.json` and `after.json`. Timings are
machine-local observations, not cross-platform guarantees.

## Format compatibility

The persisted v3 format has an explicit `P28TMAP` magic prefix and little-endian
schema version, stores only sorted non-empty bitmap cells, and rejects malformed
row counts, file indexes, ordering, empty cells, and header/payload version
disagreement. Fixed-width little-endian decoding also rejects trailing artifact
and bitmap bytes. Readers accept both historical v2 dense payloads and both
known v1 layouts. V2 payloads are migrated in memory to v3 sparse rows; v1
remains file-granularity only. New writes are always v3, so migration is
completed by the next normal write without an in-place rewrite step.
