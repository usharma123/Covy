# Compatibility facade

`context-kernel-core` preserves the selected Packet28 `0.2` root surface.
New custom compositions should depend on `context-kernel-mechanism`; new users
of the supported Packet28 target catalog may depend on
`context-kernel-builtins`.

## Reviewed root inventory

The facade exports exactly 45 names: the 53-name builtins inventory except
`KernelMechanism`, `KernelServices`, `ExecutionPolicy`, `ExecutionPolicyRun`,
`ReactivePlanner`, `ReactivePlan`, `ReactivePlanRequest`, and
`KernelPlanMutation`. Those eight composition-only names remain available from
the mechanism or builtins crate and are deliberately excluded here.

<!-- public-surface:kernel-compatibility -->
## Legacy execution

```rust
use context_kernel_core::{execute, KernelRequest};
use serde_json::json;

let response = execute(KernelRequest {
    target: "packet28.instruction.summarize".to_string(),
    reducer_input: json!({
        "path": "AGENTS.md",
        "content": "# Validate\n\nRun focused tests."
    }),
    policy_context: json!({"disable_cache": true}),
    ..KernelRequest::default()
})?;

assert_eq!(response.target, "packet28.instruction.summarize");
assert_eq!(response.output_packets.len(), 1);
# Ok::<(), context_kernel_core::KernelError>(())
```

The facade intentionally re-exports the legacy constructors, execution and
sequence APIs, selected wire and error types, built-in diff/test adapters, and
instruction renderer helpers. It does not re-export the generic composition
SPI:

```compile_fail
use context_kernel_core::KernelMechanism;
```

```compile_fail
use context_kernel_core::KernelServices;
```

Import those types from `context-kernel-mechanism` when building a custom
catalog. `Kernel` retains its existing dereference compatibility with the
mechanism, but that is not the advertised custom-composition path.

## Lifecycle, errors, and exclusions

Execution, scheduling, cooperative cancellation, cache persistence, and
[`KernelError`] behavior are inherited unchanged from the mechanism and
built-in composition. The compatibility facade adds no runtime owner, error
variant, panic boundary, or unsafe API.

The generic policy/planner SPI, raw mechanism type, internal reducer adapters,
scheduler implementation, context-memory implementation, evidence harnesses,
and daemon runtime are intentionally outside this selected root facade.
