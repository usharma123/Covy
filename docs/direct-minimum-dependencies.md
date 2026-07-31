# Direct-minimum dependency graph

Packet28 keeps two Cargo lockfiles for two different invariants:

- `Cargo.lock` is the reviewed production, CI, and release graph.
- `Cargo.direct-minimal.lock` is the smallest declared direct-dependency graph
  that compiles every workspace target and feature.

The direct-minimum graph is generated with Cargo's real
`-Z direct-minimal-versions` resolver using the exact nightly recorded in
`direct-minimum.toml`. The same file records a digest of every workspace
manifest. Any manifest edit therefore makes the canonical gate fail until the
alternate graph is refreshed.

To check the committed graph:

```bash
python3 scripts/validate_direct_minimum.py
```

To regenerate it after intentionally changing a dependency requirement:

```bash
python3 scripts/validate_direct_minimum.py --refresh
```

Add `--offline` when the local Cargo cache and index are complete. Refreshes
run in a temporary workspace and never replace the production `Cargo.lock`.
The validator then copies the alternate graph into that workspace and runs:

```bash
cargo check --workspace --all-targets --all-features --locked
```

## Lower bounds and compatibility pins

A lower bound must name the oldest dependency release whose API Packet28 uses
and whose feature set resolves. Do not raise it only to silence the check:
capture the missing API or resolver incompatibility in the commit and keep the
normal production graph unchanged.

`direct-minimum.toml` may contain a narrowly documented transitive pin when an
upstream semver-compatible release cannot run with the direct-minimum parent.
Pins are applied only after Cargo resolves the direct minima. They are not
allowed to mask a direct dependency whose declared lower bound is inaccurate.
