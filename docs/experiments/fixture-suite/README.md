# Packet28 Fixture Suite

This directory contains the repeatable local fixture harness for the real-world experiment gate in `docs/packet28_goal.md`.

Run it after building Packet28:

```bash
cargo build -p suite-cli
PACKET28_EXPERIMENT_REPEATS=3 docs/experiments/run_packet28_fixture_suite.sh
```

The harness creates temporary Rust, Node, and documentation repositories, then compares native command output with `Packet28 run --json` output across search, code-review, failing-test triage, implementation-diff, docs lookup, and handoff/bootstrap-style workflows. When the `p28` binary is available, it also records compact indexed-search attempts.

Generated run directories are ignored by git because they contain raw stdout/stderr captures. Each run writes:

- `results.jsonl` with per-command metrics.
- `summary.md` with aggregate native, Packet28, fallback, failed-command, indexed-search, savings, elapsed-time, and raw-artifact statistics.
- `*.out` and `*.err` raw command captures for local inspection.

This fixture suite is a fast regression harness. It does not replace the required larger evidence runs on Packet28 plus at least two unrelated non-trivial real repositories.
