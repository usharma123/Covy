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
