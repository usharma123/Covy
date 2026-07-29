set dotenv-load := false
set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

fmt:
    cargo fmt --all -- --check

check:
    cargo check --workspace --all-targets --all-features --locked

build:
    cargo build --workspace --all-targets --all-features --locked

lint:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

test:
    cargo test --workspace --all-targets --all-features --locked

doctest:
    cargo test --workspace --doc --all-features --locked

docs:
    env RUSTDOCFLAGS="-D warnings -D rustdoc::broken_intra_doc_links" cargo doc --workspace --all-features --no-deps --locked

deny:
    cargo deny --locked check

fast:
    scripts/validate_refactor_batch.sh

ci:
    scripts/validate_full_gate.sh

msrv:
    rustup run 1.88.0 scripts/validate_full_gate.sh --msrv

release-check tag:
    scripts/validate_full_gate.sh --release-tag "{{tag}}"

package:
    cargo package --workspace --all-features --locked --allow-dirty
