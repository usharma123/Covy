# Cache expiration maintenance

Persistent updates check expired keys before admission and apply expiration after
insertion. Previously, applying expiration rebuilt the entire request index even
when it removed no entries. With TTL disabled, both expiration calls still scanned
the cache. These operations run while the persistence owner holds its live-cache
mutex.

The patch returns immediately for disabled expiration and rebuilds the request
index only after removals. Actual expiration keeps its existing lookup, recall,
counter, and persistence behavior. No durable format or public API changes.

The isolated library experiment uses live entries and measures only the expiration
candidate/apply pair. Setup and lookup parity checks are outside timing. Each cell
is the median of five repeats of 100 pairs, in microseconds.

| Entries | TTL seconds | Before | After |
| ---: | ---: | ---: | ---: |
| 128 | 0 | 41.735 | 0.003 |
| 128 | 60 | 43.668 | 0.188 |
| 2,048 | 0 | 773.261 | 0.004 |
| 2,048 | 60 | 817.351 | 3.422 |
| 8,192 | 0 | 3,907.140 | 0.004 |
| 8,192 | 60 | 3,709.169 | 12.817 |

The TTL-zero result is at timer and call-overhead scale. This experiment does not
measure total write latency, disk I/O, end-to-end context savings, or provider
prompt caching. Enabled expiration still scans entries; expiration that removes
entries still rebuilds the request index.

Run `cargo run -p context-memory-core --example cache_expiry_experiment --release
--locked` with the pinned toolchain. `metadata.json` records the source, host, and
fixture. `before.json` and `after.json` retain every sample. To reproduce the
baseline, use its recorded SHA with this example file.

The regression first failed on an unnecessary index rebuild. It now checks
disabled expiration, the exact TTL boundary, lookup and recall after removal,
eviction counters, and zero index rebuilds when no entries expire. The full
context-memory-core all-feature test suite also checks restart, WAL, concurrent
updates, and filesystem authority failure paths.
