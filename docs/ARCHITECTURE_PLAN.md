# Packet28 Architecture Plan

This document describes the current module boundaries after the codebase-health
refactor. It is an architecture map and follow-up plan, not a claim that every
runtime path has been benchmarked or revalidated.

## Current Boundaries

### CLI and reducers

- `crates/suite-cli/src/cli_defs.rs` defines the top-level CLI surface, while
  `cli_runtime.rs` resolves configuration and dispatches commands.
- `crates/packet28-reducer-core` owns deterministic command classification and
  output reduction.
- `route_registry.rs` coordinates route decisions. `route_registry_native.rs`
  owns compact native-tool planning and `route_registry_policy.rs` owns
  workspace policy and wrapper exclusions. `cmd_run.rs` executes the selected
  route and records reduction results and fallback provenance.

### MCP

- `cmd_mcp.rs` remains the MCP command, server-session, and request-dispatch
  facade.
- Protocol framing, response shaping, prompt/resources, smoke checks, shared
  support, tool arguments, and the tool catalog live in the corresponding
  `cmd_mcp_{transport,response,prompt_resource,smoke,support,tool_args,tool_catalog}.rs`
  modules.
- Core and memory dispatch are separated into `cmd_mcp_core_tools.rs` and
  `cmd_mcp_memory_tools.rs`.
- Native tools are split by dispatch, arguments, artifacts, search, reads, FFF,
  and handoff behavior in the `cmd_mcp_native*.rs` modules.
- Upstream proxy configuration, catalog handling, and upstream calls are split
  across `cmd_mcp_proxy*.rs`; the FFF client is in `cmd_mcp_fff.rs`.

### Hooks and setup

- `cmd_hook.rs` owns hook CLI/event orchestration. Runtime normalization, packet
  construction, reducer execution, HTTP serving, and shared helpers live in
  `cmd_hook_runtime.rs`, `cmd_hook_packets.rs`, `cmd_hook_runner.rs`,
  `cmd_hook_http.rs`, and `cmd_hook_support.rs`.
- `cmd_setup.rs` coordinates setup. Command resolution, hook writers, plugin
  installation, index verification, rendering, and runtime detection live in
  the `cmd_setup_{commands,hooks,plugins,index,render,runtime}.rs` modules.
- `agent_surface.rs` and `runtime_integrations.rs` contain generated agent
  guidance and runtime-specific integration material rather than setup control
  flow.

### Dashboard and memory

- `cmd_dashboard.rs` assembles dashboard data and anomaly summaries;
  `cmd_dashboard_render.rs` owns text, HTML, and interactive rendering.
- `memory_store.rs` is the memory API facade and primary memory CRUD/recall
  implementation. Database setup, data types, scoring, local hook/extraction
  storage, feedback/transcripts, graph storage, graph rendering, project
  scanning, and linting are separated into the `memory_*.rs` modules.
- `savings_analytics.rs` owns local run-savings records consumed by gain,
  discover, and dashboard views.

### System commands

- `cmd_system.rs` owns arguments and command-level orchestration.
- Dependency summaries, JSON filtering, output envelopes, search/find, source
  reads, and command summaries live under `cmd_system/`.

### Persistent search index

- `packet28-search-core/src/lib.rs` is a documented compatibility facade. It
  exposes only the typed error/result, manifest/runtime, lifecycle entrypoints,
  query entrypoints, and the feature-gated shared-scan API.
- `model.rs` owns public and persisted formats plus immutable loaded-generation
  views; `postings.rs` owns gram derivation and the validated posting codec;
  `layer.rs` owns repository scanning, layer construction, mmap loading, and
  artifact validation; `query.rs` owns pure planning, candidate selection, and
  source verification; `generation.rs` owns writer serialization, publication,
  recovery, and retention; `paths.rs` owns repository-relative and on-disk
  layout rules; `support.rs` owns narrow shared error/context helpers.
- Generation publication depends on the immutable model view and never on query
  orchestration. Shared-scan composition imports implementation entrypoints
  from their owning modules rather than reaching back through the public facade.
- `tests/module_architecture.rs` parses the Rust source to lock the facade export
  set, reviewed file inventory and size ceilings, explicit acyclic dependency
  graph, owning-module imports, generated-source provenance, and the absence of
  production wildcard imports. External API tests compile every facade
  entrypoint and exercise manifest JSON, lifecycle, reducer-search, and
  shared-scan parity contracts.

### Daemon and tests

- `crates/packet28-daemon-protocol` owns implementation-free wire DTOs,
  framing, and endpoint paths. `crates/packet28-daemon-core` owns typed storage,
  integrity, leases, recovery, and retention plus a frozen `0.2.x`
  compatibility facade.
- `crates/packet28d/src/application.rs` owns the server lifecycle and
  `packet28d::serve`; `src/main.rs` is only the CLI/exit adapter. Broker
  context, handoff, search, rendering, limits, snapshots, and writes live under
  `src/broker/` behind an explicit crate-internal facade. Hooks, indexing,
  launch, planning, runtime files, server dispatch, state, and watches remain
  separate modules.
- [Daemon runtime](daemon-runtime.md) records startup/shutdown order,
  cancellation, transport, persistence/recovery, compatibility/errors, and the
  reviewed happy-path example inventory and exclusions.
- Tokio is confined to the `packet28d`/`suite-cli` process-orchestration
  boundary. TCP and Unix listeners share one cancellation signal and owned
  connection task set; framing has separate header, body, and write deadlines.
  CPU-heavy parsing/serialization and synchronous kernel, repository, index,
  SQLite, and filesystem work cross a bounded blocking executor, while their
  existing Rayon data parallelism remains unchanged.
- Daemon subscriber and watch ingress queues are bounded. A subscriber that
  falls behind is disconnected after its queued frames drain and resumes from
  its last sequence through `TaskSubscribe.after_seq`; watch overflow is
  coalesced per watch generation and surfaced on the next `watch_triggered`
  event as `queue_overflowed=true` before conservative replanning.
- Runtime limits have nonzero defaults and can be tuned with
  `PACKET28D_MAX_CONNECTIONS`, `PACKET28D_MAX_PENDING_TCP_AUTHENTICATIONS`,
  `PACKET28D_MAX_BLOCKING_OPERATIONS`,
  `PACKET28D_SUBSCRIBER_QUEUE_CAPACITY`, `PACKET28D_WATCH_QUEUE_CAPACITY`,
  `PACKET28D_BACKGROUND_QUEUE_CAPACITY`,
  `PACKET28D_FRAME_HEADER_TIMEOUT_MS`, `PACKET28D_FRAME_BODY_TIMEOUT_MS`,
  `PACKET28D_FRAME_WRITE_TIMEOUT_MS`, `PACKET28D_TRANSPORT_AUTH_TIMEOUT_MS`,
  and `PACKET28D_SHUTDOWN_GRACE_MS`.
- `cmd_daemon.rs` defines the CLI surface; `cmd_daemon_client.rs` and
  `cmd_daemon_commands.rs` own client transport/lifecycle and command handlers.
- Large route-registry and hook unit suites are in
  `route_registry_tests.rs` and `cmd_hook_tests.rs`. Packet28d tests are split
  by broker behavior under `crates/packet28d/src/tests/`, and CLI integration
  tests are organized by workflow under `crates/suite-cli/tests/` with shared
  fixtures in `tests/support/`.

## Reliability Fallbacks to Retain

These paths protect correctness, portability, or graceful degradation and are
not cleanup targets without an equivalent replacement and focused tests:

- raw or proxy passthrough when a route is excluded by policy or cannot be
  safely rewritten;
- raw execution and artifact recovery for unsupported reducer families, with
  explicit fallback provenance in savings/tool results;
- post-tool hook capture when pre-tool rewriting is unavailable or unsupported;
- guidance-file setup for runtimes without usable MCP integration;
- local reducer/search behavior when the daemon or preferred search backend is
  unavailable, with the selected backend or fallback reason surfaced;
- transcript LIKE fallback when SQLite FTS cannot satisfy the query.

Compatibility aliases and legacy data readers may also be reliability
boundaries. Verify their callers, persisted-data role, and tests before removal.

## Remaining Cleanup Targets

- Continue reducing the size of orchestration facades such as `cmd_mcp.rs`,
  `cmd_setup.rs`, `cmd_dashboard.rs`, `cmd_system.rs`, `memory_store.rs`, and
  `packet28d/src/application.rs` when a cohesive responsibility can be
  extracted. Keep `packet28d/src/main.rs` as the shallow CLI and process-exit
  adapter enforced by `scripts/check_architecture.py`.
- Narrow broad internal re-exports and remove `allow(dead_code)` exceptions only
  after static reference checks and targeted runtime tests show they are
  unnecessary.
- Split remaining large implementation or test modules by behavior when doing so
  improves navigation without adding forwarding-only abstraction layers.
- Audit compatibility aliases, old runtime adapters, and legacy readers
  separately from fallback-preservation work. Uncertain paths should be
  documented and isolated rather than removed opportunistically.

## Verification Strategy

Each refactor batch should use the smallest relevant unit and E2E tests, then
run `scripts/validate_refactor_batch.sh`. Major checkpoints require:

```text
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

MCP boundaries additionally need initialize/tools-list/tools-call stdio tests;
setup and hook boundaries need generated-config and rewrite E2E tests; daemon
boundaries need lifecycle, task, handoff, index, and disconnect coverage.

RTK parity remains centered on route selection, hook rewriting, reducer-aware
execution, and raw artifact recovery. ICM parity remains centered on local
memory, MCP/hooks, dashboard signals, and daemon-backed context/handoff
assembly. Broader parity work should be tracked separately from structural
cleanup.
