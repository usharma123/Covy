# Packet28 Stability Bugs

## Open

No open bugs are currently confirmed.

## Fixed

### BUG-2026-07-09-001: Hook rewrite corrupts shell pipeline semantics

- Symptom: `grep -o ... | sort -u | wc -l` could be rewritten into a Packet28 summary-producing command, causing downstream tools to count summary text instead of the original stream.
- Root cause: The PreToolUse route planner rewrote the left side of shell pipelines and stripped supported pipe postprocessors before deciding whether stdout was consumed programmatically.
- Fix: Pipe-consumed segments now fail open, pipe postprocess rewrites are disabled by default, and extraction-mode `grep`/`rg` commands are never rewritten.
- Verification:
  - `cargo test -p suite-cli route_registry -- --test-threads=1`
  - `cargo test -p suite-cli --test hook_rewrite_e2e hook_rewrite_cli -- --test-threads=1`

### BUG-2026-07-09-002: Hook rewrite emits control characters in rewritten commands

- Symptom: Claude Code could reject hook `updatedInput` when reducer fingerprints contained raw unit-separator bytes from argv joins.
- Root cause: Reducer cache fingerprints embedded structured argv material directly into the shell command passed through `--fingerprint`.
- Fix: Reducer fingerprints now use a BLAKE3 digest with a readable family/kind prefix, and `build_route_rewrite` fails open if any rewritten command contains control characters other than tab.
- Verification:
  - `cargo test -p packet28-reducer-core --lib`
  - `cargo test -p suite-cli route_registry::tests::reducer_rewrite_output_does_not_contain_control_characters`

### BUG-2026-07-09-003: MCP and daemon storage paths are brittle under malformed input and log growth

- Symptom: A missing MCP method could terminate the server, task notification polling reparsed full JSONL event logs, corrupt event-log lines could fail whole reads, concurrent atomic writes reused the same `.tmp` path, and registry reads/writes had no process-level file lock.
- Root cause: The MCP stdio loop used fallible request extraction for protocol errors, notification state tracked only sequence numbers, event-log reads failed on first bad line, `write_atomically` used `path.with_extension("tmp")`, and task/watch registry persistence relied only on atomic rename.
- Fix: MCP now returns JSON-RPC error codes and keeps serving, task polling uses byte offsets, event-log reads skip corrupt lines, pid/runtime writes use atomic replacement, atomic temp paths include process/counter uniqueness, and task/watch registries use shared/exclusive flock guards.
- Verification:
  - `cargo test -p packet28-daemon-core storage -- --test-threads=1`
  - `cargo test -p suite-cli --test mcp_native_stdio_e2e test_mcp_native_stdio_accepts_newline_json -- --test-threads=1`

### BUG-2026-07-09-004: Doctor does not flag daemon/CLI version skew

- Symptom: A stale daemon could serve older hook behavior while the CLI and npm shim appeared current.
- Root cause: Daemon status omitted the daemon version, so doctor could not compare the running daemon with the current CLI.
- Fix: Daemon runtime/status now include `PACKET28_VERSION`, and doctor fails the daemon check when the running daemon version differs from the CLI.
- Verification:
  - `cargo test -p suite-cli doctor`

### BUG-2026-07-09-005: Hook rewrite has no operator kill switch

- Symptom: When PreToolUse rewriting caused bad command behavior, users had to edit hook config files or evade Packet28 entirely to stop rewriting.
- Root cause: `HookRuntimeConfig` had a `rewrite_enabled` flag, but no CLI command exposed it for incident response.
- Fix: Added `packet28 hook rewrite off|on|status`, with `off` disabling only command rewriting while leaving PostToolUse capture enabled.
- Verification:
  - `cargo test -p suite-cli --test hook_rewrite_e2e test_hook_rewrite_cli_can_disable_and_reenable_command_rewrites -- --test-threads=1`

### BUG-2026-07-09-006: MCP default catalog still exposes nonessential tools and a handoff alias

- Symptom: The default MCP `tools/list` payload still spent first-load catalog budget on advisory/risk helpers and listed `packet28_handoff` as a compatibility alias.
- Root cause: The Core toolset included more than the minimum search/read/fetch/handoff/status tools, and the catalog listed compatibility aliases instead of accepting them only at dispatch.
- Fix: Core now omits advisory/risk helpers, `packet28_handoff` is removed from `tools/list`, and both `packet28_handoff` and `packet28.handoff` still dispatch to `packet28.prepare_handoff`.
- Verification:
  - `cargo test -p suite-cli tools_list_ -- --test-threads=1`

### BUG-2026-07-09-007: Thin reducer wrappers overwhelm top-level CLI help

- Symptom: Top-level help exposed dozens of reducer wrapper commands such as `ping`, `ssh`, `xcodebuild`, and `ollama`, making the public CLI surface hard to scan.
- Root cause: Internal reducer-routing wrappers were modeled as visible top-level Clap subcommands.
- Fix: Existing wrapper commands remain callable for compatibility, but `cmd_system::ToolArgs` wrappers are hidden from generated top-level help.
- Verification:
  - `cargo test -p suite-cli cli_defs::tests -- --test-threads=1`

### BUG-2026-05-14-001: Packaged `packet28d` can lose execute bits

- Symptom: MCP startup failed with `failed to spawn packet28d` followed by `Permission denied (os error 13)`.
- Root cause: Packaged native installs can leave the daemon sibling binary present but not executable. The CLI assumed `packet28d` beside `Packet28` was directly spawnable.
- Fix: The daemon launcher now repairs a non-executable `packet28d` before spawning it, and npm entrypoints repair the daemon sibling independently.
- Verification:
  - `cargo test -p suite-cli ensure_executable_repairs_packaged_daemon_mode`
  - `cargo check -p suite-cli -p packet28d -p packet28-daemon-core`

### BUG-2026-05-14-002: Cursor agent prompt hides canonical MCP tool names

- Symptom: `cargo test --workspace` failed `test_suite_agent_prompt_outputs_all_supported_fragments` because the Cursor prompt did not include `packet28.write_intention`.
- Root cause: The Cursor prompt had been narrowed to underscore-only MCP names, which made the canonical dotted Packet28 aliases disappear from the generated guidance.
- Fix: The Cursor prompt now includes underscore client names together with the canonical dotted aliases for intention writing, compact reads/globs, tool-result fetches, and handoff preparation.
- Verification:
  - `cargo test -p suite-cli test_suite_agent_prompt_outputs_all_supported_fragments`

### BUG-2026-05-14-003: Strict clippy gate fails on reducer and CLI lint regressions

- Symptom: `cargo clippy --workspace --all-targets -- -D warnings` failed on non-minimal booleans, duplicate `if` branches, needless borrows, derivable defaults, an oversized helper signature, manual char comparisons, and test length comparisons.
- Root cause: Several shared CLI/reducer modules had accumulated warnings that were not covered by the normal workspace test gate.
- Fix: Simplified the affected reducers, routing helpers, TOML filter defaults, system helpers, transcript import, memory token splitting, and e2e assertions; replaced the oversized filtered-run helper signature with a parameter struct.
- Verification:
  - `cargo test -p packet28-reducer-core classify_and_reduce_generic_npx_as_mutation`
  - `cargo test -p suite-cli toml_filters::tests`
  - `cargo test -p suite-cli cmd_system::tests::json_schema_renders_types_without_values`
  - `cargo test -p suite-cli route_registry::tests::routes_supported_compound_commands`
  - `cargo clippy --workspace --all-targets -- -D warnings`

### BUG-2026-05-14-004: Release source metadata lagged behind stability patch version

- Symptom: Package dry-runs showed source npm metadata at older versions while the stability patch changelog reached v0.2.59.
- Root cause: Workspace and npm template versions were not aligned with the latest stability patch release notes.
- Fix: Aligned the Cargo workspace version, root npm package metadata, platform package template, and checked-in Darwin package metadata to v0.2.59.
- Verification:
  - `cargo check -p suite-cli -p packet28d -p packet28-search-cli`
