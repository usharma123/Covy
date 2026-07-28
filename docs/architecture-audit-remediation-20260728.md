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
| COR-01 | Linux `open`/`openat` preload hooks use a fixed ABI for variadic libc symbols. | Confirmed at baseline: four fixed-arity Rust exports and four fixed function-pointer casts crossed the variadic libc ABI. | C now owns every variadic declaration, argument read, and `RTLD_NEXT` invocation for `open`, `open64`, `openat`, and `openat64`; fixed Rust callbacks only perform replacement lookup. A mode is consumed only for `O_CREAT` or the complete `O_TMPFILE` mask, and the unsafe callback contracts are documented. | A Linux `LD_PRELOAD` integration test compiles and runs a C caller across all four symbols, both arities, created-file modes, replacement paths, and `errno`; package tests/checks cover the platform-neutral and macOS paths. | `0f708f1` | DONE |
| COR-02 | Malformed or unreadable policy/configuration falls back to defaults on command paths, including coverage gates. | Confirmed: `exists()` suppressed metadata failure and exactly 15 production callers erased load errors with `unwrap_or_default`. | `ConfigLoadError` distinguishes absence from path-aware read/parse failure; direct reads remove the existence-check race; all callers propagate present-invalid configuration. | 44 foundation tests; complete affected covy/diffy/testy/suite test run in a clean detached worktree; strict affected-package Clippy and foundation rustdoc; source invariant found zero erased loads. | `803a8e9` | DONE |
| COR-03 | `TaskCancel` removes bookkeeping without stopping sequence work or child processes; completion can resurrect a task. | Confirmed: cancellation removes the record; observers and untracked child waiters can recreate it through entry-or-insert completion paths. | Give each task generation a cancellation owner; cancellation terminates owned work/process groups, reaps children, and stale completion cannot mutate a new generation. | Deterministic cancel-before-start, between-step, during-child, cancel-vs-complete, same-ID reuse, idempotence, and no-resurrection tests. | Pending | PENDING |
| COR-04 | Forced-TCP `Stop` acknowledges without waking the TCP accept loop. | Confirmed: shutdown wake connects only to the Unix socket while forced-TCP accept remains blocking. | Route shutdown through one transport lifecycle signal that wakes TCP and Unix listeners and joins connection work. | Forced-TCP and Unix stop regressions with bounded process exit and endpoint-release assertions. | Pending | PENDING |
| COR-05 | macOS swap lacks RAII restoration after spawning. | Confirmed: three post-spawn `?` paths and unwinding can bypass restore/child cleanup; recovery can delete the current file before verifying its backup. | Own a durable swap journal, staged files, relay, and child in a guard; use non-destructive restore ordering on success, error, timeout, and unwind/drop. | Injected stage/spawn/report/signal/wait failures, unwind, partial rollback, missing-backup, crash recovery, and orphan-child tests. | Pending | PENDING |
| COR-06 | Partial search-index records are accepted as clean EOF. | Confirmed: segment `UnexpectedEof`, trailing partial lookup rows, and invalid posting ranges can become clean EOF/misses. | Distinguish clean EOF at a record boundary from truncated/corrupt records; validate lookup alignment/ranges and preserve recovery provenance without publishing partial layers. | Every record/row truncation boundary, overflow/range corruption, valid EOF, cleanup, and no-publication property cases. | Pending | PENDING |
| COR-07 | Subscriber queues are unbounded and connection servers have no read deadlines. | Confirmed: unbounded subscriber and watch channels, untracked thread-per-connection, and no server read/write deadlines. | Bound queues/connections with an explicit lag-and-replay policy, enforce deadlines, and join structured connection ownership. | Slow subscriber, replay gap, stalled header/body, non-reading writer, connection cap, TCP/Unix parity, and shutdown tests. | Pending | PENDING |
| COR-08 | Task lifecycle uses parallel booleans; search status is a string plus optional fields. | Pending | Replace invalid combinations with runtime enums and typed transitions; do not introduce PhantomData where runtime flexibility is required. | Exhaustive transition/unit tests and serialization compatibility tests. | Pending | PENDING |

## Reproducibility, CI, release, and tooling

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| REP-01 | `Cargo.lock` is ignored/untracked and clean builds resolve fresh graphs. | Confirmed at baseline: `.gitignore` ignored `Cargo.lock`; the worktree had no tracked lock. | The preserved lockfile is tracked and the local batch Cargo gates use `--locked`; workflow/release enforcement remains under REP-03/REP-10. | Lock SHA-256 `8c954b492ec25ab37c277b149b734ff039abfc0eeaf6ca945b790fae1a5dc6a0`; policy verifier, locked metadata/check, and locked all-feature workspace tests passed. | `018b74a` plus REP-03/REP-10 follow-up | IN PROGRESS |
| REP-02 | Declared Rust 1.75 MSRV is incompatible with selected dependencies. | Confirmed: the preserved audit lock includes active dependencies declaring Rust 1.88; CI only tested moving stable. | Workspace MSRV is 1.88.0 and every member inherits it; exact-version CI remains under REP-03. | `rustup run 1.88.0 cargo check --workspace --all-targets --all-features --locked` passed; dependency/MSRV policy verifier passed. | `018b74a` plus REP-03 follow-up | IN PROGRESS |
| REP-03 | PR CI omits workspace check/build, strict Clippy, rustdoc, MSRV, and deny policy. | Confirmed for `.github/workflows/build.yml`; other workflows still pending inventory. | Define one canonical script and invoke it from CI with locked formatting, check/build, strict Clippy, tests/all features, rustdoc, architecture, deny, MSRV, and package checks. | Workflow syntax inspection and local canonical-gate run. | Pending | IN PROGRESS |
| REP-04 | `deny.toml` policy exists but is not executed. | Pending | Add `cargo deny check` to the canonical local/CI gate without weakening policy. | `cargo deny check --locked` or the supported locked equivalent. | Pending | PENDING |
| REP-05 | Workspace lint inheritance is absent; opt-in panic/unsafe/clone/perf lints are not policy. | Confirmed at baseline: no workspace lint table and member inheritance was absent. | Added conservative workspace lint policy and explicit inheritance in all 28 members, then repaired every warning exposed by the strict all-target/all-feature workspace pass without weakening policy. | The policy verifier asserts all-member inheritance; `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` passed in a clean isolated worktree. | `018b74a`, `580954c` | DONE |
| REP-06 | Internal path/version declarations are repeated across manifests. | Confirmed at baseline: 93 member-local internal path declarations repeated workspace topology. | Centralized 21 internal dependencies in `[workspace.dependencies]`; all 93 member declarations inherit the single source. | Locked metadata and policy verifier assert zero member-local internal paths and an unchanged acyclic graph. | `018b74a` | DONE |
| REP-07 | Release tags are not asserted against Cargo/package versions. | Pending | Add a release preflight that rejects tag/Cargo/npm version mismatch. | Unit/fixture cases and packaging dry-run. | Pending | PENDING |
| REP-08 | GitHub Action references and ARM cross installer are mutable. | Pending | Pin actions/install artifacts to immutable revisions with checksum verification. | Workflow static check that action refs are full SHAs and downloads verify checksums. | Pending | PENDING |
| REP-09 | README repository counts drifted (25→28 crates, 247→543 Rust files). | Pending | Generate/verify counts from tracked source and update the README. | Documentation-count verifier. | Pending | PENDING |
| REP-10 | Local batch validation is useful but is not the complete canonical release gate. | Pending | Preserve the fast batch path and add a deterministic full gate used before commits/releases. | Script self-tests/list mode plus full local execution. | Pending | PENDING |

## Crate and module architecture

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| ARC-01 | Daemon wire DTO/framing depends on kernel, memory, and reducer implementations. | Confirmed at baseline: daemon core directly depended on kernel, memory, and reducer implementations, and four adapter/CLI consumers imported its mixed root surface. The contract extraction is landed; direct consumer migration and a permanent architecture guard remain open. | Shared kernel, memory, search, and governance wire types now live in implementation-free `suite-packet-core` modules with compatibility re-exports. Daemon DTOs, framing, commands, and paths now live in `packet28-daemon-protocol`; daemon storage remains in `packet28-daemon-core`, whose legacy root facade preserves source/wire compatibility. | Four shared-wire compatibility suites; exhaustive daemon request/response tag tests; bounded frame round-trips and malformed/truncated/oversize cases; four exact JSON goldens; public/legacy API compile tests; protocol/core all-feature and core no-default-feature tests; strict Clippy/rustdoc and workspace check. | Partial: `e56d3f0`, `b3ac32f`; final adapter/guard commit pending | IN PROGRESS |
| ARC-02 | Kernel mechanism owns 15 concrete built-ins, including a CLI adapter. | Pending | Keep scheduler/policy/cache/audit/execution mechanism in kernel; move built-in registration to a composition boundary. | Registry behavior parity, dependency rule, and end-to-end reducer routing tests. | Pending | PENDING |
| ARC-03 | MCP catalog, argument parsing, dispatch, execution, summary, and response shape are scattered across shallow modules. | Pending | Co-locate each native tool family lifecycle behind one narrow session/daemon interface. | Per-family schema/parse/execute/summary snapshot tests and tools-list compatibility. | Pending | PENDING |
| ARC-04 | Broker files import a parent prelude wholesale and expose a hidden sibling interface. | Pending | Add one owning broker module with explicit state port; keep `main` as process bootstrap. | Visibility/dependency checks and broker behavior parity tests. | Pending | PENDING |
| ARC-05 | Local memory is a broad re-export hub; operations independently derive/open/initialize SQLite. | Pending | Make local memory own connection lifecycle, migrations, prepared work, transactions, memory/graph/feedback workflows. | Migration, transaction rollback, graph batch, and CLI/MCP compatibility tests. | Pending | PENDING |
| ARC-06 | Runtime integration behavior leaks across repeated `RuntimeKind` switchboards. | Confirmed at baseline: paths, detection, capabilities, setup, and status were repeated across switchboards for 12 runtimes. | Each of the 12 runtime adapters now owns its paths, detection markers, capabilities, setup actions, and status contract behind one static catalog; setup retains only cross-runtime orchestration. | 247 `suite-cli` library tests and 15 setup E2Es passed in an isolated worktree; catalog order/uniqueness, path/capability, detection, idempotence, invalid-config no-write, command verification, and a source invariant prohibit setup runtime switchboards/literals. | `91b3ed5` | DONE |
| ARC-07 | Public compatibility/re-export modules expose implementation details. | Pending | Narrow exports and quarantine compatibility adapters at explicit boundaries without breaking supported public behavior. | `cargo public-api`-style snapshot or rustdoc/compile tests for supported surface. | Pending | PENDING |
| ARC-08 | Architecture diagrams have no mechanically enforced negative dependency rules. | Pending | Add metadata/source architecture tests for prohibited crate and module edges. | Architecture test runs in canonical gate and fails on synthetic/fixture violations. | Pending | PENDING |

## Public APIs, errors, documentation, and tests

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| API-01 | Reusable search, daemon, diff, and test libraries leak `anyhow` and erase typed sources into strings. | Confirmed at baseline: public signatures included 7 search-core, 22 daemon-core, 5 diffy-core, and multiple test-library `anyhow` results/aliases. The diff-library portion is fixed; search, daemon, and test-library seams remain open. | `diffy-core` now exposes a stable `DiffyError`/typed `Result` across diff and pipeline entrypoints, preserves nested I/O/parse/ingestion sources, and provides actionable hints; contextual `anyhow` conversion is confined to CLI/runtime edges. | 33 diffy unit tests plus 5 external public-API tests, 12 covy-core tests, and 88 affected CLI tests passed; strict affected-package Clippy, `missing_errors_doc`, and rustdoc passed. | Partial: `3b2dae7`; remaining library commits pending | IN PROGRESS |
| DOC-01 | Rustdoc has broken intra-doc links in `impact.rs` and `cmd_map_paths.rs`. | Confirmed exactly; strict baseline failed on both links and also exposed 12 invalid HTML placeholder tags. | Repaired both links and all invalid placeholder tags. Canonical gate enforcement remains REP-03. | `cargo fmt --all --check`; `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked` passed. | `3db342d` | DONE |
| DOC-02 | Stable packet, ingestion, kernel, and daemon public interfaces lack crate/item docs, examples, and Errors/Panics/Safety contracts. | Partially confirmed and addressed: the new daemon protocol and typed diff error seams are documented, while the broader packet, ingestion, kernel, and daemon surface still needs the selected missing-doc/doctest pass. | Added protocol crate/module documentation, a migration guide, framing error contracts, and documented typed diff errors/hints. Broader stable-interface examples and contracts remain open. | Strict rustdoc passed for the protocol/core and affected diff crates; protocol public-API and diff external-API tests exercise the documented seams. | Partial: `b3ac32f`, `3b2dae7`; final docs commit pending | IN PROGRESS |
| TST-01 | `suite-ingest` has no direct tests. | Confirmed at baseline: the crate had no direct unit or doctest coverage. | Added API contracts, a runnable ingestion example, and direct auto-detection, explicit-format, merge/deduplication, and failure tests. | 7 unit tests and 1 doctest; strict package Clippy and rustdoc passed. | `8816e73` | DONE |
| TST-02 | `testy-cli-common` has no direct tests. | Confirmed at baseline: the crate had no direct tests. | Added direct CLI parsing, delegation, adapter conversion, ingestion, and contextual-error coverage. | 14 unit tests; strict package Clippy and rustdoc passed. | `0538840` | DONE |
| TST-03 | No property coverage exists where schemas/parsers/invariants call for it. | Partially addressed: bounded protocol-frame and parser-boundary cases plus a 192-seed sparse-planner parity suite are now in normal tests; lifecycle and remaining corruption properties remain open. | Added deterministic variable-size protocol round-trips, generated LCOV record-boundary parity, and generated sparse-vs-legacy dense planning parity while retaining fixed seeds and frozen reference behavior. | Protocol payload sizes 0 through 65,535 round-trip; the LCOV field matrix covers missing, invalid, overflow, extra, and comma-containing cases; 192 generated test/file/line coverage cases produce exactly the legacy plan. | Partial: `b3ac32f`, `7201ddc`, `d2b86f0`; remaining property suites pending | IN PROGRESS |
| TST-04 | No fuzz coverage exists. | Pending | Add non-default fuzz targets/corpus for untrusted framing/index/parser inputs and a bounded smoke runner. | Fuzz target compilation plus bounded smoke command. | Pending | PENDING |
| TST-05 | No snapshot coverage exists for structural MCP/protocol output. | Partially addressed: the pre-existing 39 packet snapshots remain shallow, but four representative daemon protocol requests now have exact reviewed JSON fixtures; MCP structural drift remains uncovered. | Added normalized full-value goldens for instruction, search, broker, and hook adapter requests using committed JSON fixtures. | All four fixtures are parsed and compared against complete serialized request values; exhaustive discriminator tests cover every daemon request and response variant. | Partial: `b3ac32f`; MCP and broader packet snapshots pending | IN PROGRESS |
| TST-06 | No compile-fail coverage exists for public API constraints. | Pending | Add `compile_fail` doctests or trybuild cases for invalid public-state/API usage where it adds real leverage. | Compile-fail test in canonical docs/test gate. | Pending | PENDING |
| TST-07 | No doctest examples exist. | Partially addressed: `suite-ingest` now has a runnable end-to-end ingestion example; the remaining stable library APIs still lack the requested doctest coverage. | Added a crate-level LCOV path-ingestion example that creates a report, calls the public API, checks the decoded file, and cleans up. Broader happy-path examples remain open. | The `suite-ingest` doctest passed with the package tests and strict rustdoc; the final workspace-wide doctest gate remains pending. | Partial: `8816e73`; remaining stable APIs pending | IN PROGRESS |
| TST-08 | Integration tests use many one-off helpers/nested builds and leak cleanup. | Pending | Introduce one deep RAII process/MCP/timeout/cleanup harness; migrate reused helpers while leaving truly local fixtures local. | Harness lifecycle/failure tests and representative migrated workflows. | Pending | PENDING |

## Persistence, cache, index, SQLite, and compute performance

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| PER-01 | Task/watch events append, then serialize/sync complete registries while holding the daemon-wide mutex. | Pending | Give persistence one owner: append durable event/WAL records, mark dirty state, serialize immutable snapshots outside the state lock, coalesce/checkpoint. | Crash/replay, concurrent mutation, lock-duration telemetry, and before/after release benchmark. | Pending | PENDING |
| PER-02 | Context-cache writes evict/clone/serialize the full envelope and write while holding the cache mutex. | Pending | Track dirty entries and move encoding/I/O into a debounced cache-persistence owner using deltas/checkpoints. | Concurrency correctness, crash recovery, bounded flush, lock-time and write-byte benchmarks. | Pending | PENDING |
| PER-03 | One-file index updates clone/rewrite the full repository; regex overlays reread and rebuild all overlay documents. | Pending | Use an owned mutable index with immutable base generations, overlay segments/tombstones, and threshold compaction while retaining the base `Arc`. | Update/delete/compaction parity, corruption recovery, concurrent reader tests, and incremental benchmark. | Pending | PENDING |
| PER-04 | Every local-memory connection reruns schema/FTS setup; graph workflows reopen DB repeatedly. | Pending | Use `PRAGMA user_version` migrations, an owning connection/store boundary, prepared statements, and shared batch transactions. | Migration-from-each-version, idempotence, rollback, connection-count telemetry, benchmark. | Pending | PENDING |
| PER-05 | Test planning materializes a dense tests×files matrix and clones/rebuilds bitmaps. | Confirmed at baseline: planning constructed dense test/file membership work and cloned remaining bitmaps in gain calculation. | Test-map v3 persists only sorted non-empty bitmap cells behind explicit magic/version framing, reads and migrates v1/v2, validates corruption boundaries, intersects borrowed bitmaps, and precomputes candidate overlaps without cloning remaining sets. | Storage round-trip/migration and malformed header/version/count/index/order/bitmap/trailing-byte tests; 192-seed sparse-vs-dense planner parity. A deterministic 2,000-test/1,000-file/24,000-cell/10,240-line release fixture improved median planning from 654,174 µs to 10,091 µs (64.8×), serialization from 12,332 µs to 3,796 µs (3.2×), and artifact size from 17,613,076 to 1,613,085 bytes (10.9×); the slower synthetic construction is reported. | `d2b86f0`, `153a3e3` | DONE |
| PER-06 | Map and regex indexes independently walk/read; diagnostic ingestion copies whole files; LCOV allocates per record. | Confirmed for duplicate walks, diagnostics whole-file copies, and LCOV per-record collection. The two parser-allocation claims are fixed with measured evidence; shared map/regex scanning remains experiment-gated and open. | Diagnostics now parse directly from the format-detection buffer, and LCOV `DA`/`BRDA`/`FNDA` parsing borrows iterator fields instead of allocating temporary vectors. Streaming SARIF and memory mapping were explicitly deferred because the measurements did not justify their added complexity. | Ten-iteration allocation probes and release Criterion artifacts show LCOV allocations 50,012→12, requested bytes -96.97%, and time -33.93%; diagnostics allocations 2→1 and requested bytes -50% with no target regression. Generated boundary-parity tests preserve malformed, overflow, extra-field, branch, and comma-containing-name behavior; focused tests and strict Clippy passed. | Partial: `7201ddc`; shared-scan experiment pending | IN PROGRESS |
| PER-07 | Map cache identity uses second-resolution mtime, so same-size rapid edits can collide. | Confirmed: cache reuse trusted size plus second-resolution mtime, including warm query paths. | Cache v5 records signed nanosecond mtime and a BLAKE3-backed content fingerprint; reads are reused for parsing and invalid UTF-8 evicts stale entries. | 27 mapy tests including fixed same-size/same-mtime edits, warm reuse, v4 invalidation, invalid UTF-8, and pre-epoch/subsecond timestamps; strict package Clippy passed. | `13cc5c3` | DONE |
| PER-08 | Local task/artifact storage has no bounded retention policy. | Pending | Add explicit age/size retention, observability, and a dry-run cleanup command; never delete outside resolved Packet28 state. | Dry-run/default safety, age/size boundary, active-artifact protection, and cleanup accounting tests. | Pending | PENDING |
| PER-09 | Roughly 60 redundant clones, intermediate collections, nested merge allocations, and 48–88 byte values passed by copy need measurement-led cleanup. | Pending | Enable focused lints, remove verified hot-path copies/allocations, and document benchmark-rejected changes. | Focused Clippy groups and release microbenchmarks with before/after evidence. | Pending | PENDING |
| PER-10 | Historical 10.376 s workspace-index result is not current-HEAD evidence. | Historical by audit; current rerun pending. | Preserve the historical row as provenance and record current controlled baselines before claiming improvement. | Versioned benchmark command, machine/toolchain metadata, raw artifact. | Pending | EVIDENCE ONLY |
| PER-11 | Live store/task-registry figures are volatile observations, not product constants. | Historical/volatile by audit. | Add supported inspect/retention metrics rather than hard-code the sampled values. | Metrics command fixture and current snapshot labeled with timestamp. | Pending | EVIDENCE ONLY |

## Tokio orchestration boundary

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| ASY-01 | MCP proxy is a serial stdio loop without safe concurrent request routing or owned child cleanup. | Partially confirmed: configurable per-upstream timeouts already exist, but responses share an unbounded FIFO and IDs are ignored, so a late timed-out response can poison the next request. | Adopt Tokio at this orchestration seam with ID-correlated concurrent routing, bounded in-flight/output work, existing timeouts, and child ownership. | Out-of-order and timeout-then-late response routing, fast-before-slow concurrency, child-exit/reap, cancellation, backpressure, and framing tests. | Pending | PENDING |
| ASY-02 | Daemon transport uses thread-per-connection and split lifecycle logic. | Pending | Use structured Tokio connection tasks, bounded channels, deadlines, and unified TCP/Unix shutdown; keep CPU/blocking work off runtime workers. | TCP/Unix parity, load/backpressure, shutdown/join, and stalled-client tests. | Pending | PENDING |
| ASY-03 | Wait/watch/notification uses polling/sleeps and one synchronous debounce processor. | Pending | Use owned timers/signals/bounded tasks at the async boundary. | Deterministic paused-time debounce, cancellation, overflow, and shutdown tests. | Pending | PENDING |
| ASY-04 | Tokio does not make scans, bitmap work, SQLite, serialization, or full-snapshot writes non-blocking. | Confirmed design constraint from audit; source boundary pending. | Retain Rayon for CPU parallelism and isolate blocking work with bounded workers/`spawn_blocking`; no sync DB/full-snapshot I/O on core runtime tasks. | Architecture test plus runtime starvation benchmark. | Pending | PENDING |

## Stable instruction prefix and controlled experiment

| ID | Audit finding or recommendation | Current-source validation | Implementation / invariant | Test or mechanical evidence | Closing commit | Status |
|---|---|---|---|---|---|---|
| EXP-01 | Identical instruction sources can render different bytes because active task/snapshot state changes selection, excerpts, focus line, and header. | Confirmed in current source. Cache identity is over-partitioned by task/backend metadata but omits snapshot fingerprint, so adaptive output can also be stale after same-task snapshot drift. | Separate stable source/path/schema/budget/repo-config rendering from mutable broker brief content; adaptive caching must include a canonical snapshot fingerprint or remain disabled. | Byte-hash invariance across task/snapshot changes plus adaptive hit/miss correctness. | Pending | PENDING |
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
| 1 | `d12943d docs(audit): add architecture remediation ledger` | Traceability contract established. |
| 2 | `3db342d docs(rustdoc): enforce valid public documentation links` | DOC-01. |
| 3 | `018b74a build(repro): lock workspace graph and enforce MSRV policy` | REP-01/REP-02/REP-05 partial; REP-06. |
| 4 | `8816e73 test(ingest): cover public coverage adapters` | TST-01; DOC-02/TST-07 ingestion portion. |
| 5 | `0538840 test(testy): cover CLI parsing and adapter failures` | TST-02. |
| 6 | `13cc5c3 fix(map): invalidate cache by content fingerprint` | PER-07. |
| 7 | `803a8e9 fix(config): fail closed on invalid configuration` | COR-02. |
| 8 | `ff06b2a docs(audit): record validated remediation progress` | Ledger evidence synchronized through the first remediation slices. |
| 9 | `91b3ed5 refactor(setup): give runtime adapters lifecycle ownership` | ARC-06. |
| 10 | `580954c refactor(lints): restore strict workspace Clippy baseline` | REP-05, completing the policy introduced by `018b74a`. |
| 11 | `e56d3f0 refactor(packet): extract shared wire contracts` | ARC-01 shared packet-contract portion. |
| 12 | `7201ddc perf(ingest): remove measured parser allocations` | PER-06 diagnostics/LCOV portion. |
| 13 | `3b2dae7 refactor(diff): expose typed library errors` | API-01 diff portion; DOC-02 typed-error documentation portion. |
| 14 | `b3ac32f refactor(daemon): extract implementation-free protocol contract` | ARC-01 protocol extraction; TST-03/TST-05 and DOC-02 protocol portions. |
| 15 | `0f708f1 fix(shim): preserve Linux variadic open ABI` | COR-01. |
| 16 | `d2b86f0 perf(testmap): persist and plan sparse bitmap coverage` | PER-05 implementation and TST-03 planner-property portion. |
| 17 | `153a3e3 bench(testmap): record sparse planning scale evidence` | PER-05 controlled before/after evidence. |
