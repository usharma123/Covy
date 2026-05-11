# Packet28 Architecture Plan

Packet28 implements the RTK/ICM parity slice with Packet28-native components:

- `packet28-reducer-core`: deterministic command reducers and compact previews.
- `route_registry.rs`: command classification, rewrite planning, native-tool routing, and fallback reasons.
- `cmd_run.rs`: reducer-aware execution plus platform agent backend preservation.
- `savings_analytics.rs`: local `.packet28/run-savings.jsonl` records for `gain`, `discover`, and dashboard.
- `memory_store.rs`: local SQLite schema for memory, graph, feedback, sessions, and MCP call history.
- `cmd_mcp.rs`: MCP stdio server, compact search/read/glob tools, memory/feedback/graph tools, rewrite/reduce/doctor aliases, and handoff/status tools.
- `cmd_setup.rs`, `cmd_doctor.rs`, `runtime_integrations.rs`: agent setup and validation for Claude, Cursor, Codex, and Windsurf.
- `packet28d` and daemon client modules: task state, handoff assembly, persistent context, and search/index support.

## RTK Mapping

RTK’s core path is command proxy plus hook rewrite. Packet28 maps this to route registry decisions, runtime hooks, reducer-aware `Packet28 run`, and raw artifact recovery through Packet28 artifacts. RTK’s broader custom filter and telemetry systems are deferred.

## ICM Mapping

ICM’s core path is local memory plus MCP and hooks. Packet28 maps this to SQLite memory/feedback/graph commands, `wakeup`, MCP tools, dashboard, and handoff/context assembly. ICM’s vector/FTS/memoir/transcript breadth is deferred.

## Verification Strategy

Every supported feature needs one of:

- focused unit tests for routing/schema/tool-list behavior,
- E2E CLI tests for generated setup or command execution,
- MCP stdio tests for initialize/tools-list/tools-call behavior,
- workspace-level `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
