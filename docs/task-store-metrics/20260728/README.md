# Current task-store metrics snapshots

Both artifacts were produced by the pending PER-08/PER-11 implementation with
`target/debug/Packet28 daemon storage inspect --root <root> --json --pretty`.

- [`audited-checkout.json`](audited-checkout.json) is the current snapshot of
  the checkout whose local state the audit sampled. It was captured at
  `2026-07-28T23:49:55Z` from baseline revision
  `9b1be5b6de0ca900ef8489d88d55b31b48ce2e6a`.
- [`current-worktree.json`](current-worktree.json) is the isolated remediation
  worktree snapshot captured at `2026-07-28T23:49:55Z` from baseline revision
  `829ebe7042130da8f6b976cf400dd4d848928ca6`.

These are timestamped observations, not product constants. The audited
checkout still had 300 registry records, while its logical byte and file
counts had changed since the audit. Its 169,574,400 allocated state bytes
match the same-time `du` observation of 165,600 KiB; the report additionally
attributes 45,051,904 allocated bytes to artifacts and 761,856 to task events.
The remediation worktree had no task registry, artifacts, or event logs at
capture time. Keeping both artifacts demonstrates the supported measurement
contract and preserves the evidence boundary around volatile local-store
figures.
