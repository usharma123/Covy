# Daemon Protocol

## Overview

`packet28d` is a local framed daemon that provides persistent state, file
watching, task lifecycle management, and command routing for long-running
agent workflows. The authoritative lifecycle, cancellation, persistence, and
error contract is [Packet28 daemon runtime](../docs/daemon-runtime.md).

## Transport

- Unix domain socket by preference, with a workspace-local Unix fallback and a
  loopback TCP fallback
- One Tokio-owned accept loop and bounded connection task set
- Eight-byte big-endian length prefix followed by one bounded JSON request or
  response value
- Connection-scoped requests, except task subscriptions, which retain the
  connection for bounded event streaming

## Lifecycle

```bash
# Start the daemon (auto-starts if not running when --via-daemon is used)
Packet28 daemon start --root .

# Check status
Packet28 daemon status --root . --json

# Stop the daemon
Packet28 daemon stop --root .
```

Runtime info is persisted to `.packet28/daemon/runtime.json`:
- `pid`: Daemon process ID
- `socket_path`: Selected Unix path or `tcp://127.0.0.1:<port>` endpoint
- `started_at_unix`: Startup timestamp
- `ready_at_unix`: Readiness timestamp once startup is complete

Clients wait for the readiness file and then read `runtime.json`; the
conventional `.sock` path is not authoritative when fallback transport is
active.

## Request / Response Protocol

All requests and responses are JSON-serialized `DaemonRequest` / `DaemonResponse` enums.

### Kernel Execution

```
DaemonRequest::Execute { request: KernelRequest }
→ DaemonResponse::Execute { response: KernelResponse }
```

Routes a single `KernelRequest` through the daemon's kernel instance. The daemon's kernel shares a persistent `PacketCache`, so results from prior requests are cached and available for recall.

### Task Submission

```
DaemonRequest::ExecuteSequence { spec: TaskSubmitSpec }
→ DaemonResponse::ExecuteSequence { response, task: TaskRecord, watches: Vec<WatchRegistration> }
```

Submits a multi-step task with optional file watches. The `TaskSubmitSpec` contains:

- `sequence: KernelSequenceRequest` — DAG of kernel steps with dependencies
- `watches: Vec<WatchSpec>` — File/git/test report watchers

Step IDs are auto-generated if blank or missing (via `normalize_sequence_request`).

### Task Lifecycle

```
DaemonRequest::TaskStatus { task_id }
→ DaemonResponse::TaskStatus { task: TaskRecord }

DaemonRegistryRequestV1::TaskListPage {
  request: { snapshot_revision, after_task_id, limit }
}
→ DaemonRegistryResponseV1::TaskListPage {
  page: { snapshot_revision, tasks, next_after_task_id, total }
}

DaemonRequest::TaskCancel { task_id }
→ DaemonResponse::TaskCancel { task, removed_watch_ids }
```

Failed tasks automatically clean up their watches.

`TaskCancel` uses a reserved cancellation lane. It generation-fences work,
removes watches, terminates and reaps owned process groups with bounded
TERM-to-KILL escalation, waits for quiescence, and flushes the resulting state
before returning success. A missing task is an idempotent success. See the
[full cancellation contract](../docs/daemon-runtime.md#taskcancel-contract).

### Task Streaming

```
DaemonRequest::TaskSubscribe { task_id, replay_last, after_seq }
→ DaemonResponse::TaskSubscribeAck { task_id, replayed, after_seq }
→ (streaming) step_started, step_completed, step_failed, replan_applied, context_updated
```

After the initial ack, the connection stays open and the daemon streams per-step lifecycle events. Events include:

- `step_started`: Step execution began
- `step_completed`: Step finished successfully
- `step_failed`: Step failed with error
- `replan_applied`: Reactive mutation applied to the sequence
- `context_updated`: Summary of working set tokens and evictable tokens

`replay_last` is an integer tail count. `0` replays every event after the
optional `after_seq` cursor; a positive value replays at most that many of the
newest events after the cursor. Live events continue from the acknowledged
sequence.

### Watch Management

```
DaemonRequest::WatchList { task_id }
→ DaemonResponse::WatchList { watches }

DaemonRegistryRequestV1::WatchListPage {
  request: { snapshot_revision, task_id, after_watch_id, limit }
}
→ DaemonRegistryResponseV1::WatchListPage {
  page: { snapshot_revision, watches, next_after_watch_id, total }
}

DaemonRequest::WatchRemove { watch_id }
→ DaemonResponse::WatchRemove { removed }
```

Task and watch pages are additive registry V1 messages with the wire tags
`task_list_page_v1` and `watch_list_page_v1`. They are ordered by identifier
and use the last returned identifier as an exclusive cursor. Limits must be
between 1 and 256. Every continuation echoes the first response's
`snapshot_revision`, which carries a random daemon-instance ID and monotonic
mutation counter. A mutation or daemon restart between pages is rejected
rather than mixing registry states. Pages reject stale cursors and individual records above
1 MiB explicitly, and the complete response stays below 4 MiB. The CLI uses
one authenticated connection for a complete traversal while preserving the
existing `daemon watch list` output.

### Liveness Status

The frozen legacy `DaemonRequest::Status` response remains exhaustive when its
task and watch vectors fit the 1 MiB response budget. Otherwise it returns an
explicit error; it never returns an undisclosed prefix.

`DaemonRegistryRequestV1::Status` (`registry_status_v1` on the wire) is the
bounded liveness surface. `task_count` and `watch_count` report the full live
registry sizes, `registry_revision` fences the page requests above, and
`index_truncated` discloses omitted index detail. New clients can still decode
an old daemon's exhaustive legacy status by deriving counts from its vectors.

Watch kinds:
- `File`: Glob pattern matching (e.g. `src/**/*.rs`)
- `Git`: Git ref change detection
- `TestReport`: Test result file monitoring

### Context Operations

```
DaemonRequest::ContextRecall { request }
→ DaemonResponse::ContextRecall { response }

DaemonRequest::ContextStoreList/Get/Prune/Stats { request }
→ DaemonResponse::ContextStore* { response }
```

These use the daemon's in-memory `PacketCache`, which persists to disk.

### Direct Domain Commands

```
DaemonRequest::CoverCheck { request }
→ DaemonResponse::CoverCheck { response }
```

Some commands bypass the kernel for efficiency.

## File Watching and Replan

When watches detect file changes:

1. Events are debounce-coalesced via `PendingWatchEvent` with a `due_at` timestamp
2. On flush, the daemon triggers a reactive replan for the associated task
3. The replan refreshes task context using `ScheduleMutation` (cancel/replace/append steps)
4. Subscribers receive `replan_applied` and `context_updated` events

## Persistence

| File | Purpose |
| --- | --- |
| `.packet28/daemon/packet28d.sock` | Workspace-local Unix fallback; not every run uses it |
| `.packet28/daemon/runtime.json` | PID, selected endpoint, startup time, readiness time |
| `.packet28/daemon/packet28d.log` | Daemon log output |
| `.packet28/daemon/watch-registry-v1.json` | Active watches (survives restart) |
| `.packet28/daemon/task-registry-v1.json` | Task state (survives restart) |
| `.packet28/daemon/task-watch-checkpoint-v1.json` | Atomic commit manifest for the paired registries |
| `.packet28/daemon/tasks/<id>/events.jsonl` | Per-task event log |
| `.packet28/packet-cache-v3.bin` | Persistent packet-cache checkpoint |
| `.packet28/packet-cache-v3.wal` | Checksummed cache deltas between checkpoints |

## CLI Integration

Any Packet28 command can be routed through the daemon with `--via-daemon`:

```bash
Packet28 diff analyze --coverage report.xml --via-daemon --json
Packet28 map repo --repo-root . --via-daemon --json
Packet28 context recall --query "coverage gap" --via-daemon --json
```

The daemon auto-starts if not already running. `--daemon-root` overrides the workspace root for socket resolution.

## Error Handling

- Broken pipe on subscriber disconnect is suppressed (not logged as error)
- Failed task submissions clean up associated watches
- Socket write errors on benign disconnects are silently ignored
- Request state is flushed even when request handling fails; a flush failure
  converts an otherwise successful request into a failure
- Runtime failures cross the legacy wire as
  `DaemonResponse::Error { message: String }`; typed internal error chains are
  not preserved in that response
- The standalone `packet28d` CLI uses status `2` for argument errors; runtime
  failures print their error chain and also exit `2`
