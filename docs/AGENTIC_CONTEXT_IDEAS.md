# Packet28 Agentic Context Ideas

This note tracks high-leverage ideas that go beyond RTK/ICM parity and are specific to making coding agents spend less context while making better next-step decisions.

## Priority Ideas

| Idea | Agent benefit | First implementation slice | Evidence gate |
|---|---|---|---|
| Plan-to-test map validation | Prevents agents from claiming weak verification after edits. | Extend broker `validate_plan` to score test gates by mapped impact coverage, broad-suite fallback, missing mappings, and stale cached testmaps; expose it through MCP as `packet28.validate_plan`. | Unit tests now cover mapped, broad, missing-mapping, stale-testmap, and MCP tool-list surfacing cases; one real-repo plan validation artifact remains. |
| Context ROI ledger | Lets agents prefer commands and memories that historically saved the most tokens for the current repo. | `Packet28 gain --json` now exposes route-level ROI from recorded compact/raw token deltas, and the dashboard surfaces top saved routes across JSON, text, HTML, and TUI output. | Evidence: `test_run_reduces_git_status`; `test_dashboard_shows_local_product_metrics`; future slices should persist downstream follow-up counts. |
| Evidence freshness scoring | Stops stale reads/searches from anchoring current decisions after edits. | Broker edit/inspect context now emits an `evidence_freshness` section for paths and symbols changed since checkpoint, distinguishing fresh reads from evidence that should be refreshed. | Evidence: `broker_edit_context_surfaces_evidence_freshness_for_changed_paths`; future slices should downrank cached snippets directly. |
| Hypothesis checkpoints | Helps agents carry explicit assumptions across handoff without replaying long transcripts. | Add `packet28 hypothesis add/confirm/reject` and MCP aliases backed by local memory/events. | Handoff brief contains active hypotheses with state and evidence links. |
| Failure fingerprint memory | Turns repeated build/test failures into reusable local advice. | Failed `Packet28 run` records now include a deterministic `failure_fingerprint`, and `Packet28 gain --failures` surfaces that key with repeat counts for recurring failure triage. | Evidence: `test_gain_reports_failed_and_fallback_runs`; future slices should attach successful fix commands or file edits. |
| Scoped wake-up packs | Reduces startup context by matching memory to the requested path/symbol/task type instead of broad project recall. | `Packet28 wakeup` and MCP wakeup now accept path, symbol, and intent filters; scoped JSON and rendered packs prune irrelevant candidates. | Evidence: `test_wakeup_scopes_context_by_path_symbol_and_intent`; broader `cargo test -p suite-cli wakeup` also preserves SessionStart wake-up injection. |
| Agent action critic | Catches risky next actions before execution, not after output reduction. | Broker `choose_tool` and `edit` contexts now emit an `action_critic` section for missing tool intent, destructive command shapes, broad unscoped searches, missing edit scope, and read-before-edit gaps on focused paths. | Evidence: `choose_tool_action_critic_flags_missing_intent_and_risky_commands`; `edit_action_critic_flags_missing_scope_and_unread_paths`; future slices should add hook/MCP surfacing. |
| Experiment manifest verifier | Prevents maturity claims from drifting away from actual artifacts. | Add a JSON manifest for each experiment suite with commands, versions, metrics, and required gates. | `Packet28 verify experiments` fails on missing raw artifacts, fallback reasons, or uncovered workflows. |

## Research Rules

- Prefer local deterministic signals before LLM calls: coverage, test maps, git status, reducer stats, memory FTS/vector scores, and task state.
- Every new context idea needs a compact-output metric and a correctness metric.
- A feature is not mature until it is exercised through at least one real agent workflow and one deterministic test.
- Warnings should guide agents toward cheaper or safer actions without blocking unless Packet28 has strong local proof.
- Avoid adding schema fields unless the same value is used by CLI, MCP, dashboard, or handoff paths.
