# PER-10 current workspace-index benchmark

This is a controlled, machine-local cold-build measurement of the current source snapshot. It preserves the March 2026 result as historical evidence and does not treat it as a directly comparable baseline.
A result is final-tree evidence only when its recorded HEAD and snapshot identity match the integrated source being handed off.

## Evidence boundary

| Evidence | Source | Result | Interpretation |
| --- | --- | ---: | --- |
| Historical | p28 `59e54fb` / packet28d `0.2.39` on 2026-03-31 | 10,375.754 ms | Historical only; different source, harness, and potentially machine/toolchain. |
| Current frozen snapshot | HEAD `8ea049265f7f` (`dirty=true`), snapshot `fe106066be0ac7b1…` | 18,266.493 ms median | Current measurement; no cross-environment speedup ratio is claimed. |

The historical `10,375.754 ms` row remains unchanged in [the original benchmark](../packet28_search_tool_benchmark.md).

## Method

- Select the version-control-visible worktree with `git ls-files --cached --others --exclude-standard -z`, so tracked modifications and non-ignored untracked source are captured. Generated outputs and disposable Python runtime artifacts are excluded.
- Copy those files once to a frozen OS-temporary source snapshot. The repository-root `.git`, `.packet28`, `target`, and ignored user state are not copied or passed to p28.
- Build `packet28-search-cli` from that frozen snapshot with the checked-in lock graph and a temporary `CARGO_TARGET_DIR`.
- Clone a fresh fixture for every iteration, add an empty deterministic temporary Git commit for git-aware traversal, and time `p28 debug build` against that fixture.
- Record both p28's internal `build_ms` and external process wall time. Fixture creation is deliberately outside both timers.

## Reproduce

```text
python3 benchmarks/per-10-workspace-index/run.py --iterations 3
```

The harness executes these command shapes:

```text
CARGO_TARGET_DIR=<temporary-cargo-target> cargo build --quiet --release --locked -p packet28-search-cli
<temporary-cargo-target>/release/p28 debug build <fresh-temporary-fixture>
```

Raw source, host, toolchain, command, stdout/stderr, and per-iteration records are in [`current-2026-07-28-v1.json`](current-2026-07-28-v1.json).

## Current results

| Run | p28 build_ms | Wall ms | Generation | Indexed files |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 26949.847 | 27118.517 | 1 | 1010 |
| 2 | 18266.493 | 18274.757 | 1 | 1010 |
| 3 | 18245.793 | 18254.117 | 1 | 1010 |

Median internal build time: **18266.493 ms** (min 18245.793, max 26949.847).
Median external wall time: **18274.757 ms** (min 18254.117, max 27118.517).

## Safety invariants

- Every fixture path is mechanically required to be beneath the harness-owned temporary directory and outside the live repository.
- Every p28 invocation receives only a fresh temporary fixture path.
- This preliminary result predates the current runner's pinned historical digest, nested `.packet28` exclusion, Git-environment isolation, and symlink-boundary checks; a final integrated-HEAD rerun is required.
- The historical benchmark file was hashed before and after the preliminary run and remained unchanged.
- Temporary `.packet28` indexes disappear with the temporary directory.
