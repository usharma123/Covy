# Rust safety and panic policy

Packet28 keeps unsafe code at OS, FFI, memory-map, and verification boundaries.
Reusable algorithms remain safe Rust. Production fallibility is represented by
`Result` or `Option`; a panic is permitted only for a mechanically proven
programmer invariant and must use a narrow, reasoned `#[expect(...)]`.

## Validated baseline

The audit reported 17 unsafe and 66 production panic-family warnings. The
focused commands were rerun against the then-current remediation branch before
this slice: earlier harness work had added one unsafe and two production panic
sites, so current-source validation found 18 and 68 respectively.

| Scope | Command scope | Diagnostics | Breakdown |
|---|---|---:|---|
| Unsafe, all targets | workspace, all targets/features | 18 | 15 production, 3 test-only; all were `undocumented_unsafe_blocks` |
| Missing safety docs | workspace, all targets/features | 0 | Linux and macOS public unsafe hooks already had `# Safety` contracts |
| Panic family, production | workspace libraries, binaries, and build scripts | 68 | 23 `expect(Result)`, 17 `expect(Option)`, 14 `unwrap(Result)`, 4 `unwrap(Option)`, 6 `panic!`, 4 `unreachable!` |
| Panic family, non-production unique locations | tests, benches, and examples after subtracting production locations | 4,301 | 4,011 unwrap-family, 216 expect-family, 72 `panic!`, 2 `panic_in_result_fn` |

The panic inventory deliberately separates production from tests. Assertions,
`unwrap_err`, and direct fixture setup make failures legible in tests; enabling
the production panic policy for every test would create thousands of low-value
rewrites without reducing shipped risk.

The 68 production diagnostics were accounted for by file:

```text
buildy-core/src/parse.rs                         unwrap=5
context-instruct-shim/build.rs                   expect=1 panic=1
context-kernel-core/src/instruction_runtime.rs   unreachable=1
context-kernel-core/src/kernel_runtime.rs        expect=1
context-scheduler-core/src/runtime.rs            expect=1
covy-ingest/src/gocov.rs                         unwrap=1
diffy-core/src/report.rs                         unwrap=1
mapy-core/src/ast.rs                             expect=5
mapy-core/src/generation.rs                      expect=2
mapy-core/src/runtime.rs                         expect=1
packet28-search-core/src/lib.rs                  expect=9
packet28d/src/broker/handoff.rs                  unwrap=1
packet28d/src/broker/render.rs                   expect=2
packet28d/src/broker/support.rs                  expect=1
packet28d/src/watch.rs                           expect=2
stacky-core/src/parse.rs                         unwrap=3
suite-cli/build.rs                               expect=4 panic=5
suite-cli/src/cmd_compact_session.rs             expect=1
suite-cli/src/cmd_dashboard.rs                   expect=3
suite-cli/src/cmd_doctor_mcp.rs                  unwrap=2
suite-cli/src/cmd_run.rs                         unreachable=1
suite-cli/src/cmd_setup.rs                       unreachable=2
suite-cli/src/cmd_system/source.rs               expect=2
suite-cli/src/toml_filters.rs                    expect=1
suite-foundation-core/src/coverage_store.rs      unwrap=2
suite-foundation-core/src/diagnostics_store.rs   unwrap=3
suite-foundation-core/src/pathmap.rs              expect=1
suite-proxy-core/src/runtime.rs                   expect=2
testy-core/src/pipeline.rs                       expect=1
```

The 18 unsafe diagnostics were similarly exhaustive: 13 in the macOS
interpose adapter, one in the read-only search mmap, one in Unix stdout
redirection, two in the process-harness E2E, and one in the macOS runtime E2E.

Task-store retention adds two reviewed unsafe-bearing files while keeping the
same locality rule. `retention/capability.rs` owns the task store's
descriptor-level OS boundary: `fpathconf`, Linux ACL xattrs, and macOS ACL
calls. Each call has a local `SAFETY` contract; the capability tests exercise
name limits, ACL rejection without mutation, inherited-ACL stripping before
publication, and descriptor identity checks. `storage.rs` uses `getrusage`
only inside a `cfg(test)` subprocess probe that proves adversarial authority
JSON stays within the resident-memory bound. No reusable retention or storage
algorithm requires callers to write unsafe Rust. The MCP artifact boundary is
similarly isolated in `cmd_mcp_artifact_io.rs`: its `openat`, `mkdirat`,
`renameat`, `unlinkat`, `fstatat`, and bounded directory-enumeration calls
implement descriptor-relative confinement. Its tests cover exact-name
revalidation, symlink and hard-link rejection, case-folded alias rejection,
bounded reads/writes, and non-mutation on failure.

Authenticated daemon transport adds four production unsafe-bearing files and
one test fixture without moving unsafe code into reusable algorithms.
`packet28-daemon-client` owns descriptor-relative runtime discovery, ACL/xattr
queries, and Unix peer-credential calls. `packet28-daemon-protocol::paths` and
`packet28d::application` use the effective UID only to isolate the socket
namespace and authenticate its owner and peer. The p28 daemon fixture uses the
same value to force the unauthenticated-parent fallback path. Every call has a
local `SAFETY` contract; transport tests cover authenticated Unix operation,
forced TCP, workspace fallback, wrong-owner rejection, and runtime-entry
replacement and special-file failures.

MCP notification-poller lifecycle tests use one reviewed Unix-only socket
probe. The fixture calls `setsockopt`, `getsockopt`, and `ioctl(FIONREAD)` to
create and observe deterministic pipe pressure while verifying cancellation
and join behavior. Each call is confined to the test helper, owns its socket
descriptor for the duration of the call, and documents the pointer-lifetime
contract at the unsafe boundary. No MCP production path requires unsafe Rust.

Persistence path-safety tests add two reviewed Unix-only probes while the
production state-filesystem API remains safe Rust. Context-memory subprocess
fixtures use `pre_exec` with async-signal-safe `setrlimit` calls to enforce
resource ceilings and `mkfifo` to prove special-file rejection.
`packet28-state-fs` has a separate `mkfifo` rejection fixture. The pathname
pointers are live, NUL-terminated, and never retained; all calls are confined
to test configurations.

After fallible paths were repaired, 13 reviewed lint expectations remain:
four lint entries cover the two build scripts' intentional fatal exits, seven
cover compile-time regex literals exercised by parser/filter tests, and two
cover collection lookups proven by construction in the scheduler and test-map
builder. The checker stores this exact multiset, rejects production `#[allow]`,
and requires a reason on each expectation.

## Enforced invariants

- Workspace Clippy denies undocumented unsafe blocks and missing safety docs.
- Rust denies unsafe operations inside an unsafe function unless each operation
  is placed in an explicit unsafe block.
- `scripts/check_rust_hazards.py` denies the panic family for production library,
  binary, and build-script targets. Narrow `#[expect]` annotations require a
  reason, are checked for fulfillment, and are reconciled against an exact
  reviewed inventory. Production `#[allow]` overrides are rejected.
- The same checker inventories unsafe syntax. Its reviewed allowlist currently
  contains 26 files: 14 production OS/FFI/mmap adapters and twelve
  test/benchmark instrumentation files. A new unsafe-bearing file or stale
  allowlist entry fails the gate until the architectural inventory is
  reconciled explicitly.

Run the permanent check with:

```console
python3 scripts/check_rust_hazards.py
```

The canonical full gate runs this check before its ordinary all-target Clippy
pass. Linux-specific unsafe contracts are additionally compiled by Linux CI;
the macOS host validates the macOS interpose implementation.
