# Packet28 RTK/ICM Parity Plan

This document is the first checkpoint for `docs/packet28_goal.md`. It records what was verified from the local Packet28 workspace, what was inferred from the goal, and what remains unsupported or deferred.

## Status Legend

- `verified`: implemented locally and has an identified test or command path.
- `partial`: Packet28 has a native equivalent, but the requested product behavior or proof is incomplete.
- `missing`: no local equivalent was found.
- `deferred`: deliberately not in the MVP slice or blocked by missing external proof.

## RTK Feature Matrix

| RTK feature | Packet28 equivalent | Status | Test or proof |
|---|---|---:|---|
| Broad CLI command compression surface | `Packet28 compact ...`, top-level `gain`, `discover`, `run`, `hook`, `setup`, `doctor`, `mcp` in `crates/suite-cli/src/cli_defs.rs` | partial | CLI definitions and reducer tests exist; top-level `rewrite` and `session` aliases still needed. |
| Rewrite planner | Route decisions in `crates/suite-cli/src/route_registry.rs`; `Packet28 compact rewrite` in `cmd_compact.rs` | partial | Route registry tests cover classification; goal still requires `packet28 rewrite "<command>"`. |
| Unsupported-command fallback | `RawPassthrough` reasons for empty commands, shell composition, shell expansion, globs, parse errors, unsupported commands | verified | `route_registry.rs` tests. |
| Git reducers | `packet28-reducer-core` git family | verified | `packet28-reducer-core` tests; architecture agent baseline passed `cargo test -q -p packet28-reducer-core`. |
| Cargo reducers | `packet28-reducer-core` cargo family | partial | Reducer dispatch exists; goal needs `packet28 run` proof and structured savings output. |
| npm/pytest reducers | JavaScript/Python reducer families in `packet28-reducer-core` | partial | Reducer dispatch exists; goal needs `packet28 run` proof. |
| Search/file reducers | `fs` family plus `packet28-search-core` and MCP `search/read_regions/glob` | verified | `packet28-search-core` tests; setup E2E checks indexed search. |
| Docker/GitHub reducers | `docker`, `kubectl`, `gh` reducer families | partial | Reducer dispatch exists; goal needs explicit fixture tests. |
| Analytics | top-level `gain`, `discover`, compact session/discover commands | partial | Existing commands found; local savings analytics must be proven end to end with `packet28 gain` and `packet28 discover`. |
| Hook-first capture | `Packet28 hook` handlers and daemon ingestion | partial | Claude/Cursor/Codex behavior has local tests; Windsurf interception is not proven. |
| Supported agents | Claude, Cursor, Codex, Windsurf setup; OpenCode detected by runtime launcher | partial | Setup tests cover Cursor/Codex/Windsurf artifacts; wider RTK agent list is not implemented. |

## ICM Feature Matrix

No local ICM checkout or docs were found in or near this workspace. ICM requirements below are therefore goal-derived unless otherwise noted.

| ICM feature | Packet28 equivalent | Status | Test or proof |
|---|---|---:|---|
| Local memory store | Existing `context-memory-core` packet cache with recall indexes | partial | `context-memory-core` tests exist, but not SQLite memory CLI. |
| SQLite schema under `~/.packet28/packet28.db` | None found | missing | `rg` found no SQLite implementation beyond the goal doc. |
| Memory store/recall/list/consolidate CLI | `Packet28 context store` and `context recall` are adjacent | partial | Goal requires `packet28 memory ...` commands. |
| Memory MCP tools | Current MCP has context/search/handoff tools | missing | Required `packet28.memory_store` and `packet28.memory_recall` are not exposed. |
| Graph concepts/relations | Recall has graph-overlap scoring concepts, not persisted concept/relation tables | missing | Goal requires `graph create/add-concept/link/inspect`. |
| Feedback record/search | `Packet28 learn` is adjacent, but no feedback table/tool was found | missing | Goal requires `feedback record/search` and MCP `feedback_record`. |
| Wakeup/init lifecycle | Setup, daemon, hook, handoff lifecycle exist | partial | Goal requires `packet28 wakeup` and `packet28 init --agent windsurf`. |
| Local dashboard/TUI | Website/static docs only | missing | Goal requires `dashboard` or `serve --port 2828`. |

## Windsurf Support Tier

Current support tier is **MCP/rules verified target, command rewrite unproven**.

Packet28 currently writes:

- Windsurf MCP config: `~/.codeium/windsurf/mcp_config.json`
- Windsurf rules: `.windsurf/rules/packet28.md`
- Repo-local hook config: `.windsurf/hooks.json`

Packet28 must not claim full Windsurf command interception until a test proves Windsurf actually invokes Packet28 before or after shell commands. The Phase 1 acceptance bar is narrower: setup writes a correct MCP config, doctor validates that config, MCP initialize/tools-list succeeds from the generated config, and the generated rules describe support honestly.

## Unsupported Or Deferred

| Feature | Reason |
|---|---|
| Full Windsurf hook/rewrite parity | Deferred until a real command-interception test exists. |
| RTK full supported-agent breadth | Current product slice starts with Windsurf plus existing Claude/Cursor/Codex behavior. |
| ICM exact schema parity | ICM source/docs were not locally available; implement Packet28-native SQLite MVP from the goal contract. |
| Cloud/team analytics | Explicit non-goal. |
| Telemetry or signup/API-key dependency | Explicit non-goal. |

## Immediate Proof Gaps

1. Add `Packet28 mcp smoke-test --from-config windsurf` and tests that spawn the configured command.
2. Add `Packet28 doctor --agent windsurf --root .` with config, rules, daemon, index, and MCP handshake checks.
3. Add top-level `rewrite` and `session` UX or aliases over the existing compact/rewrite/session implementation.
4. Add additive SQLite memory, feedback, and graph storage without replacing the existing packet cache.
