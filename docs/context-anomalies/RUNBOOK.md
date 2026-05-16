# Context Anomaly Trends

Context anomaly history is written by:

```sh
Packet28 verify context-anomalies --root . --json
```

The verifier appends compact JSONL records to `.packet28/context-anomaly-history.jsonl`. `Packet28 dashboard` reads that live history and reports latest status, latest high count, latest hidden categories, and recurring hidden categories:

```sh
Packet28 dashboard --root . --json
```

Use the checked-in fixture when you need deterministic trend output without mutating live `.packet28` state:

```sh
Packet28 dashboard --root . --context-anomaly-history docs/context-anomalies/history.jsonl --json
```

The fixture should report `latest_status=ready` with recurring hidden `fallback_provenance`. That proves recurring hidden categories survive a final clean record. It should also report `recurring_hidden_samples` with `fallback_provenance=recent_fallbacks=1`.

When live recurring hidden categories differ from the fixture, inspect the omitted categories before treating the digest as complete:

```sh
Packet28 verify context-anomalies --root . --max-high 2 --json
Packet28 digest --root . --json
```

`verify context-anomalies` emits `hidden_samples` for capped categories in the current digest. The dashboard trend tile emits `recurring_hidden_samples` from stored history, which gives the latest compact source sample for each recurring hidden category.

Hidden sample signals are capped at 120 characters so fixture, dashboard, and CI summaries can stay compact while preserving the source category and signal prefix.

Text summaries percent-escape `%`, semicolons, and newlines inside signals. Category names also escape `=` so `category=signal;category=signal` lines stay parseable.

Recurring hidden categories usually mean medium-severity sources are repeatedly being capped from the digest. Fix the underlying source or raise it into a visible dashboard tile before relying on the compact anomaly summary.
