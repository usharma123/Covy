# Packet28 fuzz targets

This standalone workspace keeps fuzz-only dependencies and its lockfile out of
the root workspace and default test graph. It exercises three public,
untrusted-input seams:

- `daemon_frame`: length-prefixed daemon requests through
  `packet28_daemon_protocol::frame::read_frame`.
- `search_index_bytes`: persisted base lookup/postings bytes through
  `packet28_search_core::load_runtime`.
- `lcov_parser`: LCOV bytes through `covy_ingest::lcov::LcovIngestor`.

## Requirements

Install a nightly toolchain and `cargo-fuzz`:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz --locked
```

Build every target without running it:

```sh
cd fuzz
RUSTC="$(rustup which --toolchain nightly rustc)" cargo fuzz build
```

Run the deterministic bounded smoke suite from the repository root:

```sh
bash fuzz/smoke.sh
```

`PACKET28_FUZZ_RUNS` and `PACKET28_FUZZ_SEED` override the default 64 runs per
target and fixed seed. `PACKET28_FUZZ_TOOLCHAIN` selects another installed
nightly toolchain. The runner resolves that toolchain's `rustc` explicitly, so
it also works when `cargo` is not installed through the rustup proxy.

## Resource bounds and corpora

The daemon and LCOV targets reject inputs above 256 KiB. The daemon harness
also avoids declared in-limit frames above 256 KiB, while still exercising the
protocol's over-limit rejection path. The index target rejects inputs above
64 KiB, mutates only lookup/postings files, restores a small valid index before
every case, and uses one process-local temporary directory. These constraints
keep input-driven memory and filesystem work bounded during smoke runs.

Committed seed corpora cover valid requests/reports, truncated or oversized
frames, malformed JSON, near-valid index files, partial lookup rows, malformed
posting blocks, numeric boundaries, duplicate LCOV records, and wrong-format
input. The smoke runner copies seeds into a temporary writable corpus so local
runs never modify committed corpus files. Crashes are written under
`fuzz/artifacts/`, which is intentionally ignored.
