# Packet28 on claude-code-main

This experiment compares a native shell-code-navigation run against Packet28-mediated runs on the same `claude-code-main` source snapshot.

Latest passing run:

- `fixfinal-20260511T165918Z/summary.md`
- Source: `/Users/utsavsharma/Documents/GitHub/Buns/claude-code-main`
- Isolated repo: `/tmp/packet28-claude-code-main-fixfinal-20260511T165918Z/repo`
- Packet28: `0.2.52`

## Test Task

Find hook, MCP, and tool-use plumbing in `claude-code-main`.

Broad query:

```bash
rg -n 'hook|mcp|tool_use' -g '*.ts' -g '*.tsx' .
```

Indexed control query:

```bash
rg -n --fixed-strings 'tool_use' -g '*.ts' -g '*.tsx' .
```

## Results

| Path | Native tokens | Packet28 tokens | Savings | Notes |
|---|---:|---:|---:|---|
| `Packet28 run -- rg ...` reducer | 1,836,533 | 257 stdout tokens / `0` reducer tokens | ~99.99-100.00% | Same broad native `rg` command, reduced to JSON metadata and summary. |
| `p28 --compact --stats 'hook|mcp|tool_use'` | 1,836,533 | 27,307 | 98.51% | Uses daemon-backed `backend=indexed_regex`; no fallback reason. |
| `p28 --engine indexed --transport inproc tool_use` | 19,778 | 5,638 | 71.49% | Literal indexed-control path used `backend=indexed_regex`. |

Top native-hit files for the broad task:

1. `utils/hooks.ts` - 991 hits
2. `services/tools/toolExecution.ts` - 209 hits
3. `cli/print.ts` - 197 hits
4. `utils/messages.ts` - 186 hits
5. `screens/REPL.tsx` - 165 hits
6. `main.tsx` - 153 hits
7. `services/tools/toolHooks.ts` - 127 hits
8. `services/mcp/config.ts` - 100 hits
9. `commands/plugin/ManagePlugins.tsx` - 96 hits
10. `entrypoints/sdk/coreSchemas.ts` - 87 hits

## Monitoring

The run directory stores:

- `metrics.tsv`: command, exit status, duration, stdout bytes, and estimated stdout tokens.
- `index-poll.tsv`: daemon index readiness polling over the 300-second window.
- `daemon-*.json`: daemon status/index snapshots.
- `*.out` / `*.err` / `*.status`: raw artifacts for every native and Packet28 command.

## Reproduce

```bash
scripts/run_packet28_claude_code_experiment.sh
```

Optional knobs:

```bash
SOURCE_ROOT=/path/to/claude-code-main \
INDEX_TIMEOUT_SECONDS=300 \
scripts/run_packet28_claude_code_experiment.sh
```

## Findings

Packet28 gave a large savings win for the broad search task through the command reducer and compact indexed search output.

The original experiment exposed two implementation bugs, both fixed before the latest passing run:

- Duplicate full-index rebuilds could leave daemon status in `building`; startup and explicit rebuild requests now coalesce while the first full rebuild is in flight.
- `p28 ... .` treated `.` as a literal path filter and broad alternation planning was too conservative; `.` now means repo root, and bounded alternation searches can use the indexed backend.

The latest run verifies both fixes: daemon index readiness reached `true`, broad `p28` used `indexed_regex`, and the fallback reason was `none`.
