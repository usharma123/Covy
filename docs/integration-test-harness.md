# Integration-test harness boundary

`suite-cli` integration tests use
`crates/suite-cli/tests/support/process_harness.rs` as the single shared owner
for child-process and MCP stdio lifecycles.

## Deep interface

- `ProcessHarness` owns the child process group, stdin writer, bounded stdout and
  stderr capture, deadlines, termination, and reaping.
- `McpHarness` adds bounded content-length or newline-JSON framing, response-ID
  routing, protocol limits, and a single deadline spanning request writes and
  response reads.
- `ensure_packet28d_built`, `build_workspace_package`, and `run_git` keep nested
  fixture commands bounded and use the locked Cargo graph.
- A dropped harness closes stdin, terminates the process group, and reaps the
  leader. Timeout errors retain bounded diagnostics.

The stdin writer runs on an owned pump thread. This matters because a child that
does not read stdin can fill the OS pipe: the calling test still observes its
deadline, kills the process group, captures diagnostics, and reaps the child.

## Local fixtures that stay local

One-shot `assert_cmd` assertions and data-only fixture writers stay beside the
workflow they describe. Raw MCP framing is limited to malformed-frame harness
regressions and fake upstream peers. Direct sockets remain only where endpoint
release or disconnect behavior is itself under test, and their polling loops
carry explicit elapsed-time bounds. Temporary directories rely on RAII; tests
must not add ad hoc filesystem cleanup.

`scripts/check_test_harness.py` derives this inventory from source and rejects:

- manual child spawn/kill/wait outside the harness;
- nested Cargo builds or Git fixture processes outside shared bounded helpers;
- synchronous child waits inside support modules;
- manual temporary-file cleanup;
- raw MCP framing or socket lifecycles outside reviewed local fixtures; and
- reviewed socket fixtures that lose their deadline markers.

Run the policy and its mutation-style unit tests with:

```text
python3 scripts/check_test_harness.py
python3 -m unittest scripts.tests.test_check_test_harness
```
