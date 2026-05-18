# Packet28 Agentic Coding Tokenomics

This experiment compares a normal shell-tool trace against a Packet28 trace on the same generated Rust bug-fix task.

## Result

- Normal-tool context: `8040` estimated tokens
- Packet28 context: `487` estimated tokens
- Saved context: `7553` estimated tokens
- Savings: `93.9%`
- Normal steps: `5`
- Packet28 steps, including extra safety/features: `7`
- Optional full-artifact fetch tokens verified but not counted in slim context: `2772`
- Feature checks passed: `9/9`

## Feature Checks

| Feature | Status |
| --- | --- |
| `glob_artifact_fetch` | ok |
| `search_artifact_fetch` | ok |
| `read_regions_artifact_fetch` | ok |
| `patch_risk_returned_required_checks` | ok |
| `hook_reduced_pre_fix_test` | ok |
| `workspace_fingerprint_busted_stale_test_cache` | ok |
| `hook_reduced_post_fix_test` | ok |
| `post_fix_tests_passed` | ok |
| `validate_tool_outcome_returned_status` | ok |

## Step Token Comparison

| Trace | Step | Tool/Command | Exit/Status | Tokens | Notes |
| --- | --- | --- | --- | ---: | --- |
| normal | 1 | `find src tests docs -type f` | `0` | 22 | raw output |
| normal | 2 | `rg -n normalize_user_id|resolve_login_alias|login alias|UserId src tests docs` | `0` | 1355 | raw output |
| normal | 3 | `sed -n 1,140p src/auth/user_id.rs` | `0` | 1019 | raw output |
| normal | 4 | `cargo test --lib` | `101` | 2912 | raw output |
| normal | 5 | `cargo test --lib` | `0` | 2732 | raw output |
| packet28 | 1 | `packet28.glob` | `ok` | 37 | artifact `tool-invocation-1-result.json` fetch ok=True |
| packet28 | 2 | `packet28.search` | `ok` | 154 | artifact `tool-invocation-2-result.json` fetch ok=True |
| packet28 | 3 | `packet28.read_regions` | `ok` | 154 | artifact `tool-invocation-3-result.json` fetch ok=True |
| packet28 | 4 | `packet28.patch_risk` | `ok` | 50 | slim MCP payload |
| packet28 | 5 | `packet28.hook` | `101` | 11 | hook reduction 99.6% |
| packet28 | 6 | `packet28.hook` | `0` | 7 | hook reduction 99.7% |
| packet28 | 7 | `packet28.validate_tool_outcome` | `ok` | 74 | slim MCP payload |

## Task Outcome

- Both traces applied the same deterministic fix: `input.trim().to_ascii_lowercase()`.
- Packet28 post-fix focused Rust tests: `passed`.
- Packet28 used more featureful steps than the normal baseline and still reduced context.
