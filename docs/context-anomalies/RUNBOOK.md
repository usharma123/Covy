# Context Anomaly Trends

Use this command table before treating a compact anomaly summary as complete:

| Need | Command | Expected local check |
|---|---|---|
| Write live history | `Packet28 verify context-anomalies --root . --json` | Appends compact JSONL to `.packet28/context-anomaly-history.jsonl`. |
| Replay fixture trend | `Packet28 dashboard --root . --context-anomaly-history docs/context-anomalies/history.jsonl --json` | Reports fixture `latest_status=ready`. |
| Check sample formatter | `node scripts/check_context_anomaly_hidden_samples.mjs` | Prints one `context_anomaly_hidden_sample_fixture_ok=...` line. |
| Inspect digest | `Packet28 digest --root . --json` | Shows visible anomalies and capped `hidden_samples`. |

`Packet28 dashboard --root . --json` reads live history and reports latest status, high count, hidden categories, and recurring hidden categories.

The fixture should report `latest_status=ready` with recurring hidden `fallback_provenance`. That proves recurring hidden categories survive a final clean record. It should also report `recurring_hidden_samples` with `fallback_provenance=recent_fallbacks=1`.

When live recurring hidden categories differ from the fixture, inspect the omitted categories before treating the digest as complete.

`verify context-anomalies` emits `hidden_samples` for capped categories in the current digest. The dashboard trend tile emits `recurring_hidden_samples` from stored history, which gives the latest compact source sample for each recurring hidden category.

Hidden sample signals are capped at 120 characters so fixture, dashboard, and CI summaries can stay compact while preserving the source category and signal prefix.

Text summaries percent-escape `%`, semicolons, and newlines inside signals. Category names also escape `=` so `category=signal;category=signal` lines stay parseable.

Check the shared summary fixture locally with `node scripts/check_context_anomaly_hidden_samples.mjs`. A passing run prints one `context_anomaly_hidden_sample_fixture_ok=...` line and fails if the escaped summary exceeds 256 characters.

Recurring hidden categories usually mean medium-severity sources are repeatedly being capped from the digest. Fix the underlying source or raise it into a visible dashboard tile before relying on the compact anomaly summary.
