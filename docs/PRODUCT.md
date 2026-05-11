# Packet28 Local Context Product Slice

## Usage

Setup Windsurf:

```bash
Packet28 setup --runtime windsurf --yes --root .
Packet28 doctor --agent windsurf --root .
Packet28 mcp smoke-test --from-config windsurf
```

Plan command routing:

```bash
Packet28 rewrite "git status --short" --json
```

Run reducer-aware commands and record savings:

```bash
Packet28 run --root . git status --short
Packet28 run --root . cargo check
Packet28 run --root . npm test
Packet28 run --root . pytest
Packet28 run --root . grep TODO src
Packet28 run --root . docker logs app
Packet28 run --root . gh pr checks 1
```

Inspect local savings and missed savings:

```bash
Packet28 gain --root .
Packet28 discover --root .
Packet28 dashboard --root .
```

Use local memory, feedback, and graph:

```bash
Packet28 memory store "Important local project fact" --tags project
Packet28 memory recall "project fact"
Packet28 feedback record "bad reducer output" "prefer focused summaries"
Packet28 feedback search reducer
Packet28 graph add-concept Packet28
Packet28 graph link Packet28 Reducers --relation uses
Packet28 graph inspect
```

MCP tools exposed by the product slice:

- `packet28.search`
- `packet28.reduce` is represented by reducer-aware `Packet28 run` and will be added as a named MCP tool later.
- `packet28.rewrite` is represented by `Packet28 rewrite` and will be added as a named MCP tool later.
- `packet28.memory_store`
- `packet28.memory_recall`
- `packet28.feedback_record`
- `packet28.graph_inspect`
- `packet28.prepare_handoff`
- `packet28.task_status`
- `packet28.capabilities`

## Support Tiers

| Runtime | Tier | Notes |
|---|---|---|
| Claude Code | Existing hook/MCP support | Existing setup and doctor behavior are preserved. |
| Cursor | Existing MCP/rules/hooks support | Existing setup tests remain in place. |
| Codex | MCP/rules support | Setup writes Codex MCP config without hooks. |
| Windsurf | MCP/rules verified, command rewrite guidance-only | `doctor --agent windsurf` validates config, rules, daemon/index, and MCP initialize/tools-list from generated config. Packet28 does not claim command interception. |

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

## Limitations

- Windsurf hook command interception is not guaranteed and is not claimed.
- Memory recall is keyword/LIKE search for this MVP; vector search can be added later.
- `packet28.reduce`, `packet28.rewrite`, `packet28.handoff`, and `packet28.doctor` named MCP tools remain follow-up aliases over existing CLI/MCP behavior.
- No telemetry, cloud sync, signup, API key, Redis, Qdrant, or Postgres dependency is used.
