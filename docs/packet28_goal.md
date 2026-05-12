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
- Use `fff` (`dmtrKovalenko/fff`) instead of rebuilding the search primitive where it is available. Packet28 has an opt-in `p28 --engine fff` MCP adapter, and local fake-MCP plus real upstream `fff-mcp` smoke tests show Packet28-shaped `engine=fff_mcp` output works. `docs/experiments/search-backends/FFF_COMPARE_20260511/summary.md` adds 81 repeated real-repo comparison rows across native `rg`, default `p28`, and `p28 --engine fff`. `p28 --engine auto` selects `fff-mcp` for broad/common-literal planner fallbacks when `fff-mcp` is available, and `P28_FFF_AUTO=prefer` makes auto mode try `fff-mcp` first before falling back to Packet28's native indexed/reducer path. Full adoption still needs broader comparisons that prove the adapter preserves Packet28 reducer packets, raw artifact recovery, fallback reasons, local-first daemon semantics, and parity evidence while improving search behavior. Evidence: `p28_fff_engine_adapts_mcp_grep_results`, `p28_auto_uses_fff_for_broad_index_fallback_when_available`, and `p28_auto_can_prefer_fff_when_configured`.
- RTK system-command coverage now includes top-level `Packet28 read`, `Packet28 summary`, `Packet28 find`, `Packet28 grep`, `Packet28 json`, `Packet28 deps`, `Packet28 env`, and `Packet28 log`: compact file reading with line windows and lightweight language-aware filtering, command-output summary detection with exit-code preservation, compact find output for common native and Packet28 find syntax, grouped grep output with truncation and file-type filters, JSON compaction/schema rendering with non-JSON extension rejection, dependency manifest summaries for Cargo/package.json/requirements.txt/pyproject.toml/go.mod, categorized environment output with secret masking by default, and log deduplication that normalizes timestamps, IDs, large numbers, hex values, and paths. `Packet28 grep --engine fff` reuses the existing `p28 --engine fff` / `fff-mcp` adapter, so fff remains one optional search backend rather than a second bespoke implementation. Evidence: `cmd_system::tests::*`, `test_system_read_command_filters_and_numbers_files`, `test_system_summary_command_preserves_exit_and_summarizes_output`, `test_system_find_command_supports_native_find_shape`, `test_system_grep_command_groups_matches`, `test_system_grep_fff_engine_delegates_to_p28`, `test_system_json_deps_and_env_commands`, and `test_system_log_command_deduplicates_noisy_lines`.
- RTK formatter command coverage now includes top-level `Packet28 prettier` and `Packet28 format`, preserving formatter exit codes while rendering compact command summaries. `Packet28 format` accepts an explicit formatter name such as `prettier`, `black`, `ruff`, or `biome`, and otherwise makes a local best-effort formatter choice from project files. Evidence: `test_system_prettier_and_format_commands_wrap_formatter_output`.
- RTK language-tool wrapper coverage now includes top-level `Packet28 npm`, `Packet28 npx`, `Packet28 tsc`, `Packet28 vitest`, `Packet28 pytest`, and `Packet28 ruff`, preserving child exit codes and compacting common JS/Python test, typecheck, and lint output through Packet28's local summary surface. Evidence: `test_system_language_tool_commands_wrap_common_rtk_tools`.
- RTK system/infra wrapper coverage now includes top-level `Packet28 wc`, `Packet28 wget`, `Packet28 curl`, `Packet28 docker`, and `Packet28 kubectl`, using Packet28's reducer API for compact counts, download summaries, HTTP title/byte summaries, container lists, and Kubernetes pending-row summaries. Evidence: `test_system_infra_and_count_commands_use_reducer_wrappers`.
- RTK build/lint wrapper coverage now includes top-level `Packet28 cargo`, `Packet28 go`, `Packet28 dotnet`, `Packet28 golangci-lint`, `Packet28 gradlew`, `Packet28 rake`, `Packet28 rspec`, `Packet28 rubocop`, `Packet28 mypy`, `Packet28 pip`, and `Packet28 pnpm`, using Packet28's reducer API for compact Rust, Go, .NET, Gradle, Ruby, Python, and JS package-manager summaries. Evidence: `test_system_build_and_lint_commands_use_reducer_wrappers`.
- RTK analytics coverage now includes `Packet28 cc-economics`, which merges local Packet28 run-savings records with ccusage-style daily/weekly/monthly Claude Code spend JSON, computes weighted input-token cost per token, estimates dollar savings, supports text/JSON/CSV output, and gracefully reports local savings when ccusage is unavailable. Evidence: `test_cc_economics_merges_ccusage_and_packet28_savings`.
- RTK `gain` coverage now includes direct RTK-style mode flags and short aliases in addition to Packet28's `--format` selector: `--graph/-g`, `--history/-H`, `--failures/-F`, `--daily/-d`, `--weekly/-w`, `--monthly/-m`, `--quota/-q`, and `--all/-a`; quota mode accepts RTK-style `--tier/-t pro|5x|20x` plus Packet28's explicit `--quota-tokens` override; `--reset --yes` clears local Packet28 run-savings records. Evidence: `test_run_reduces_git_status`; `test_gain_reports_failed_and_fallback_runs`.
- RTK session coverage now includes RTK-style text output for `Packet28 session --sessions-dir <dir>`: a session/date/command/adoption/output table, compact adoption bar, average adoption, and discover hint, while preserving JSON output for automation. Evidence: `test_session_reports_adoption_from_session_jsonl`, `test_session_adoption_all_and_since_scan_multiple_session_files`.
- RTK learn coverage now includes CLI correction detection from Claude-style session history: Packet28 pairs Bash tool uses with tool results, detects fail-then-succeed corrections, classifies common error types, filters by frequency/confidence, emits JSON/text, and can write `.claude/rules/cli-corrections.md`. Evidence: `test_learn_detects_cli_correction_from_session_history`.
- ICM dashboard coverage now includes a local static web-view export: `Packet28 dashboard --format html --output <path>` renders savings, memory, graph, feedback, transcript, noisy command, pending extraction, and integration-health metrics without starting a server or leaving the machine. Evidence: `test_dashboard_shows_local_product_metrics`.
- ICM memory recall and vector-search coverage now uses the stronger local `packet28-local-lexical-v2` embedder: normalized tokens, token parts, character n-grams, and memory metadata are hashed into a normalized local vector, while legacy `packet28-local-hash-v1` rows remain readable. Evidence: `test_memory_store_recall_uses_sqlite_home_db` covers embedding creation plus typo-tolerant vector recall; `test_memory_recall_scores_importance_and_keywords`; `test_mcp_memory_store_recall_uses_sqlite_home_db`.
- ICM FTS coverage now includes calibrated memory recall scoring: FTS/BM25 candidates are blended with local vector candidates and reranked with deterministic importance, weight, metadata, content-term, raw-excerpt, and exact-phrase bonuses, while feedback, concept, and transcript search keep FTS-first behavior with LIKE fallback. Evidence: `test_memory_recall_scores_importance_and_keywords` covers exact phrase ranking; `test_feedback_and_graph_cli_use_sqlite`; `test_mcp_memory_store_recall_uses_sqlite_home_db`.
- ICM memoir graph coverage now includes local-first semantic distillation: topic memories create primary graph concepts, secondary keywords create related concepts in the same memoir, and `mentions` relations link the distilled concept neighborhood without calling a remote LLM. Evidence: `test_feedback_and_graph_cli_use_sqlite`; `test_mcp_memory_store_recall_uses_sqlite_home_db`.
- ICM transcript coverage now includes runtime hook transcripts for both successful `PostToolUse` output and failed `PostToolUseFailure` diagnostics, preserving session, agent, source, and project metadata for FTS-backed transcript search. Evidence: `test_hook_records_local_event_log_stats_and_dashboard_count`; `test_hook_failure_output_is_searchable_transcript_context`; `test_transcript_export_import_round_trip`.
- ICM wake-up coverage now includes deterministic CLI/MCP packs plus Claude SessionStart injection with project filtering and budgeted truncation. Non-Claude runtimes remain honest pre/post-tool rewrite and capture adapters because Packet28 does not claim a compatible SessionStart context-injection contract for them. Evidence: `test_packet28_hook_session_start_injects_wakeup_pack`; `test_mcp_memory_store_recall_uses_sqlite_home_db`.
- ICM hook lifecycle coverage now includes persisted Claude SessionStart/SessionEnd lifecycle events, PostToolUse/PostToolUseFailure capture into pending extraction/transcripts, hook log/stats reporting, and local deterministic pending extraction processing. Evidence: `test_hook_session_end_is_recorded_in_local_lifecycle_log`; `test_hook_records_local_event_log_stats_and_dashboard_count`; `test_memory_pending_extraction_queue_processes_into_memory`.
- ICM MCP tool-surface coverage now includes Packet28-native recurring memory pattern extraction: `Packet28 memory extract-patterns` and MCP `packet28.memory_extract_patterns` group repeated topic memories by deterministic local keyword/content signals and can materialize those patterns as graph concepts in a selected memoir. Evidence: `test_memory_consolidate_preserves_metadata_and_deletes_sources`; `test_mcp_memory_store_recall_uses_sqlite_home_db`; `cmd_mcp::tests::tools_list_exposes_product_compatibility_aliases`.
- RTK graceful hook degradation coverage now includes missing-binary generated command guards for Claude, Cursor, Copilot, Gemini, and Windsurf plus malformed-JSON fail-open checks for Claude, Cursor, Copilot, and Gemini. Evidence: `generated_packet28_hook_command_exits_zero_when_binary_is_missing`; `test_packet28_hooks_degrade_gracefully_on_bad_json_and_no_rewrite`.
- RTK command rewrite planning now normalizes RTK-style runtime prefixes and binary paths for shared reducer routing: bare `sudo`, `env VAR=...`, and absolute binary paths such as `/usr/bin/git`. Evidence: `routes_sudo_and_env_prefixed_commands_like_rtk`; `normalizes_absolute_binary_paths_like_rtk`.
- RTK command rewrite planning now distinguishes RTK ignored exact/prefix commands such as `cd`, `echo`, `mkdir`, `python -c`, `node -e`, and `pwd` from unsupported commands. Evidence: `marks_rtk_ignored_commands_as_intentional_passthrough`.
- RTK command rewrite planning now normalizes `golangci-lint` global options before `run`, including `-v`, `--color never`, `--color=never`, `--config=...`, and the `golangci` alias. Evidence: `routes_golangci_global_options_before_run_like_rtk`.
- RTK command rewrite planning now unwraps upstream JavaScript package-runner prefixes for supported reducer-backed tools: `npm exec`, `npm x`, `npm run-script`, `pnpm dlx`, `pnpm exec`, and `pnpx` route tsc, eslint, biome/lint, next build, prisma, vitest, jest, and playwright commands to Packet28 reducers instead of passing them through. Evidence: `classify_javascript_accepts_rtk_package_runner_prefixes`; `routes_rtk_javascript_package_runner_prefixes`.
- RTK command rewrite planning now covers upstream Python package-manager forms for `pip3`, `pip outdated`, `pip install`, `pip show`, and `uv pip list/outdated/install/show`, with reducer summaries for install/show output instead of raw passthrough. Evidence: `classify_python_accepts_rtk_pip_package_manager_forms`; `routes_rtk_python_package_manager_forms`; `reduce_pip_install_and_show_summarize_common_output`.
- RTK command rewrite planning now covers upstream Docker mutation forms `docker build`, `docker run`, `docker exec`, and `kubectl apply`, marking the resulting reducer specs as mutating and summarizing common output without treating them as read-only fetches. Evidence: `classify_infra_supports_rtk_docker_and_kubectl_mutation_forms`; `routes_rtk_docker_and_kubectl_mutation_forms`; `reduce_rtk_docker_and_kubectl_mutation_forms`.
- RTK command rewrite planning now handles compound `&&`/`||`/`;` commands with mixed supported/unsupported segments and pipe commands that rewrite the left side while leaving pipe consumers raw before resuming after later chain operators, including Claude-style PreToolUse hook rewriting through the shared planner. Evidence: `builds_compound_rewrite_for_supported_and_mixed_segments`; `builds_pipe_rewrite_for_left_side_only_then_resumes_after_chain_operator`; `pretool_rewrites_supported_compound_command`; `test_top_level_rewrite_handles_compound_commands_like_rtk`.
- RTK runtime hook delegation evidence now proves Claude, Cursor, Copilot, and Gemini PreToolUse-style adapters route RTK-style prefix/path commands through the same Packet28 planner. Evidence: `runtime_pretool_rewrites_use_shared_route_planner`.
- RTK OpenCode/Hermes plugin delegation now has a safer text-mode rewrite contract: no-rewrite `Packet28 rewrite` emits empty stdout instead of a human status string, so generated plugins preserve unsupported commands while still mutating supported rewrites. OpenCode and Hermes both have plugin-level smokes that verify rewrite and pass-through behavior with mocked runtime command runners. Evidence: `test_top_level_rewrite_prints_empty_stdout_on_no_rewrite`; `opencode_plugin_smoke_rewrites_and_passes_through_empty_stdout`; `hermes_plugin_smoke_rewrites_and_passes_through_empty_stdout`.
- RTK CLI command-family coverage now includes top-level `Packet28 smart <file>` with local two-line heuristic source summaries and top-level `Packet28 err <command...>` with exit-preserving failure summaries and JSON output. Evidence: `smart_summarizes_source_file_with_local_heuristics`; `test_system_smart_command_summarizes_source_file`; `test_system_err_command_preserves_exit_and_summarizes_failure`.
- RTK TOML filter parity now includes the embedded RTK-compatible built-in filter catalog as the last filter tier after trusted project and user-global filters. Built-ins use the same Packet28 raw-artifact-preserving custom-filter path, `Packet28 verify filters --filter <name>` can run their inline tests, and transparent rewrite planning routes matching TOML-filter commands to `Packet28 run` when no reducer/proxy route claims them first. Evidence: `builtin_toml_filters_route_to_packet28_run`; `test_run_applies_builtin_rtk_compatible_toml_filter`.
- ICM dashboard parity now includes a tested terminal dashboard mode with interactive navigation across Overview, Memory, Graph, Feedback, and Integrations panels, alongside existing JSON/text/static HTML output. Evidence: `test_dashboard_shows_local_product_metrics`.

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
