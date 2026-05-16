# Context Anomaly Trends

Use this command table before treating a compact anomaly summary as complete:

| Need | Command | Expected local check |
|---|---|---|
| Write live history | `Packet28 verify context-anomalies --root . --json` | Appends compact JSONL to `.packet28/context-anomaly-history.jsonl`. |
| Replay fixture trend | `Packet28 dashboard --root . --context-anomaly-history docs/context-anomalies/history.jsonl --json` | Reports fixture `latest_status=ready`. |
| Check sample formatter | `node scripts/check_context_anomaly_hidden_samples.mjs` | Prints one `context_anomaly_hidden_sample_fixture_ok=...` line. |
| Read formatter budget | `node scripts/check_context_anomaly_hidden_samples.mjs --json` | Emits `actual_len`, `max_len`, `checksum`, and escaped `summary`. |
| Self-test formatter | `node scripts/check_context_anomaly_hidden_samples.mjs --self-test` | Prints one `context_anomaly_hidden_sample_self_test_ok` line. |
| List formatter modes | `node scripts/check_context_anomaly_hidden_samples.mjs --help` | Lists every accepted mode in under eight lines. |
| Audit formatter flow | `node scripts/audit_context_anomaly_hidden_samples.mjs` | Runs the smoke modes, fixture dashboard, digest, and verifier checks; add `--json` for compact fields. |
| Audit release gate | `node scripts/audit_context_anomaly_hidden_samples.mjs --strict` | Uses release-like `--max-high 0` for the verifier check. |
| List audit modes | `node scripts/audit_context_anomaly_hidden_samples.mjs --help` | Lists tolerant, strict, JSON, and checksum modes in under six lines. |
| Check summary budget | `node scripts/check_context_anomaly_summary_budget.mjs --self-test` | Prints one success line; `--help` lists modes; low env budgets fail with `context_anomaly_summary_budget_too_many_lines`, `context_anomaly_summary_budget_line_too_long`, or `context_anomaly_summary_budget_json_too_long`. |
| Read summary budget | `node scripts/check_context_anomaly_summary_budget.mjs --json` | Emits budgets, labels, and `max_json_bytes`; env knobs are `P28_CONTEXT_ANOMALY_SUMMARY_MAX_LINES`, `P28_CONTEXT_ANOMALY_SUMMARY_MAX_LINE`, and `P28_CONTEXT_ANOMALY_SUMMARY_JSON_MAX`. |
| Check runbook density | `node scripts/check_context_anomaly_runbook_density.mjs --self-test` | Prints one success line; named failures are listed below. |
| Read runbook density | `node scripts/check_context_anomaly_runbook_density.mjs --json` | JSON keeps full field names including `alias_docs_checked`; JSON budget is `max_json_bytes=320`; default text is one-line `key=value` aliases: `lines`, `row`, `cmds`, `fc`, `env`, `labels`, `phrases`, `alias_docs`, `wf`, `prose`, `json`, `width_docs`, `width`; alias map: `fc`=failure codes, `wf`=workflow commands, `json`=JSON headroom; `width` is capped by `P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX`; envs below. |
| Inspect digest | `Packet28 digest --root . --json` | Shows visible anomalies and capped `hidden_samples`. |
Density envs: `P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES`, `P28_CONTEXT_ANOMALY_RUNBOOK_ROW_MAX`, `P28_CONTEXT_ANOMALY_RUNBOOK_PROSE_MAX`, `P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX`, `P28_CONTEXT_ANOMALY_RUNBOOK_JSON_MAX`, `P28_CONTEXT_ANOMALY_RUNBOOK_JSON_HEADROOM_MIN`.
Density failures: `context_anomaly_runbook_density_too_many_lines`, `context_anomaly_runbook_density_row_too_wide`, `context_anomaly_runbook_density_missing_commands`, `context_anomaly_runbook_density_workflow_missing_commands`, `context_anomaly_runbook_density_missing_env_docs`, `context_anomaly_runbook_density_text_too_wide`.
Density doc failures: `context_anomaly_runbook_density_missing_failure_docs`, `context_anomaly_runbook_density_missing_output_docs`, `context_anomaly_runbook_density_prose_too_wide`, `context_anomaly_runbook_density_json_too_long`. `Packet28 dashboard --root . --json` reads live history and reports latest status, high count, hidden categories, and recurring hidden categories.
The audit script uses `verify context-anomalies --max-high 2` so local smoke can pass with known live quality debt and reports `audit_mode=tolerant`. The workflow threshold step and strict audit mode use `--max-high 0` and report `audit_mode=strict`.

Manual workflow dispatch has a `strict_audit` input that also runs `node scripts/audit_context_anomaly_hidden_samples.mjs --strict`. Workflow runs upload `context-anomaly-hidden-sample-audit.txt` and `.json` in the `context-anomaly-hidden-sample-audit` artifact; reproduce checksums with `node scripts/audit_context_anomaly_hidden_samples.mjs --checksum context-anomaly-hidden-sample-audit.txt` or the `.json` path. `missing-audit-text/json` means the artifact file was absent.

The fixture should report `latest_status=ready` with recurring hidden `fallback_provenance`. That proves recurring hidden categories survive a final clean record. It should also report `recurring_hidden_samples` with `fallback_provenance=recent_fallbacks=1`.

When live recurring hidden categories differ from the fixture, inspect the omitted categories before treating the digest as complete.

`verify context-anomalies` emits `hidden_samples` for capped categories in the current digest. The dashboard trend tile emits `recurring_hidden_samples` from stored history, which gives the latest compact source sample for each recurring hidden category.

Hidden sample signals are capped at 120 characters so fixture, dashboard, and CI summaries can stay compact while preserving the source category and signal prefix.

Text summaries percent-escape `%`, semicolons, and newlines inside signals. Category names also escape `=` so `category=signal;category=signal` lines stay parseable.

Check the shared summary fixture locally with `node scripts/check_context_anomaly_hidden_samples.mjs`. A passing run prints one `context_anomaly_hidden_sample_fixture_ok=...` line and fails if the escaped summary exceeds 256 characters.

The smoke script also checks `docs/context-anomalies/hidden-samples-delimiters.sha256` before comparing the expected summary; refresh it with `shasum -a 256 docs/context-anomalies/hidden-samples-delimiters.json | awk '{print $1}' > docs/context-anomalies/hidden-samples-delimiters.sha256`.

Workflow formatter budget lines use `actual/max`, matching the smoke script JSON fields `actual_len` and `max_len`; formatter checksum lines match the JSON `checksum`, which is verified before success output and equals `hidden-samples-delimiters.sha256`.

Recurring hidden categories usually mean medium-severity sources are repeatedly being capped from the digest. Fix the underlying source or raise it into a visible dashboard tile before relying on the compact anomaly summary.
