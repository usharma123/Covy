# Packet28 Real Repository Suite

This directory contains committed summaries from repeatable real-repository Packet28 experiment runs.

Run it after building Packet28:

```bash
cargo build -p suite-cli
PACKET28_REAL_EXPERIMENT_REPEATS=3 docs/experiments/run_packet28_real_repo_suite.sh
```

The harness clones Packet28, ripgrep, and fd into a temporary work directory, then compares native shell output with `Packet28 run --json` across search, code-review, failing-test triage, implementation-state, docs lookup, and handoff/bootstrap-style workflows. When `p28` is available, it also records compact indexed-search attempts.

Generated run directories are ignored by git because they contain raw stdout/stderr captures. Curated `SMOKE_*.md` and `SMOKE_*.jsonl` files may be committed as evidence artifacts.
