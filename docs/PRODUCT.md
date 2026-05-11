# Packet28 Local Context Product Slice

## Usage

Setup Windsurf:

```bash
Packet28 init --agent windsurf --yes --root .
Packet28 init --mode all --yes --root .
Packet28 setup --runtime windsurf --yes --root .
Packet28 doctor --agent windsurf --root .
Packet28 mcp smoke-test --from-config windsurf
```

Plan and run reducer-aware commands:

```bash
Packet28 rewrite "git status --short" --json
Packet28 run --root . git status --short
Packet28 run --root . cargo check
Packet28 run --root . npm test
Packet28 run --root . pytest
Packet28 run --root . grep TODO src
Packet28 run --root . docker logs app
Packet28 run --root . gh pr checks 1
```

Inspect savings and missed savings:

```bash
Packet28 gain --root .
Packet28 discover --root .
Packet28 session --root .
Packet28 dashboard --root .
```

Use local memory, feedback, and graph:

```bash
Packet28 memory store "Important local project fact" --tags project
Packet28 memory recall "project fact"
Packet28 memory list
Packet28 memory consolidate
Packet28 wakeup --query project --json
Packet28 feedback record "bad reducer output" "prefer focused summaries"
Packet28 feedback search reducer
Packet28 feedback stats
Packet28 graph create
Packet28 graph add-concept Packet28
Packet28 graph link Packet28 Reducers --relation uses
Packet28 graph inspect
```

MCP tools exposed by the product slice:

- `packet28.search`
- `packet28.reduce`
- `packet28.rewrite`
- `packet28.memory_store`
- `packet28.memory_recall`
- `packet28.memory_list`
- `packet28.feedback_record`
- `packet28.feedback_search`
- `packet28.feedback_stats`
- `packet28.graph_inspect`
- `packet28.prepare_handoff`
- `packet28.handoff`
- `packet28.doctor`
- `packet28.task_status`
- `packet28.capabilities`

## Support Tiers

| Runtime | Tier | Notes |
|---|---|---|
| Claude Code | Hook/MCP support | Setup and tests cover Claude hook config behavior. |
| Cursor | MCP/rules/hooks support | Setup tests cover Cursor artifacts. |
| Codex | MCP/rules support | Setup writes Codex MCP config without claiming transparent shell interception. |
| Windsurf | MCP/rules verified, command rewrite guidance-only | Doctor validates config, rules, daemon/index, and MCP initialize/tools-list from generated config. Packet28 does not claim command interception. |

## Local Storage

Packet28 stores memory, feedback, and graph data locally at:

```text
~/.packet28/packet28.db
```

The SQLite schema includes:

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

Reducer run savings are stored repo-locally under `.packet28/run-savings.jsonl`.

## Explicit Deferrals

- Full RTK command catalog, custom TOML filters, telemetry, and release packaging.
- Full ICM vector/FTS recall, wake-up extraction lifecycle, memoir export/search, transcript replay, cloud/import/upgrade, and web dashboard.
- Windsurf hook command interception until a real runtime test proves it.
