# Contributing to Packet28

Packet28 accepts focused fixes and improvements that preserve its local-first,
bounded-context design. Start a discussion before a broad feature, dependency,
wire-format, or storage change; those decisions affect several compatibility
and recovery boundaries.

## Before you begin

- Search existing issues and documentation for the behavior you plan to change.
- Keep one change centered on one invariant. Do not mix opportunistic cleanup
  with a behavior or architecture patch.
- Preserve behavior unless the current behavior is demonstrated to be
  defective.
- Add or update tests with the change. A mechanically checked invariant is
  acceptable where a runtime test cannot express the property.
- Never weaken a lint, safety check, corruption check, or test to make a patch
  pass.

For non-trivial work, describe the problem, the narrow interface you intend to
change, compatibility impact, failure paths, and how you will measure success.

## Development setup

Packet28 uses the toolchain pinned in `rust-toolchain.toml`; its declared MSRV
is Rust 1.88.0.

```bash
git clone https://github.com/usharma123/Packet28.git
cd Packet28
rustup show
cargo check --workspace --all-targets --all-features --locked
```

Useful optional tools:

- `just` for the repository task aliases;
- Python 3 for policy, architecture, benchmark, and release scripts;
- the checksum-pinned `cargo-deny` 0.20.2 installed by
  `scripts/install_cargo_deny.sh`.

Use the committed `Cargo.lock`. Do not regenerate dependency state casually,
and do not omit `--locked` from reproducibility or release checks.

## Repository shape

The codebase is organized around deep ownership boundaries:

- contract crates own wire and storage invariants;
- reducer crates own artifact-specific transformations;
- kernel crates own composition, budget, cache, scheduling, and policy;
- daemon crates own lifecycle, authenticated persistence, transport, and
  orchestration;
- CLI crates own argument parsing and presentation.

Read [docs/architecture.md](docs/architecture.md) before moving code between
crates. `scripts/check_architecture.py` and metadata tests enforce dependency
direction and public-surface constraints.

Prefer a small deep interface over a new cross-cutting helper, re-export hub, or
switchboard. New crates and public exports require a clear ownership reason.

## Make a change

1. Reproduce or measure the current behavior.
2. Add the regression, failure-path, property, architecture, or benchmark
   coverage appropriate to the claim.
3. Implement the smallest coherent change.
4. Run the focused package tests and strict package Clippy.
5. Run the fast repository gate.
6. Commit the code, tests, and directly coupled documentation together.

Typical focused commands:

```bash
cargo fmt --all -- --check
cargo test -p <package> --all-features --locked
cargo clippy -p <package> --all-targets --all-features --locked -- -D warnings
scripts/validate_refactor_batch.sh
```

Do not use sleeps to stabilize concurrent tests. Prefer barriers, channels,
drainable workers, explicit deadlines, and observable lifecycle state.

## Change-specific requirements

### Public APIs and wire types

- Use typed library errors and retain their source chains.
- Keep contextual `anyhow` errors at executable edges.
- Document public modules, errors, safety contracts, and compatibility behavior.
- Add JSON/snapshot/compile tests for protocol changes.
- Treat the `0.2.x` compatibility facades as frozen unless the change includes a
  migration plan.

### Daemon lifecycle and persistence

- Identify the single owner for startup, shutdown, joining, cancellation, and
  durable mutation.
- Test startup failure, partial write, corruption, retry, cancellation,
  restart, and concurrent authority changes.
- Keep filesystem authority capability-relative and no-follow.
- Preserve causal ordering between registry WAL revisions and task events.
- Never turn malformed or unauthenticated state into an empty successful load.

Read [docs/daemon-runtime.md](docs/daemon-runtime.md) and
[docs/task-store-retention.md](docs/task-store-retention.md).

### Unsafe Rust and platform code

- Keep unsafe code at narrow operating-system or FFI boundaries.
- Add a `SAFETY:` justification for every unsafe block.
- Document `# Safety` for every public unsafe function.
- Test supported platforms and make unsupported behavior explicit.

The enforced policy is in
[docs/rust-safety-and-panic-policy.md](docs/rust-safety-and-panic-policy.md).

### Performance changes

- Revalidate the audit or performance claim against current source.
- Record a behavior-equivalent baseline and selected path.
- Keep fixture, toolchain, host, source SHA, command, iterations, and raw result.
- Distinguish microbenchmark evidence from end-to-end product claims.
- Feature-gate architectural experiments until measurements support adoption.

Do not present provider-cache cost, placement, or instruction adherence as
proven by a local hash/hit-rate experiment.

### Dependencies

- Add direct dependencies through `[workspace.dependencies]`.
- Make every member inherit the workspace source and lint policy.
- Update `Cargo.lock` and `Cargo.direct-minimal.lock` when the graph changes.
- Run `python3 scripts/check_direct_dependencies.py` and
  `python3 scripts/validate_direct_minimum.py`.
- Explain any new runtime, native, or supply-chain surface.

### Documentation

- Update the nearest authoritative document, not only the README.
- Keep commands executable and paths relative to the repository root.
- Label historical plans, audit snapshots, and benchmark captures as evidence;
  do not rewrite them as current contracts.
- Run `python3 scripts/verify_readme_stats.py --check` when Rust files or
  workspace membership change.

## Canonical validation

Before requesting review, run:

```bash
scripts/validate_full_gate.sh
```

The command lists and runs the same core checks used by CI. It covers workspace
policy, direct dependencies, architecture, audit-ledger integrity, Rust
hazards, test-harness boundaries, runtime-starvation evidence, formatting,
check/build, strict Clippy, all-feature tests, doctests, rustdoc, `cargo-deny`,
release package verification, and Cargo packaging.

Also run the exact MSRV lane for dependency, language, or build-system changes:

```bash
rustup run 1.88.0 scripts/validate_full_gate.sh --msrv
```

For a release candidate:

```bash
scripts/validate_full_gate.sh --release-tag vX.Y.Z
```

If a platform or external provider cannot be tested locally, state exactly what
was not run and preserve the evidence boundary.

## Commits and review

Use small, descriptive commits such as:

- `fix(storage): reject changed registry authority`
- `refactor(kernel): isolate built-in reducer composition`
- `test(protocol): preserve legacy status wire shape`
- `docs(operations): document retention recovery`
- `ci: enforce locked release graph`

Each commit should build, contain its own tests or invariant, and be
independently reviewable. Do not combine the entire implementation into a
catch-all commit or erase useful atomic history.

In a review request, include:

- the invariant or defect;
- the chosen boundary and rejected alternatives;
- compatibility and migration notes;
- exact validation commands and results;
- benchmark evidence for performance claims;
- platform or external-service gaps.

## Security and data safety

Do not include real task stores, access tokens, provider credentials, private
prompts, or user repositories in fixtures. Use synthetic workspace state.

Retention, migration, and recovery changes must fail safely. Never make tests
green by deleting user state or bypassing authentication, lifecycle leases,
hooks, lints, or release policy.
