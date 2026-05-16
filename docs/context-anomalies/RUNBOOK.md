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

The fixture should report `latest_status=ready` with recurring hidden `fallback_provenance`. That proves recurring hidden categories survive a final clean record.

When live recurring hidden categories differ from the fixture, inspect the omitted categories before treating the digest as complete:

```sh
Packet28 verify context-anomalies --root . --max-high 2 --json
Packet28 digest --root . --json
```

Recurring hidden categories usually mean medium-severity sources are repeatedly being capped from the digest. Fix the underlying source or raise it into a visible dashboard tile before relying on the compact anomaly summary.
