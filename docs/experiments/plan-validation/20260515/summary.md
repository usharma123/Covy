# Packet28 Plan Validation CLI Evidence - 2026-05-15

This experiment exercises `Packet28 plan validate` on the Packet28 repository with cached coverage and testmap state.

Inputs:

- `coverage.lcov` marks `crates/suite-cli/src/cmd_plan.rs` as an uncovered changed target.
- `testmap-manifest.jsonl` maps that file to the existing `crates/suite-cli/tests/e2e_smoke.rs` test path.
- `steps-mapped.json` reads the CLI file, edits it, then runs the mapped test path.

Commands:

- `target/debug/covy ingest docs/experiments/plan-validation/20260515/coverage.lcov --format lcov --output .covy/state/latest.bin --json`
- `target/debug/Packet28 test map --manifest docs/experiments/plan-validation/20260515/testmap-manifest.jsonl --output .covy/state/testmap.bin --timings-output .covy/state/testtimings.bin --json`
- `target/debug/Packet28 plan validate --task-id plan-validation-20260515 --steps-file docs/experiments/plan-validation/20260515/steps-mapped.json --json --pretty`

Result:

- `valid`: `true`
- `test_gate_score`: `100`
- `violations`: `0`
- `warnings`: `0`
