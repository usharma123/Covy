# Packet28 Agentic Context Ideas

This note tracks high-leverage ideas that go beyond RTK/ICM parity and are specific to making coding agents spend less context while making better next-step decisions.

## Priority Ideas

| Idea | Agent benefit | First implementation slice | Evidence gate |
|---|---|---|---|
| Plan-to-test map validation | Prevents agents from claiming weak verification after edits. | Extend broker `validate_plan` to score test gates by mapped impact coverage, broad-suite fallback, and missing mappings. | Unit tests now cover mapped, broad, and missing-mapping cases; stale test-map detection and one real-repo plan validation artifact remain. |
| Context ROI ledger | Lets agents prefer commands and memories that historically saved the most tokens for the current repo. | Record per-tool compact/raw token delta, reuse count, and downstream follow-up count in local SQLite. | `Packet28 gain --json` exposes ROI by route; dashboard shows top saved context sources. |
| Evidence freshness scoring | Stops stale reads/searches from anchoring current decisions after edits. | Add freshness metadata to broker evidence sections and downrank cached snippets for changed paths. | Broker tests proving changed files demote older evidence and preserve fresh tool results. |
| Hypothesis checkpoints | Helps agents carry explicit assumptions across handoff without replaying long transcripts. | Add `packet28 hypothesis add/confirm/reject` and MCP aliases backed by local memory/events. | Handoff brief contains active hypotheses with state and evidence links. |
| Failure fingerprint memory | Turns repeated build/test failures into reusable local advice. | Hash normalized failure summaries, count repeats, and attach successful fix command or file edits when observed. | Failing-test triage experiment shows the matching fingerprint and suggested prior fix. |
| Scoped wake-up packs | Reduces startup context by matching memory to the requested path/symbol/task type instead of broad project recall. | `Packet28 wakeup` and MCP wakeup now accept path, symbol, and intent filters; scoped JSON and rendered packs prune irrelevant candidates. | Evidence: `test_wakeup_scopes_context_by_path_symbol_and_intent`; broader `cargo test -p suite-cli wakeup` also preserves SessionStart wake-up injection. |
| Agent action critic | Catches risky next actions before execution, not after output reduction. | Broker `choose_tool` emits warnings for destructive commands, broad searches, irrelevant tests, and missing read-before-edit context. | Hook/MCP tests verify warnings are emitted while preserving fail-open behavior. |
| Experiment manifest verifier | Prevents maturity claims from drifting away from actual artifacts. | Add a JSON manifest for each experiment suite with commands, versions, metrics, and required gates. | `Packet28 verify experiments` fails on missing raw artifacts, fallback reasons, or uncovered workflows. |

## Research Rules

- Prefer local deterministic signals before LLM calls: coverage, test maps, git status, reducer stats, memory FTS/vector scores, and task state.
- Every new context idea needs a compact-output metric and a correctness metric.
- A feature is not mature until it is exercised through at least one real agent workflow and one deterministic test.
- Warnings should guide agents toward cheaper or safer actions without blocking unless Packet28 has strong local proof.
- Avoid adding schema fields unless the same value is used by CLI, MCP, dashboard, or handoff paths.
