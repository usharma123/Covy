#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
toolchain="${PACKET28_FUZZ_TOOLCHAIN:-nightly}"
runs="${PACKET28_FUZZ_RUNS:-64}"
seed="${PACKET28_FUZZ_SEED:-280728}"

if [[ ! "$runs" =~ ^[1-9][0-9]*$ ]]; then
  echo "PACKET28_FUZZ_RUNS must be a positive integer" >&2
  exit 2
fi
if [[ ! "$seed" =~ ^[0-9]+$ ]]; then
  echo "PACKET28_FUZZ_SEED must be a non-negative integer" >&2
  exit 2
fi
if ! nightly_rustc="$(rustup which --toolchain "$toolchain" rustc 2>/dev/null)"; then
  echo "Rust toolchain '${toolchain}' is unavailable." >&2
  echo "Install it with: rustup toolchain install nightly" >&2
  exit 2
fi
if ! RUSTC="$nightly_rustc" cargo fuzz --version >/dev/null 2>&1; then
  echo "cargo-fuzz is unavailable for toolchain '${toolchain}'." >&2
  echo "Install it with: cargo install cargo-fuzz --locked" >&2
  exit 2
fi

scratch_root="$(mktemp -d "${TMPDIR:-/tmp}/packet28-fuzz-smoke.XXXXXX")"
cleanup() {
  rm -rf -- "$scratch_root"
}
trap cleanup EXIT

targets=(daemon_frame search_index_bytes lcov_parser)
max_lengths=(262144 65536 262144)

cd -- "$script_dir"
for index in "${!targets[@]}"; do
  target="${targets[$index]}"
  corpus="$scratch_root/$target"
  mkdir -p -- "$corpus"
  cp -R "$script_dir/corpus/$target/." "$corpus/"
  RUSTC="$nightly_rustc" cargo fuzz run "$target" "$corpus" -- \
    -runs="$runs" \
    -seed="$seed" \
    -max_len="${max_lengths[$index]}" \
    -timeout=5 \
    -rss_limit_mb=512 \
    -print_final_stats=1
done
