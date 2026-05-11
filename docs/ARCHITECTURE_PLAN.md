# Packet28 Architecture Plan

This plan preserves the current crate layout while moving toward the architecture target in `docs/packet28_goal.md`.

## Current Mapping

| Target module | Current implementation |
|---|---|
| `packet28-core` | `suite-packet-core` for `EnvelopeV1`, registry, diagnostics, token/budget packet fields. |
| `packet28-reducers` | `packet28-reducer-core` for command reducer families and structured reductions. |
| `packet28-rewrite` | `suite-cli/src/route_registry.rs` plus `cmd_compact.rs` rewrite rendering. |
| `packet28-agent` | `suite-cli/src/cmd_setup.rs`, `cmd_hook.rs`, `cmd_run.rs`, and `agent_surface.rs`. |
| `packet28-mcp` | `suite-cli/src/cmd_mcp.rs` and `cmd_mcp_native.rs`. |
| `packet28-memory` | `context-memory-core` packet cache and recall indexes; SQLite MVP is missing. |
| `packet28-daemon` | `packet28d` plus `packet28-daemon-core`. |
| `packet28-cli` | `suite-cli`. |

## Refactor Principles

1. Preserve existing crate names and CLI compatibility until tests and docs are green.
2. Add narrow facades before moving code across crate boundaries.
3. Keep setup, doctor, MCP smoke tests, and reducer tests passing after each feature commit.
4. Make SQLite memory additive; do not migrate or remove the existing bincode packet cache in this slice.
5. Treat Windsurf command rewrite as guidance-only until command interception is proven.

## Incremental Plan

### 1. Runtime Config Inspection

Create small read-only helpers for runtime MCP/rules config inspection and reuse them from setup, doctor, and smoke-test code.

Risky files:

- `crates/suite-cli/src/cmd_setup.rs`
- `crates/suite-cli/src/cmd_doctor.rs`
- `crates/suite-cli/src/cmd_mcp.rs`
- `crates/suite-cli/src/agent_surface.rs`

### 2. Windsurf Verification

Add:

- `Packet28 doctor --agent windsurf --root .`
- `Packet28 mcp smoke-test --from-config windsurf`
- tests for fresh home, existing config preservation, invalid JSON refusal, paths with spaces, missing binary, idempotency, and generated MCP handshake.

### 3. Rewrite UX

Expose the existing route planner through top-level `Packet28 rewrite "<command>"`. Keep reducer implementations in `packet28-reducer-core`.

### 4. RTK-Style Run and Analytics

Make `Packet28 run <command...>` execute supported commands through reducer-aware capture and persist savings. Keep backend launcher behavior compatible by using an explicit backend flag or submode where needed.

### 5. SQLite Memory MVP

Add local SQLite storage under `~/.packet28/packet28.db` with tables:

- `events`
- `commands`
- `reductions`
- `memories`
- `memory_chunks`
- `concepts`
- `relations`
- `feedback`
- `agent_sessions`
- `mcp_calls`

Expose CLI and MCP memory, feedback, and graph tools over this additive store.

### 6. Dashboard

Implement a local-only `dashboard` or `serve --port 2828` command showing savings, sessions, memory, feedback, graph counts, MCP calls, and Windsurf doctor status.

## Current Test Baseline

The architecture subagent ran:

```bash
cargo test -q -p packet28-reducer-core -p packet28-search-core -p suite-packet-core
```

Result: passed (`76`, `21`, and `12` tests reported across those packages, plus zero-test targets).

Before implementation proceeds, the current workspace also needs a compile check because `crates/suite-cli/src/cmd_setup.rs` contains an apparent duplicated `let hook_present = new_entries` line in the Windsurf hook writer.
