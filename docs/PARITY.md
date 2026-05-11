# Packet28 RTK/ICM Parity Audit

Parity references:

- RTK: `https://github.com/rtk-ai/rtk`, cloned at `/private/tmp/rtk-parity`
- ICM: `https://github.com/rtk-ai/icm`, cloned at `/private/tmp/icm-parity`

## Scope

Packet28 is not a line-for-line clone. The implemented scope is Packet28-native parity with the `docs/packet28_goal.md` acceptance slice: reducer-plus-handoff runtime, Windsurf-first setup verification, local analytics/discovery, local SQLite memory, graph/feedback, MCP tools, and dashboard.

Full upstream parity is broader than this slice. RTK includes a large filter catalog, opt-in telemetry, custom TOML filters, broad hook adapters, and release packaging. ICM includes vector/FTS recall, wake-up packs, memoir export/search, transcript storage, hook extraction, web dashboard, cloud/upgrade/import flows, and a larger MCP surface.

## RTK Findings

| RTK area | Upstream evidence | Packet28 status |
|---|---|---|
| Hook-first rewrite | RTK centralizes hook rewrite in `src/hooks/rewrite_cmd.rs`; Claude/Cursor/OpenCode adapters are thin wrappers. | Implemented for Packet28-supported runtimes through setup hooks and runtime guidance; Windsurf command interception remains unclaimed. |
| Command registry | RTK classifies supported/ignored/unsupported commands in `src/discover/registry.rs` and `src/discover/rules.rs`. | Implemented through `route_registry.rs`, `Packet28 rewrite`, and reducer family classification. |
| Reducers | RTK has command modules under `src/cmds/*` plus many TOML filters under `src/filters`. | Implemented for goal-required families: git, cargo/Rust, npm/JS, pytest/Python, grep/rg/find/ls/tree/cat/head/tail, docker/infra, gh. Broader RTK catalog is deferred. |
| Raw recovery | RTK stores raw output hints through tee support in `src/core/tee.rs`. | Implemented through Packet28 artifacts/fetch tools and hook raw-output capture paths. |
| Analytics/discover | RTK uses SQLite tracking in `src/core/tracking.rs`, `rtk gain`, `rtk discover`, and session analytics. | Implemented local run-savings analytics, `Packet28 gain`, `Packet28 discover`, `Packet28 session`, and dashboard. |
| Agent support | RTK documents Claude, Cursor, Codex, Copilot, Gemini, Windsurf, OpenCode, Cline/Roo, Kilo, Antigravity. | Implemented/verified target set is Claude, Cursor, Codex, Windsurf. Wider RTK agent breadth is explicitly deferred. |
| MCP | RTK has no primary MCP server surface. | Packet28 MCP is a product differentiator, not RTK parity. |
| Telemetry/custom filters | RTK has opt-in telemetry and trust-gated TOML filters. | Deferred for this slice. |

## ICM Findings

| ICM area | Upstream evidence | Packet28 status |
|---|---|---|
| SQLite memory | ICM stores local memory in SQLite with FTS/vector support under `crates/icm-store`. | Implemented local SQLite at `~/.packet28/packet28.db` with required goal tables; vector/FTS is deferred. |
| Memory CLI | ICM exposes store/recall/list/forget/update/health/topics/stats/decay/prune/consolidate/embed/wake-up and more in `crates/icm-cli/src/main.rs`. | Implemented goal slice: `memory store/recall/list/consolidate` plus Packet28-native `wakeup`; forget/update/health/topics/decay/prune/embed are deferred. |
| MCP tools | ICM exposes memory, memoir, feedback, transcript, wake-up, learn, and embed tools in `crates/icm-mcp/src/tools.rs`. | Implemented goal-required Packet28 MCP: `search`, `reduce`, `rewrite`, `memory_store`, `memory_recall`, `memory_list`, `feedback_record`, `feedback_search`, `feedback_stats`, `graph_inspect`, `handoff`, `doctor`. Broader ICM tools are deferred. |
| Memoir graph | ICM has memoirs, concepts, typed links, labels, FTS, BFS inspect, and exports. | Implemented simple Packet28 concepts/relations and inspect; full memoir graph is deferred. |
| Feedback | ICM stores prediction/correction/reason/source and FTS search/stats. | Implemented feedback record/search/stats using local SQLite; richer ICM fields/FTS/applied counts are deferred. |
| Wake-up/hooks | ICM has wake-up memory packs and hook extraction lifecycle. | Packet28 implements `wakeup` as a local memory/feedback/graph summary and uses handoff/context assembly plus runtime hooks for agent continuity; ICM's full wake-up extraction lifecycle is deferred. |
| Dashboard | ICM has an Axum/Svelte dashboard. | Implemented local CLI dashboard for acceptance slice; web UI is deferred. |

## Current Conclusion

Packet28 is now at parity with the explicit `docs/packet28_goal.md` acceptance slice, after checking RTK and ICM source directly. It is not full upstream RTK/ICM feature parity; deferred areas are documented above and in `docs/PRODUCT.md`.
