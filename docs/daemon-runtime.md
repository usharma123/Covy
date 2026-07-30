# Packet28 daemon runtime

This document is the operating contract for the Packet28 daemon stack. It
describes ownership, startup and shutdown order, persistence and recovery,
transport behavior, compatibility, request failure semantics, and the exact
public documentation inventory.

## Ownership boundaries

| Owner | Responsibility | Must not own |
| --- | --- | --- |
| `packet28-daemon-protocol` | Wire DTOs, request/response tags, bounded JSON framing, and deterministic endpoint paths. | Runtime, storage, kernels, memory, search, or Tokio orchestration. |
| `packet28-daemon-core` | Typed storage errors, task/event persistence, integrity, leases, trust checks, recovery, and retention. | The server lifecycle or new wire contracts. Its root re-exports are a frozen `0.2.x` compatibility facade. |
| `packet28d::application` | One daemon instance: recovery, state construction, Tokio ownership, listeners, workers, cancellation, joining, and final cleanup. | CLI parsing or broker implementation details. |
| `packet28d::broker` | Context, handoff, write-state, search, rendering, limits, snapshots, and a small explicit crate-internal facade. | Application lifecycle ownership or parent-prelude wildcard imports. |
| `packet28d` binary | CLI parsing, error presentation, and process exit. | Protocol dispatch, persistence, transport, or daemon state. |

`scripts/check_architecture.py` enforces the thin binary, broker file inventory,
explicit imports, protocol/runtime dependency direction, and the public
documentation inventory below.

## Public documentation inventory

Every public module in `packet28-daemon-protocol` and `packet28-daemon-core`,
and every public module or export in `packet28d`, appears exactly once here.
`covered` means the row names a compile-checked source example. `excluded`
means the public seam is better covered by the named exact JSON, compatibility,
failure-path, process, or platform tests. An exclusion is deliberate coverage,
not an undocumented omission.

| Owner | Public module or export | Classification | Evidence or exclusion |
| --- | --- | --- | --- |
| packet28-daemon-protocol | broker | excluded | wire-dto-json-compat-tests |
| packet28-daemon-protocol | commands | excluded | command-json-dispatch-tests |
| packet28-daemon-protocol | context_store | excluded | context-store-process-tests |
| packet28-daemon-protocol | frame | covered | protocol-frame-runnable |
| packet28-daemon-protocol | hooks | excluded | hook-ingest-json-tests |
| packet28-daemon-protocol | index | excluded | index-state-process-tests |
| packet28-daemon-protocol | message | excluded | request-response-json-tests |
| packet28-daemon-protocol | paths | excluded | path-endpoint-tests |
| packet28-daemon-protocol | task | covered | protocol-task-lifecycle-runnable+compile_fail |
| packet28-daemon-core | integrity | excluded | integrity-corruption-tests |
| packet28-daemon-core | retention | excluded | retention-recovery-process-tests |
| packet28-daemon-core | storage | covered | daemon-core-storage-runnable |
| packet28-daemon-core | task_store_lease | excluded | lease-authority-process-tests |
| packet28-daemon-core | trust | excluded | trust-platform-tests |
| packet28d | serve | excluded | non-hermetic-process-lifecycle-owner |
| packet28d | shared_repository_scan | covered | packet28d-shared-scan-no_run+feature-shared-repository-scan |

<!-- packet28d-public owner=packet28-daemon-protocol item=broker classification=excluded evidence=wire-dto-json-compat-tests -->
<!-- packet28d-public owner=packet28-daemon-protocol item=commands classification=excluded evidence=command-json-dispatch-tests -->
<!-- packet28d-public owner=packet28-daemon-protocol item=context_store classification=excluded evidence=context-store-process-tests -->
<!-- packet28d-public owner=packet28-daemon-protocol item=frame classification=covered evidence=protocol-frame-runnable -->
<!-- packet28d-public owner=packet28-daemon-protocol item=hooks classification=excluded evidence=hook-ingest-json-tests -->
<!-- packet28d-public owner=packet28-daemon-protocol item=index classification=excluded evidence=index-state-process-tests -->
<!-- packet28d-public owner=packet28-daemon-protocol item=message classification=excluded evidence=request-response-json-tests -->
<!-- packet28d-public owner=packet28-daemon-protocol item=paths classification=excluded evidence=path-endpoint-tests -->
<!-- packet28d-public owner=packet28-daemon-protocol item=task classification=covered evidence=protocol-task-lifecycle-runnable+compile_fail -->
<!-- packet28d-public owner=packet28-daemon-core item=integrity classification=excluded evidence=integrity-corruption-tests -->
<!-- packet28d-public owner=packet28-daemon-core item=retention classification=excluded evidence=retention-recovery-process-tests -->
<!-- packet28d-public owner=packet28-daemon-core item=storage classification=covered evidence=daemon-core-storage-runnable -->
<!-- packet28d-public owner=packet28-daemon-core item=task_store_lease classification=excluded evidence=lease-authority-process-tests -->
<!-- packet28d-public owner=packet28-daemon-core item=trust classification=excluded evidence=trust-platform-tests -->
<!-- packet28d-public owner=packet28d item=serve classification=excluded evidence=non-hermetic-process-lifecycle-owner -->
<!-- packet28d-public owner=packet28d item=shared_repository_scan classification=covered evidence=packet28d-shared-scan-no_run+feature-shared-repository-scan -->

`packet28d::serve` is intentionally excluded from a runnable happy-path
doctest. It changes the process working directory, acquires workspace leases,
publishes runtime files, binds a listener, and blocks until shutdown, so a
runnable doctest would not be hermetic. Lifecycle and process tests cover that
boundary instead.

The checker also requires each source anchor below to resolve to exactly one
Rustdoc fence of the declared kind and to contain the relevant API operations.
This covers the public `DaemonCoreError` re-export and the frozen core-root
compatibility boundary in addition to the module rows.

| Anchor | Source | Fence |
| --- | --- | --- |
| protocol-frame-runnable | crates/packet28-daemon-protocol/src/frame.rs | runnable |
| protocol-task-lifecycle-runnable | crates/packet28-daemon-protocol/src/task.rs | runnable |
| protocol-task-lifecycle-compile_fail | crates/packet28-daemon-protocol/src/task.rs | compile_fail |
| daemon-core-storage-runnable | crates/packet28-daemon-core/src/lib.rs | runnable |
| daemon-core-error-source-chain-runnable | crates/packet28-daemon-core/src/error.rs | runnable |
| daemon-core-root-compatibility-compile_fail | crates/packet28-daemon-core/src/lib.rs | compile_fail |
| packet28d-shared-scan-no_run | crates/packet28d/src/shared_repository_scan.rs | no_run |

<!-- packet28d-anchor id=protocol-frame-runnable source=crates/packet28-daemon-protocol/src/frame.rs fence=runnable -->
<!-- packet28d-anchor id=protocol-task-lifecycle-runnable source=crates/packet28-daemon-protocol/src/task.rs fence=runnable -->
<!-- packet28d-anchor id=protocol-task-lifecycle-compile_fail source=crates/packet28-daemon-protocol/src/task.rs fence=compile_fail -->
<!-- packet28d-anchor id=daemon-core-storage-runnable source=crates/packet28-daemon-core/src/lib.rs fence=runnable -->
<!-- packet28d-anchor id=daemon-core-error-source-chain-runnable source=crates/packet28-daemon-core/src/error.rs fence=runnable -->
<!-- packet28d-anchor id=daemon-core-root-compatibility-compile_fail source=crates/packet28-daemon-core/src/lib.rs fence=compile_fail -->
<!-- packet28d-anchor id=packet28d-shared-scan-no_run source=crates/packet28d/src/shared_repository_scan.rs fence=no_run -->

## Startup and request flow

`packet28d::serve(root)` resolves the nearest repository root and owns this
order:

1. Change to the resolved workspace, acquire the single-daemon instance lease,
   recover task-store quarantine under authority, and retain the task-store
   lease.
2. Ensure the daemon directory, load runtime configuration, bind the preferred
   Unix transport or its fallback, generate a fresh owner capability when TCP
   is selected, and write initial owner-only `runtime.json` metadata with no
   readiness timestamp.
3. Construct the persistent kernel registry, load task/watch registries and
   authenticated event-log tails, start the single persistence owner, and
   reconcile lagging registry high-water marks.
4. Load index state, construct `DaemonState`, restore watchers, and enqueue
   initial index work.
5. Rewrite `runtime.json` with `ready_at_unix`, publish the readiness file, then
   create the Tokio runtime and start the transport, watch, background, and
   index owners.
6. Authenticate the accepted Unix peer UID or the TCP capability prelude,
   decode a bounded protocol frame, dispatch it in `server`, cross the bounded
   blocking pool for synchronous filesystem/kernel/index work, flush request
   persistence, and encode one response frame.

The listener and initial runtime metadata therefore exist before registry load
and persistence startup, but clients must wait for readiness before sending
work. The protocol crate remains usable by clients without linking runtime or
storage implementations.

## Persistence, admission, and recovery

Task-event logs are the durable sequence authority. An append validates the
existing authenticated tail, allocates the next sequence, synchronizes the
frame, advances the in-memory high-water mark, and queues an immutable
task/watch snapshot. The persistence owner serializes and publishes the latest
snapshot after the daemon-state mutex has been released; its bounded debounce
coalesces bursts without making failure invisible.

A task must be durably admitted to the registry before any managed artifact or
event namespace is created. Artifact and hook writers call the admission fence,
which waits for the persistence owner to publish the required revision. Startup
then either reconciles a lagging registry from valid event tails or fails
closed if a registry is ahead of its log, a durable frame is corrupt, or
recovery authority is conflicted.

Detailed retention, journal, corruption, and descriptor-confinement guarantees
are in [Task-store retention](task-store-retention.md).

## Transport, endpoint discovery, and compatibility

Unix domain sockets are the primary transport. A workspace-local Unix socket
is the permission fallback; forced TCP and Unix-permission failure use a
loopback TCP listener. Both transports use an eight-byte big-endian length
followed by one bounded JSON value, the same cancellation signal, connection
cap, read/write deadlines, and owned Tokio connection task set.
TCP authentication has its own smaller pending-connection cap and one-second
default deadline. An unauthenticated peer therefore cannot consume the normal
authenticated connection budget; both controls are independently configurable.

Clients must discover the endpoint from `.packet28/daemon/runtime.json` after
the readiness file appears. Its `socket_path` field is the selected endpoint:
either a Unix path or `tcp://127.0.0.1:<port>`. The conventional socket path is
not authoritative when fallback transport is active. Runtime discovery is
published as an authenticated owner-only regular file. A TCP runtime also
contains `transport_auth`, a redacted-in-debug 256-bit per-instance capability.
Capability-bearing metadata is rejected if any group or other permission bit
is present, including read-only exposure; legacy secret-free Unix metadata may
remain owner-readable with conventional `0644` permissions.
The client sends that value as the first framed message and waits for an
authentication acknowledgement before sending any `DaemonRequest`.

Unix accepts fail closed unless the operating-system peer credential UID
matches the daemon's effective UID. TCP accepts fail closed on a missing,
malformed, stale, or incorrect capability; authentication failures all use the
same response and cannot dispatch commands. A `0.2.x` runtime file that names a
TCP endpoint without `transport_auth` is treated as a legacy insecure daemon:
the upgraded client reports an explicit stop-and-restart migration error
instead of using it.

Subscriber and watch queues are bounded. A slow subscriber is disconnected
after its queued frames drain and resumes from its last sequence with
`TaskSubscribe.after_seq`. Watch overflow is coalesced for the current
generation and reported on the next trigger before conservative replanning.

`packet28-daemon-protocol` is the preferred client dependency.
`packet28-daemon-core` retains an explicit, frozen root facade through the
`0.2.x` line for source compatibility; new protocol items are not added there.
Protocol framing returns `FrameError`, daemon storage returns
`DaemonCoreError`, and `packet28d::serve` adds application context at the
process boundary with `anyhow::Error`.

## Request completion and legacy errors

Every ordinary blocking request runs its handler and then requests a
persistence flush even when the handler failed. A successful handler with a
failed flush becomes a failed request rather than returning a success that was
not checkpointed. If both fail, the handler failure remains in the internal
source chain and the flush failure is attached as context.

The legacy runtime wire boundary represents all such failures as
`DaemonResponse::Error { message: String }`. That conversion preserves a
human-readable message but erases typed `FrameError`, `DaemonCoreError`, and
`anyhow` source-chain structure from the response. Clients must not infer a
stable typed error kind from the string.

Clap reports standalone `packet28d` argument errors with status `2`. For
startup, runtime, persistence, or cleanup failure, `main` prints the error
chain to stderr and also exits with status `2`.

## `TaskCancel` contract

`TaskCancel` has a dedicated bounded blocking lane, separate from both normal
work and the short control-codec lane, so saturated data work cannot prevent
cancellation or `Stop` progress. For an existing task, cancellation:

1. Obtains the current generation (creating its token if needed), marks that
   generation cancelled, moves the task lifecycle to cancelling, and queues
   persistence.
2. Removes every registered watch and watcher handle.
3. Sends `SIGTERM` to each owned delegated process group, allows a 250 ms grace,
   escalates remaining groups to `SIGKILL`, and waits up to five seconds for
   child waiters to reap them.
4. Waits up to 30 seconds for all generation operations and children to become
   idle. Generation checks prevent late completion, context, artifact, or
   replacement publication.
5. Removes the task, subscribers, and current generation, queues the final
   state, and returns the removed task and watch IDs only after the enclosing
   request flush succeeds.

A missing task is idempotent and returns `task: null` with no removed watch
IDs. A process, quiescence, state, or flush failure returns the legacy
`DaemonResponse::Error` string; it does not misreport successful durable
cancellation.

## Shutdown

A stop request first acknowledges on the active transport and requests the
shared shutdown signal. The supervisor then:

1. Withdraws readiness, marks state as shutting down, requests cancellation for
   every active task generation, clears watcher ownership, and stops new index
   work.
2. Stops accepting connections and joins transport, watch, background, and
   index owners within one grace deadline.
3. Waits for admitted blocking mutations; work that cannot be cancelled keeps
   its lifecycle leases until it actually exits.
4. Shuts down persistent kernel caches and flushes the persistence owner.
5. Removes runtime files, then releases task-store and daemon-instance leases.

The first lifecycle failure is the error returned by `serve`. Later cleanup
failures are logged as additional shutdown failures and do not replace or join
that returned error.

## Validation map

The canonical gate runs the architecture checker and the full workspace tests.
Focused daemon evidence is:

```text
cargo check -p packet28d --all-targets --all-features --locked
cargo clippy -p packet28d --all-targets --all-features --locked -- -D warnings
cargo test -p packet28d --all-features --locked
cargo test -p packet28-daemon-protocol --all-features --locked --doc
cargo test -p packet28-daemon-core --all-features --locked --doc
python3 scripts/check_architecture.py
python3 -m unittest scripts.tests.test_check_architecture
```

The daemon tests include authenticated TCP success, missing/wrong TCP
capability rejection, Unix peer-owner enforcement, endpoint metadata
permissions, TCP/Unix stop parity, connection caps and deadlines,
slow-subscriber replay, cancellation generation fencing, process-group reap,
persistence failure and bounded shutdown, startup high-water reconciliation,
artifact-admission ordering, and task-store corruption/recovery cases.
