# Packet28 Full RTK + ICM Parity Goal

## Objective

Turn Packet28 into an end-to-end local AI coding context product with full Packet28-native parity against:

- RTK: https://github.com/rtk-ai/rtk
- ICM: https://github.com/rtk-ai/icm

Packet28 should not become a source clone. It should match the useful behavior and user-facing capability of RTK and ICM using Packet28 packets, schemas, daemon, MCP server, reducers, local storage, runtime hooks, and editor integrations.

The product thesis:

Packet28 is the local context operating system for coding agents:

- compress what agents see
- remember what matters
- expose context through MCP
- prove every editor integration works
- show local savings, sessions, memory, and integration health

## Current Reality

Packet28 is useful today as a context-reduction, search, and runtime helper. It is not yet a magic replacement for all agent workflows.

Some queries and workflows will still be better handled by native tools, especially when the agent needs exact raw output, broad exploratory control, or tool-specific behavior that Packet28 has not modeled yet. Packet28 needs more repeated real-world runs across different repositories, agents, and task shapes before it should be called fully mature.

Do not claim full RTK or ICM parity until the claim is backed by:

- current source inspection of both upstream repos
- a complete parity matrix in `docs/PARITY.md`
- passing targeted tests for each claimed equivalent
- real-world experiment artifacts showing measured utility
- release smoke evidence after publishing

## Reference Sources

Before changing Packet28 code, inspect the current upstream sources:

1. RTK repo: https://github.com/rtk-ai/rtk
   - Identify CLI proxy behavior, command rewriting, reducer surface, hooks, supported commands, savings analytics, session/discover behavior, config, supported agents, tests, and architecture.

2. RTK docs: https://www.rtk-ai.app/docs/
   - Extract user-facing commands, setup flows, support promises, and product expectations.

3. ICM repo: https://github.com/rtk-ai/icm
   - Identify memory model, SQLite schema, FTS/vector search, MCP tools, graph/memoir model, feedback tools, init flow, supported editors, tests, and architecture.

4. ICM product page: https://www.rtk-ai.app/icm/
   - Extract user-facing memory, graph, feedback, MCP, and dashboard expectations.

Use these sources to derive a parity contract. Do not copy code blindly.

## Full Parity Definition

Full parity means every meaningful RTK and ICM capability is either implemented in Packet28, documented as implemented differently with equivalent user value, or explicitly accepted as a non-goal.

### RTK Parity Areas

Packet28 must cover:

- CLI proxy and command wrapper behavior through `packet28 run <command...>` and runtime hooks installed by `Packet28 setup`.
- Command rewrite planning through `packet28 rewrite "<command>"`, hook rewrites, and MCP/runtime equivalents.
- Output reducers that emit compact human output, structured packets, raw artifact recovery pointers, token estimates, savings percent, provenance, exit code, and fallback reason.
- Supported command families: git, cargo, npm, pytest, grep/rg, find, ls/tree, cat/head/tail, docker, and GitHub CLI.
- Savings analytics through `packet28 gain`, missed-savings discovery through `packet28 discover`, and session analytics through `packet28 session`.
- Configurable filtering/reduction policy, including any upstream RTK custom filter or config concepts that materially affect user workflows.
- Agent/editor integrations for Claude, Cursor, Codex, Windsurf, OpenCode, Gemini, Copilot, Cline, Roo, Kilo, and Antigravity where supported by upstream expectations.
- Safe fallback behavior that preserves original exit codes, does not hide critical errors, and records why reduction was skipped.
- Local-first privacy behavior with no telemetry, signup, cloud dependency, or API key requirement for the core product.

### ICM Parity Areas

Packet28 must cover:

- Local permanent memory through `packet28 memory store`, `packet28 memory recall`, `packet28 memory list`, and MCP equivalents.
- Topics, importance, timestamps, source metadata, decay fields, and recall scoring.
- Memory consolidation through `packet28 memory consolidate`.
- Local SQLite storage at `~/.packet28/packet28.db`, with FTS/BM25 search and a vector-ready schema.
- Knowledge graph and memoir-style behavior through `packet28 graph create`, `packet28 graph add-concept`, `packet28 graph link`, and `packet28 graph inspect`.
- Typed relations such as `depends_on`, `part_of`, `refines`, `contradicts`, and `supersedes`.
- Feedback and correction loop through `packet28 feedback record`, `packet28 feedback search`, and `packet28 feedback stats`.
- Transcript, command, reduction, agent session, and MCP-call history sufficient to support recall, audit, and dashboard views.
- Wake-up/context-pack extraction through `packet28 wakeup`.
- MCP tools for search, reduce, rewrite, memory, feedback, graph, handoff, and doctor operations.
- Local dashboard or TUI showing savings, missed savings, sessions, memory, graph, feedback, MCP activity, and integration health.

## Workstreams

### 1. Source-Backed Parity Audit

Create or update `docs/PARITY.md` so it is the source of truth for parity.

The matrix must include:

| Upstream | Capability | Command/API/tool | Upstream source files/docs | Packet28 equivalent | Status | Test or evidence |
|---|---|---|---|---|---|---|

Rules:

- Clone or inspect current `rtk-ai/rtk` and `rtk-ai/icm` before updating the matrix.
- Mark each item as `implemented`, `partial`, `missing`, or `non-goal`.
- Include a test name, experiment artifact, or explicit implementation issue for every item.
- Remove any language that implies Packet28 is at full parity when only the current acceptance slice is covered.

### 2. RTK Runtime And Reduction Parity

Implement or complete:

- `packet28 rewrite "<command>"`
- `packet28 run <command...>`
- runtime hook rewrites installed by `Packet28 setup`
- `packet28 gain`
- `packet28 discover`
- `packet28 session`
- reducer coverage for the RTK command families listed above
- raw-output artifact capture and recovery through Packet28 artifact IDs
- fallback recording for every unsupported or unsafe command

Acceptance tests must prove:

- failing exit codes are preserved
- critical errors are not hidden
- unsupported commands pass through safely
- raw output remains recoverable
- compact output is materially smaller than raw output for noisy commands
- hook-driven operation works where Packet28 claims transparent runtime support

### 3. ICM Memory, Graph, And Feedback Parity

Implement or complete:

- SQLite-backed memory tables for events, commands, reductions, memories, memory chunks, concepts, relations, feedback, agent sessions, and MCP calls
- FTS/BM25 recall and vector-ready schema fields
- CLI and MCP memory store/recall/list/consolidate flows
- graph creation, linking, relation typing, and inspection
- feedback recording, search, stats, and application counts where useful
- transcript/session capture that feeds recall and dashboard views
- `packet28 wakeup` for context-pack extraction

Acceptance tests must prove:

- memory recall works through CLI and MCP
- feedback is searchable and summarized
- graph relations are inspectable
- consolidation does not erase important source metadata
- wake-up packs contain relevant recent context and durable memories

### 4. Integration, Setup, And Doctor Reliability

Implement or harden:

- `packet28 init --agent <agent>`
- `packet28 init --mode all`
- `packet28 setup --runtime <agent> --yes`
- `packet28 doctor --agent <agent> --root .`
- `packet28 mcp smoke-test --from-config <agent>`

For Windsurf, do not claim full transparent rewrite/hook support unless a real test proves command interception.

Windsurf acceptance bar:

- MCP config works
- rules are installed
- existing MCP servers are preserved
- generated config completes MCP initialize and tools/list
- `packet28 doctor --agent windsurf --root .` passes in a clean fixture
- Packet28 clearly reports rewrite support as `guaranteed`, `best-effort`, or `rules-only/guidance-only`

### 5. Real-World Experiment Harness

Build a repeatable experiment suite that runs Packet28 against real repositories and task types.

Required experiment dimensions:

- repositories: Packet28 itself plus at least two unrelated non-trivial repos
- workflows: search, code review, failing test triage, implementation, docs lookup, and handoff/bootstrap
- comparisons: native tool path versus Packet28 path
- metrics: raw estimated tokens, compact estimated tokens, savings percent, fallback count, indexed-search hit rate, elapsed time, failed commands, and raw artifact recovery success
- artifacts: committed or archived summaries under `docs/experiments/` or another documented evidence path

Minimum maturity gate:

- at least three repeated runs per workflow
- no unexplained daemon/index readiness failures
- broad search uses indexed Packet28 search when claimed
- fallback reasons are explicit
- Packet28 output is sufficient for the task without hiding correctness-critical information

Current evidence:

- `docs/experiments/run_packet28_real_repo_suite.sh` runs the repeatable real-repository suite against clean temporary checkouts of Packet28, ripgrep, and fd.
- `docs/experiments/real-repos/SMOKE_20260511.md` and `docs/experiments/real-repos/SMOKE_20260511.jsonl` record a 3x run across the required workflows. The `Packet28 run --json` path had 0 fallbacks, 0 failed commands, and raw artifact recovery for 54/54 runs.
- The latest run had 9/9 indexed `p28` hits and 0 indexed-search readiness fallbacks after `p28` was changed to wait for daemon index readiness before falling back.
- Evaluate `fff` (`dmtrKovalenko/fff`) as a future search backend or optional MCP peer. Packet28 now has an opt-in `p28 --engine fff` MCP adapter, and local fake-MCP plus real upstream `fff-mcp` smoke tests show Packet28-shaped `engine=fff_mcp` output works. Full adoption still needs repeated real-repo comparisons that prove the adapter preserves Packet28 reducer packets, raw artifact recovery, fallback reasons, local-first daemon semantics, and parity evidence while improving search behavior.

### 6. Release Readiness

Before tagging a release that claims parity progress:

- run focused unit/integration tests for setup, MCP, doctor, reducers, search, memory, graph, and feedback
- run `cargo test --workspace` when feasible
- run `cargo clippy --workspace --all-targets -- -D warnings` when feasible
- update `docs/PRODUCT.md`, `docs/ARCHITECTURE_PLAN.md`, and `docs/PARITY.md` if behavior or support tiers changed
- publish only after CI passes
- verify the npm package or install path after release
- record the release smoke evidence

## Architecture Target

Refactor incrementally toward these boundaries where they reduce complexity:

- `packet28-core`: schemas, `EnvelopeV1`, token estimates, packet formats
- `packet28-reducers`: RTK-style command output reducers
- `packet28-rewrite`: command classification and rewrite decisions
- `packet28-agent`: Claude, Cursor, Codex, Windsurf, OpenCode, Gemini, and other agent adapters
- `packet28-mcp`: MCP tools and stdio server
- `packet28-memory`: SQLite memory, recall, feedback, graph, and wake-up packs
- `packet28-daemon`: indexer, event capture, local API
- `packet28-cli`: init, doctor, setup, run, gain, discover, session, dashboard

If the current workspace uses different crate names, preserve compatibility and refactor incrementally instead of renaming everything at once.

## Commit Rules

Commit per coherent feature. Do not create one giant commit.

Suggested sequence:

1. `docs: define full RTK and ICM parity gates`
2. `docs: update source-backed parity matrix`
3. `test: add setup and MCP smoke coverage`
4. `fix: harden editor setup and doctor checks`
5. `feat: complete command rewrite planner`
6. `feat: complete runtime reducer coverage`
7. `feat: add savings and session analytics`
8. `feat: complete SQLite memory recall`
9. `feat: expose memory and feedback over MCP`
10. `feat: add graph and wakeup flows`
11. `feat: add local dashboard`
12. `test: add real-world Packet28 experiment harness`
13. `docs: update product usage and support tiers`

Every commit must compile, include tests when behavior changes, avoid unrelated formatting churn, and avoid mixing refactor with behavior unless unavoidable.

## Definition Of Done

Packet28 can be described as fully mature and at full RTK + ICM parity only when:

1. `docs/PARITY.md` maps every meaningful current RTK and ICM capability to a Packet28 equivalent, accepted non-goal, or tracked missing item.
2. All claimed RTK runtime, reducer, rewrite, analytics, and session features have tests.
3. All claimed ICM memory, graph, feedback, recall, wake-up, and MCP features have tests.
4. Setup, doctor, and MCP smoke tests pass for every claimed supported agent.
5. Windsurf support is labeled honestly and tested according to its actual interception capability.
6. Real-world experiment artifacts show repeated Packet28 utility across multiple repos and workflows.
7. Raw output recovery works for reduced commands.
8. Fallback reasons are visible and actionable.
9. CI passes.
10. Release smoke verifies the published package or install path.
11. Docs explain exact usage, support tiers, limitations, and remaining non-goals.

Until those gates pass, describe Packet28 as useful but still maturing.

## Non-Goals And Constraints

- Do not add cloud/team analytics yet.
- Do not add telemetry.
- Do not require signup.
- Do not require API keys for core local behavior.
- Do not break Claude, Cursor, Codex, Windsurf, or existing setup behavior while adding parity.
- Do not casually change packet schemas.
- Do not claim full transparent hook support for an editor without proof.
- Do not copy RTK or ICM code blindly.
- Do not hide failures behind pretty output.

## Progress Reporting

Work in checkpoints. After each checkpoint, summarize:

- files changed
- behavior added
- tests run
- commits made
- current failure, if any
- next checkpoint

## Reusable Goal Prompt

```text
/goal Update Packet28 until it can honestly claim full Packet28-native parity with RTK and ICM. Use docs/packet28_goal.md as the roadmap. First inspect current https://github.com/rtk-ai/rtk and https://github.com/rtk-ai/icm source and docs, then update docs/PARITY.md with a complete source-backed matrix. Implement missing RTK runtime/reducer/rewrite/analytics/session behavior and missing ICM memory/graph/feedback/wakeup/MCP/dashboard behavior using Packet28-native architecture. Add tests and repeated real-world experiment artifacts before making maturity claims. Preserve local-first privacy, leave unrelated dirty files untouched, commit per coherent feature, and do not claim full parity until the Definition Of Done in docs/packet28_goal.md is satisfied.
```
