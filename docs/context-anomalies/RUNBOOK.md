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
| Inspect digest | `Packet28 digest --root . --json` | Shows visible anomalies and capped `hidden_samples`. |

`Packet28 dashboard --root . --json` reads live history and reports latest status, high count, hidden categories, and recurring hidden categories.

The fixture should report `latest_status=ready` with recurring hidden `fallback_provenance`. That proves recurring hidden categories survive a final clean record. It should also report `recurring_hidden_samples` with `fallback_provenance=recent_fallbacks=1`.

When live recurring hidden categories differ from the fixture, inspect the omitted categories before treating the digest as complete.

`verify context-anomalies` emits `hidden_samples` for capped categories in the current digest. The dashboard trend tile emits `recurring_hidden_samples` from stored history, which gives the latest compact source sample for each recurring hidden category.

Hidden sample signals are capped at 120 characters so fixture, dashboard, and CI summaries can stay compact while preserving the source category and signal prefix.

Text summaries percent-escape `%`, semicolons, and newlines inside signals. Category names also escape `=` so `category=signal;category=signal` lines stay parseable.

Check the shared summary fixture locally with `node scripts/check_context_anomaly_hidden_samples.mjs`. A passing run prints one `context_anomaly_hidden_sample_fixture_ok=...` line and fails if the escaped summary exceeds 256 characters.

The smoke script also checks `docs/context-anomalies/hidden-samples-delimiters.sha256` before comparing the expected summary.

Refresh that checksum with `shasum -a 256 docs/context-anomalies/hidden-samples-delimiters.json | awk '{print $1}' > docs/context-anomalies/hidden-samples-delimiters.sha256`.

Workflow formatter budget lines use `actual/max`, matching the smoke script JSON fields `actual_len` and `max_len`; formatter checksum lines match the JSON `checksum`.

The JSON `checksum` is verified before success output and equals `hidden-samples-delimiters.sha256`.

Recurring hidden categories usually mean medium-severity sources are repeatedly being capped from the digest. Fix the underlying source or raise it into a visible dashboard tile before relying on the compact anomaly summary.
