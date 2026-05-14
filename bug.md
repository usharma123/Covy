# Packet28 Stability Bugs

## Open

No open bugs are currently confirmed.

## Fixed

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
