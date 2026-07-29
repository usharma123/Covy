# Task-store inspection and retention

Packet28 exposes supported, timestamped observations for workspace-local task
state. Values from a particular machine or audit are snapshots, not product
constants:

```console
Packet28 daemon storage inspect --root /path/to/workspace
Packet28 daemon storage inspect --root /path/to/workspace --json --pretty
```

The report distinguishes the logical size of the whole `.packet28` tree from
the task state governed by retention. Managed task bytes are the sum of compact
serialized task-registry records, task artifacts, and daemon task-event logs.
The retention bound uses these stable logical file bytes. Reports also expose
native allocated filesystem bytes for the whole state tree, registry, artifact
tree, event tree, and their managed total; `allocated_bytes_supported` is false
when those fields must fall back to logical size. Every report includes
`observed_at_unix`, a schema version, record and entry counts, oldest/newest
known task timestamps, active-task count, corruption/safety issues, and before
and after metrics. Historical registry fields that were persisted in Unix
milliseconds are normalized to seconds before age comparison and reporting.

## Plan before applying

Cleanup is a dry run unless `--apply` is present, and at least one explicit
bound is required:

```console
# Select tasks strictly older than seven days without changing state.
Packet28 daemon storage cleanup \
  --root /path/to/workspace \
  --max-age-seconds 604800

# Preview the oldest-first plan needed to reach 512 MiB.
Packet28 daemon storage cleanup \
  --root /path/to/workspace \
  --max-bytes 536870912 \
  --json --pretty

# Apply both bounds after reviewing the plan and stopping packet28d.
Packet28 daemon storage cleanup \
  --root /path/to/workspace \
  --max-age-seconds 604800 \
  --max-bytes 536870912 \
  --apply
```

Boundaries are exact: a task exactly at the age limit is retained, and no
size-based candidate is selected when managed bytes equal the size limit.
Size cleanup selects the oldest eligible candidates first. Applying an
age-based plan may also bring the store below a configured size bound.

## Safety model

Retention resolves the workspace and requires `.packet28` to be a real
workspace-local directory. Symlinked state and candidates are protected, and
symlinks are never followed. Active tasks, storage-key collisions, unreadable
or special entries, malformed registry or active-task state, and candidates
that change after inspection are protected and reported.

Apply is supported on Unix platforms. A running daemon holds a shared
task-store lifecycle lease from before it loads mutable state through shutdown;
apply takes the corresponding exclusive lease for its complete cleanup and
reporting window. If the daemon wins the race, apply fails without mutation. If
cleanup wins, daemon startup waits until cleanup releases the lease. The
readiness marker remains an additional compatibility check, so stop
`packet28d` before applying a reviewed plan.

Each candidate is re-scanned, checked by filesystem identity and a nanosecond
metadata-tree fingerprint, moved into a workspace-local quarantine, and
checked again for active or changed state. Registry records are removed only
if their complete serialized values still match under the registry's exclusive
interprocess lock. Pre-commit races are restored or skipped, cleanup failures
are surfaced, and unrelated state and registry records are preserved.

The JSON format is the automation interface. Consumers should check
`schema_version`, retain the observation timestamp with any recorded values,
and surface `issues` plus `remaining_over_limit_bytes` rather than assuming a
requested bound was achievable.

A [timestamped remediation-worktree snapshot](task-store-metrics/20260728/README.md)
records the volatile evidence separately from this interface contract.
