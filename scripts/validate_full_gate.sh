#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/validate_full_gate.sh [--list] [--msrv] [--release-tag TAG]

Runs the canonical repository gate. The default mode verifies repository policy,
formatting, locked workspace check/build, strict Clippy, all tests and doctests,
strict rustdoc, cargo-deny policy, and Cargo package assembly.

Pass --msrv to run the policy checks and locked workspace check intended for the
exact minimum supported Rust toolchain.
Pass --release-tag to additionally verify the tag, Cargo version, npm package
versions, and release-note filename before any release work.
Pass --list to print the selected commands without executing them.
USAGE
}

list_only=false
msrv_only=false
release_tag=""

while (($#)); do
  case "$1" in
    --list)
      list_only=true
      shift
      ;;
    --msrv)
      msrv_only=true
      shift
      ;;
    --release-tag)
      [[ $# -ge 2 ]] || {
        echo "--release-tag requires a value" >&2
        exit 2
      }
      release_tag="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "$msrv_only" == true && -n "$release_tag" ]]; then
  echo "--msrv and --release-tag cannot be combined" >&2
  exit 2
fi

run_cmd() {
  printf '+'
  printf ' %q' "$@"
  printf '\n'
  if [[ "$list_only" == false ]]; then
    "$@"
  fi
}

run_cmd scripts/verify_workspace_policy.sh
run_cmd python3 scripts/check_architecture.py
run_cmd python3 -m unittest scripts.tests.test_check_architecture
run_cmd python3 scripts/check_instruction_claims.py
run_cmd python3 scripts/check_rust_hazards.py
run_cmd python3 scripts/check_test_harness.py
run_cmd python3 scripts/verify_ci_policy.py
run_cmd python3 scripts/verify_tooling.py
run_cmd python3 scripts/verify_readme_stats.py --check
run_cmd python3 -m unittest discover -s scripts/tests -p 'test_*.py'

if [[ -n "$release_tag" ]]; then
  run_cmd python3 scripts/verify_release_version.py --root . --tag "$release_tag"
fi

if [[ "$msrv_only" == true ]]; then
  run_cmd cargo check --workspace --all-targets --all-features --locked
  exit 0
fi

# `cargo fmt` does not resolve dependencies and does not accept `--locked`.
run_cmd cargo fmt --all -- --check
run_cmd cargo check --workspace --all-targets --all-features --locked
run_cmd cargo build --workspace --all-targets --all-features --locked
run_cmd cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
run_cmd cargo test --workspace --all-targets --all-features --locked
run_cmd cargo test --workspace --doc --all-features --locked
run_cmd env RUSTDOCFLAGS="-D warnings -D rustdoc::broken_intra_doc_links" \
  cargo doc --workspace --all-features --no-deps --locked
run_cmd cargo deny --locked check
run_cmd cargo package --workspace --all-features --locked --allow-dirty
