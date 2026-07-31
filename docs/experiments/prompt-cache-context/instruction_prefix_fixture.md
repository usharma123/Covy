# Packet28 repository instructions

Preserve public behavior unless a defect is demonstrated. Keep changes reviewable, validate failure paths, and record mechanically checkable evidence for every claimed improvement.

## Authentication

Authorization must be checked before protected data is read. Preserve session boundaries, reject expired credentials, and cover both successful and rejected requests with deterministic tests.

## Cache

Cache identities must include every stable input that can affect output bytes. Mutable task state must either be fingerprinted canonically or bypass caching so same-task snapshot drift cannot return stale output.

## Compaction

Keep the stable instruction prefix separate from the replaceable broker brief. Compaction may refresh mutable focus, decisions, questions, and next actions without changing repository-owned instruction bytes.

## Release

Use locked dependencies, strict lint and documentation checks, architecture verification, and reproducible packaging commands. Preserve raw benchmark observations and machine metadata.
