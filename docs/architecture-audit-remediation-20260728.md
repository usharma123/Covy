# Architecture audit remediation ledger

Source: `architecture-review-20260728-114543.html` (28 July 2026).

This ledger is the completion contract for the audit remediation. Every audit
claim is assigned an ID and must record:

- current-source validation (`confirmed`, `historical`, `already fixed`, or
  `not reproducible`);
- the implementation or mechanically verified invariant that closes it;
- focused and full-gate evidence; and
- the atomic commit that closes it.

Overlapping observations share one implementation where appropriate, but each
source observation remains listed. Historical and experiment-gated claims are
not promoted to current facts without new measurements.

Status keys: `PENDING`, `IN PROGRESS`, `DONE`, `EVIDENCE ONLY`, `NOT
REPRODUCIBLE`.

## Correctness and lifecycle

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| COR-01 | Linux `open`/`openat` preload hooks use a fixed ABI for variadic libc symbols. | Pending | Preserve the C variadic ABI and only consume `mode_t` when flags require it. Add public safety contracts around unsafe hooks. | Linux compile/link ABI regression plus flag/mode behavior tests; focused unsafe Clippy. | Pending | PENDING |
| COR-02 | Malformed or unreadable policy/configuration falls back to defaults on command paths, including coverage gates. | Pending | Introduce an explicit missing-vs-invalid load result. Missing optional configuration may use defaults; present but unreadable or malformed configuration fails closed with a typed diagnostic. | Unit tests for missing, malformed, unreadable, and valid config; CLI failure-path tests for every call-site family. | Pending | PENDING |
| COR-03 | `TaskCancel` removes bookkeeping without stopping sequence work or child processes; completion can resurrect a task. | Pending | Give task execution a cancellation owner; cancellation terminates owned work/children and terminal state cannot be overwritten by late completion. | Deterministic cancel-before-start, cancel-during-child, cancel-vs-complete race, and no-resurrection tests. | Pending | PENDING |
| COR-04 | Forced-TCP `Stop` acknowledges without waking the TCP accept loop. | Pending | Route shutdown through one transport lifecycle signal that wakes TCP and Unix listeners and joins connection work. | Forced-TCP stop regression with bounded shutdown timeout and socket-release assertion. | Pending | PENDING |
| COR-05 | macOS swap lacks RAII restoration after spawning. | Pending | Own swapped files and spawned process in a guard that restores on success, error, timeout, and unwind/drop. | Success/failure/early-return restoration tests and child cleanup assertion. | Pending | PENDING |
| COR-06 | Partial search-index records are accepted as clean EOF. | Pending | Distinguish clean EOF at a record boundary from truncated/corrupt records and preserve recovery/fallback provenance. | Truncation-at-each-field/property cases plus valid EOF regression. | Pending | PENDING |
| COR-07 | Subscriber queues are unbounded and connection servers have no read deadlines. | Pending | Bound queues with an explicit overflow policy and enforce idle/request deadlines. | Slow-subscriber/backpressure and stalled-client deadline tests. | Pending | PENDING |
| COR-08 | Task lifecycle uses parallel booleans; search status is a string plus optional fields. | Pending | Replace invalid combinations with runtime enums and typed transitions; do not introduce PhantomData where runtime flexibility is required. | Exhaustive transition/unit tests and serialization compatibility tests. | Pending | PENDING |

## Reproducibility, CI, release, and tooling

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| REP-01 | `Cargo.lock` is ignored/untracked and clean builds resolve fresh graphs. | Confirmed: `.gitignore` ignores `Cargo.lock`; the worktree has no tracked lock. | Track the generated lockfile and make every CI/local/release Cargo gate use `--locked`. | `git check-ignore`, `git ls-files`, `cargo metadata --locked`, canonical gate. | Pending | IN PROGRESS |
| REP-02 | Declared Rust 1.75 MSRV is incompatible with selected dependencies. | Pending | Choose the lowest credible supported toolchain from the locked graph, document it, and continuously test it. | Locked build/check on declared MSRV plus dependency-MS​​RV inspection. | Pending | PENDING |
| REP-03 | PR CI omits workspace check/build, strict Clippy, rustdoc, MSRV, and deny policy. | Confirmed for `.github/workflows/build.yml`; other workflows still pending inventory. | Define one canonical script and invoke it from CI with locked formatting, check/build, strict Clippy, tests/all features, rustdoc, architecture, deny, MSRV, and package checks. | Workflow syntax inspection and local canonical-gate run. | Pending | IN PROGRESS |
| REP-04 | `deny.toml` policy exists but is not executed. | Pending | Add `cargo deny check` to the canonical local/CI gate without weakening policy. | `cargo deny check --locked` or the supported locked equivalent. | Pending | PENDING |
| REP-05 | Workspace lint inheritance is absent; opt-in panic/unsafe/clone/perf lints are not policy. | Pending | Add workspace lint policy and explicit member inheritance; enable useful lints at a warning/deny level justified by a clean migration. | Strict all-target/all-feature Clippy and metadata assertion that every member inherits. | Pending | PENDING |
| REP-06 | Internal path/version declarations are repeated across manifests. | Pending | Centralize internal dependency versions/paths in `[workspace.dependencies]` and inherit them in members. | Metadata script checks one version/path source and an acyclic graph. | Pending | PENDING |
| REP-07 | Release tags are not asserted against Cargo/package versions. | Pending | Add a release preflight that rejects tag/Cargo/npm version mismatch. | Unit/fixture cases and packaging dry-run. | Pending | PENDING |
| REP-08 | GitHub Action references and ARM cross installer are mutable. | Pending | Pin actions/install artifacts to immutable revisions with checksum verification. | Workflow static check that action refs are full SHAs and downloads verify checksums. | Pending | PENDING |
| REP-09 | README repository counts drifted (25→28 crates, 247→543 Rust files). | Pending | Generate/verify counts from tracked source and update the README. | Documentation-count verifier. | Pending | PENDING |
| REP-10 | Local batch validation is useful but is not the complete canonical release gate. | Pending | Preserve the fast batch path and add a deterministic full gate used before commits/releases. | Script self-tests/list mode plus full local execution. | Pending | PENDING |

## Crate and module architecture

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| ARC-01 | Daemon wire DTO/framing depends on kernel, memory, and reducer implementations. | Pending | Create a deep protocol crate/module containing only wire DTOs and framing; runtime implementation stays in `packet28d`/runtime ownership. | Cargo metadata negative-dependency assertions; protocol round-trip and compatibility tests for all adapters. | Pending | PENDING |
| ARC-02 | Kernel mechanism owns 15 concrete built-ins, including a CLI adapter. | Pending | Keep scheduler/policy/cache/audit/execution mechanism in kernel; move built-in registration to a composition boundary. | Registry behavior parity, dependency rule, and end-to-end reducer routing tests. | Pending | PENDING |
| ARC-03 | MCP catalog, argument parsing, dispatch, execution, summary, and response shape are scattered across shallow modules. | Pending | Co-locate each native tool family lifecycle behind one narrow session/daemon interface. | Per-family schema/parse/execute/summary snapshot tests and tools-list compatibility. | Pending | PENDING |
| ARC-04 | Broker files import a parent prelude wholesale and expose a hidden sibling interface. | Pending | Add one owning broker module with explicit state port; keep `main` as process bootstrap. | Visibility/dependency checks and broker behavior parity tests. | Pending | PENDING |
| ARC-05 | Local memory is a broad re-export hub; operations independently derive/open/initialize SQLite. | Pending | Make local memory own connection lifecycle, migrations, prepared work, transactions, memory/graph/feedback workflows. | Migration, transaction rollback, graph batch, and CLI/MCP compatibility tests. | Pending | PENDING |
| ARC-06 | Runtime integration behavior leaks across repeated `RuntimeKind` switchboards. | Pending | Let each runtime integration own paths, detection, capability, setup, and status while setup orchestrates cross-runtime concerns. | Table-driven integration fixture tests and switchboard/dependency invariant. | Pending | PENDING |
| ARC-07 | Public compatibility/re-export modules expose implementation details. | Pending | Narrow exports and quarantine compatibility adapters at explicit boundaries without breaking supported public behavior. | `cargo public-api`-style snapshot or rustdoc/compile tests for supported surface. | Pending | PENDING |
| ARC-08 | Architecture diagrams have no mechanically enforced negative dependency rules. | Pending | Add metadata/source architecture tests for prohibited crate and module edges. | Architecture test runs in canonical gate and fails on synthetic/fixture violations. | Pending | PENDING |

## Public APIs, errors, documentation, and tests

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| API-01 | Reusable search, daemon, diff, and test libraries leak `anyhow` and erase typed sources into strings. | Pending | Define stable `thiserror` error seams in libraries; reserve contextual `anyhow` reporting for binaries. | Error variant/source/display tests and public API compile checks. | Pending | PENDING |
| DOC-01 | Rustdoc has broken intra-doc links in `impact.rs` and `cmd_map_paths.rs`. | Pending | Repair links and enforce `-D rustdoc::broken_intra_doc_links`. | Strict rustdoc gate. | Pending | PENDING |
| DOC-02 | Stable packet, ingestion, kernel, and daemon public interfaces lack crate/item docs, examples, and Errors/Panics/Safety contracts. | Pending | Document stable public seams first; add executable examples and explicit contracts. Avoid cosmetic docs for internal details. | Missing-doc policy on selected public crates, doctests, strict rustdoc. | Pending | PENDING |
| TST-01 | `suite-ingest` has no direct tests. | Pending | Add direct parser/dispatch success and failure-path tests. | Package test target. | Pending | PENDING |
| TST-02 | `testy-cli-common` has no direct tests. | Pending | Add direct public-behavior and error tests. | Package test target. | Pending | PENDING |
| TST-03 | No property coverage exists where schemas/parsers/invariants call for it. | Pending | Add bounded deterministic property tests for protocol/schema round-trips, corruption boundaries, and lifecycle transitions. | Seeded property suite in normal CI. | Pending | PENDING |
| TST-04 | No fuzz coverage exists. | Pending | Add non-default fuzz targets/corpus for untrusted framing/index/parser inputs and a bounded smoke runner. | Fuzz target compilation plus bounded smoke command. | Pending | PENDING |
| TST-05 | No snapshot coverage exists for structural MCP/protocol output. | Pending | Add small named snapshots for stable schemas/catalog/response shapes, with volatile fields normalized. | Snapshot test with committed reviewed snapshots. | Pending | PENDING |
| TST-06 | No compile-fail coverage exists for public API constraints. | Pending | Add `compile_fail` doctests or trybuild cases for invalid public-state/API usage where it adds real leverage. | Compile-fail test in canonical docs/test gate. | Pending | PENDING |
| TST-07 | No doctest examples exist. | Pending | Add runnable happy-path examples to stable library APIs. | `cargo test --doc --workspace --locked`. | Pending | PENDING |
| TST-08 | Integration tests use many one-off helpers/nested builds and leak cleanup. | Pending | Introduce one deep RAII process/MCP/timeout/cleanup harness; migrate reused helpers while leaving truly local fixtures local. | Harness lifecycle/failure tests and representative migrated workflows. | Pending | PENDING |

## Persistence, cache, index, SQLite, and compute performance

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| PER-01 | Task/watch events append, then serialize/sync complete registries while holding the daemon-wide mutex. | Pending | Give persistence one owner: append durable event/WAL records, mark dirty state, serialize immutable snapshots outside the state lock, coalesce/checkpoint. | Crash/replay, concurrent mutation, lock-duration telemetry, and before/after release benchmark. | Pending | PENDING |
| PER-02 | Context-cache writes evict/clone/serialize the full envelope and write while holding the cache mutex. | Pending | Track dirty entries and move encoding/I/O into a debounced cache-persistence owner using deltas/checkpoints. | Concurrency correctness, crash recovery, bounded flush, lock-time and write-byte benchmarks. | Pending | PENDING |
| PER-03 | One-file index updates clone/rewrite the full repository; regex overlays reread and rebuild all overlay documents. | Pending | Use an owned mutable index with immutable base generations, overlay segments/tombstones, and threshold compaction while retaining the base `Arc`. | Update/delete/compaction parity, corruption recovery, concurrent reader tests, and incremental benchmark. | Pending | PENDING |
| PER-04 | Every local-memory connection reruns schema/FTS setup; graph workflows reopen DB repeatedly. | Pending | Use `PRAGMA user_version` migrations, an owning connection/store boundary, prepared statements, and shared batch transactions. | Migration-from-each-version, idempotence, rollback, connection-count telemetry, benchmark. | Pending | PENDING |
| PER-05 | Test planning materializes a dense tests×files matrix and clones/rebuilds bitmaps. | Pending | Persist sparse bitmap rows, intersect by reference, and avoid cloning remaining sets in gain calculation. | Property parity and realistic scale benchmark over tests/files/changed lines. | Pending | PENDING |
| PER-06 | Map and regex indexes independently walk/read; diagnostic ingestion copies whole files; LCOV allocates per record. | Pending | Share bounded scan/content work and stream/remove copies only where current release benchmarks show leverage. | Allocation/throughput benchmarks plus parser parity/property tests. | Pending | PENDING |
| PER-07 | Map cache identity uses second-resolution mtime, so same-size rapid edits can collide. | Pending | Include nanosecond timestamps and a content identity fallback in cache keys. | Same-size/same-second rapid-edit regression. | Pending | PENDING |
| PER-08 | Local task/artifact storage has no bounded retention policy. | Pending | Add explicit age/size retention, observability, and a dry-run cleanup command; never delete outside resolved Packet28 state. | Dry-run/default safety, age/size boundary, active-artifact protection, and cleanup accounting tests. | Pending | PENDING |
| PER-09 | Roughly 60 redundant clones, intermediate collections, nested merge allocations, and 48–88 byte values passed by copy need measurement-led cleanup. | Pending | Enable focused lints, remove verified hot-path copies/allocations, and document benchmark-rejected changes. | Focused Clippy groups and release microbenchmarks with before/after evidence. | Pending | PENDING |
| PER-10 | Historical 10.376 s workspace-index result is not current-HEAD evidence. | Historical by audit; current rerun pending. | Preserve the historical row as provenance and record current controlled baselines before claiming improvement. | Versioned benchmark command, machine/toolchain metadata, raw artifact. | Pending | EVIDENCE ONLY |
| PER-11 | Live store/task-registry figures are volatile observations, not product constants. | Historical/volatile by audit. | Add supported inspect/retention metrics rather than hard-code the sampled values. | Metrics command fixture and current snapshot labeled with timestamp. | Pending | EVIDENCE ONLY |

## Tokio orchestration boundary

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| ASY-01 | MCP proxy is a serial stdio loop without request-ID concurrency, per-upstream timeouts, or owned child cleanup. | Pending | Adopt Tokio at this orchestration seam with concurrent ID routing, bounded in-flight work, timeouts, and child ownership. | Out-of-order response, timeout, child-exit, cancellation, and protocol framing tests. | Pending | PENDING |
| ASY-02 | Daemon transport uses thread-per-connection and split lifecycle logic. | Pending | Use structured Tokio connection tasks, bounded channels, deadlines, and unified TCP/Unix shutdown; keep CPU/blocking work off runtime workers. | TCP/Unix parity, load/backpressure, shutdown/join, and stalled-client tests. | Pending | PENDING |
| ASY-03 | Wait/watch/notification uses polling/sleeps and one synchronous debounce processor. | Pending | Use owned timers/signals/bounded tasks at the async boundary. | Deterministic paused-time debounce, cancellation, overflow, and shutdown tests. | Pending | PENDING |
| ASY-04 | Tokio does not make scans, bitmap work, SQLite, serialization, or full-snapshot writes non-blocking. | Confirmed design constraint from audit; source boundary pending. | Retain Rayon for CPU parallelism and isolate blocking work with bounded workers/`spawn_blocking`; no sync DB/full-snapshot I/O on core runtime tasks. | Architecture test plus runtime starvation benchmark. | Pending | PENDING |

## Stable instruction prefix and controlled experiment

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| EXP-01 | Identical instruction sources can render different bytes because active task/snapshot state changes selection, excerpts, focus line, and header. | Confirmed by audit; current source/tests pending. | Separate stable source/path/schema/budget/repo-config rendering from mutable broker brief content. | Byte-hash invariance across task/snapshot changes. | Pending | PENDING |
| EXP-02 | Passthrough must remain the default baseline; stable and adaptive modes are controlled variants. | Experiment-gated. | Add explicit `passthrough`, `stable`, and `adaptive` modes with conservative feature/config gating and compatibility telemetry. | Mode-selection/default tests and output compatibility cases. | Pending | PENDING |
| EXP-03 | Required scenarios: cold start, second request, compaction, task A→B, snapshot drift, fresh-worker handoff. | Experiment-gated. | Add a reproducible experiment harness and manifest covering all scenarios. | Repeated controlled run artifacts with stable-prefix hashes. | Pending | PENDING |
| EXP-04 | Required metrics: churn rate, reuse multiple, effective cache cost, compaction rewarm tokens; hit rate alone is insufficient. | Experiment-gated. | Record creation/read tokens and costs separately, with provider/order metadata and explicit unknowns. | Schema tests and benchmark report generated from raw records. | Pending | PENDING |
| EXP-05 | Claims of fixed token loss, 100% misses, or guaranteed net savings are unsupported. | Evidence boundary retained. | Documentation and telemetry must label these as hypotheses until provider cache placement/order is measured. | Claim-lint/doc review plus no unsupported constants in user-facing output. | Pending | EVIDENCE ONLY |

## Verified strengths and preservation constraints

These audit observations are invariants to retain, not change requests.

| ID | Audit observation | Preservation evidence | Closing commit | Status |
|---|---|---|---|---|
| INV-01 | Workspace dependency graph is acyclic and mostly points toward packet/foundation contracts. | Pending current metadata graph plus the new negative-dependency test. | Pending | PENDING |
| INV-02 | Scheduler is a deep typed interface for DAG validation, mutation, ordering, and budget enforcement. | Existing focused tests plus post-refactor full suite. | Pending | PENDING |
| INV-03 | Behavioral coverage is broad and deterministic contracts have focused tests. | Current test inventory and full locked suite. | Pending | PENDING |
| INV-04 | Production APIs borrow idiomatically and dynamic dispatch is used at real runtime-selection seams. | Focused source/Clippy inspection; no churn without measured value. | Pending | PENDING |
| INV-05 | Unsafe code stays peripheral to core algorithms. | Unsafe inventory and architecture check after FFI fixes. | Pending | PENDING |
| INV-06 | Packet hashing, schemas, route catalog, scheduler failures, index parity, exit codes, and fallbacks retain deterministic behavior. | Existing and added compatibility/regression suites. | Pending | PENDING |

## Canonical final gate

The exact commands and results will be recorded here before the final push.

| Gate | Command | Result |
|---|---|---|
| Formatting | Pending | PENDING |
| Workspace check | Pending | PENDING |
| Workspace build | Pending | PENDING |
| Strict Clippy | Pending | PENDING |
| Full all-feature tests | Pending | PENDING |
| Doctests | Pending | PENDING |
| Strict rustdoc | Pending | PENDING |
| Architecture rules | Pending | PENDING |
| Supply-chain policy | Pending | PENDING |
| MSRV | Pending | PENDING |
| Packaging/release dry-run | Pending | PENDING |
| Performance/caching experiments | Pending | PENDING |

## Ordered commits

Filled as atomic slices land.

| Order | Commit | Invariants closed |
|---|---|---|
| 1 | Pending | Pending |
