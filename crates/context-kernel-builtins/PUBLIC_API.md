# Supported public surface

`context-kernel-builtins` composes the generic mechanism with Packet28 policy,
reactive planning, and the exact version-one target catalog.
[`Kernel::new`] installs Packet28 services but starts with an empty registry;
[`Kernel::with_v1_reducers`] additionally registers all supported targets.

## Reviewed root inventory

The stable root inventory contains the 35 mechanism names plus these 18
composition-owned names:

- `Kernel`;
- `execute`, `execute_sequence`, and `register_v1_reducers`;
- `build_diff_analyze_envelope`, `build_diff_pipeline_request`,
  `build_test_impact_envelope`, `DiffAnalyzeKernelInput`,
  `DiffAnalyzeKernelOutput`, `ImpactKernelInput`, `ImpactKernelOutput`, and
  `SerializableFileDiff`;
- `render_instruction`, `InstructionSummaryPayload`,
  `InstructionSummaryRequest`, `RenderedInstructionSummary`,
  `DEFAULT_INSTRUCTION_SUMMARY_BUDGET_TOKENS`, and
  `INSTRUCTION_SUMMARY_SCHEMA_VERSION`.

<!-- public-surface:builtins-catalog -->
## Built-in catalog execution

```rust
use context_kernel_builtins::{Kernel, KernelRequest};
use serde_json::json;

let kernel = Kernel::with_v1_reducers();
let response = kernel.execute(KernelRequest {
    target: "packet28.instruction.summarize".to_string(),
    reducer_input: json!({
        "path": "AGENTS.md",
        "content": "# Build\n\nRun the focused tests before committing."
    }),
    policy_context: json!({"disable_cache": true}),
    ..KernelRequest::default()
})?;

assert_eq!(response.target, "packet28.instruction.summarize");
assert_eq!(response.output_packets.len(), 1);
# Ok::<(), context_kernel_builtins::KernelError>(())
```

The version-one catalog is:

```text
agenty.state.write              agenty.state.snapshot
packet28.instruction.summarize  packet28.broker_memory.write
contextq.correlate              contextq.manage
contextq.assemble               governed.assemble
guardy.check                    diffy.analyze
testy.impact                    stacky.slice
buildy.reduce                   proxy.run
mapy.repo                       mapy.query
```

The free `execute` and `execute_sequence` functions construct this catalog for
one call. `register_v1_reducers` installs the same catalog on an existing
[`Kernel`].

## Composition, lifecycle, and errors

`Kernel` forwards execution, sequence, observer, cache, context-store, and
persistence operations to `KernelMechanism`. Scheduler, cooperative
cancellation, persistence, and [`KernelError`] behavior are therefore the
contracts documented by `context-kernel-mechanism`.

`with_persistence` and `with_v1_reducers_and_persistence` preserve the
mechanism's in-memory fallback when persistence cannot open. Explicit bounded
flush or shutdown remains the durability contract.
Services that cannot safely run without persistence should use
`try_with_persistence` or `try_with_v1_reducers_and_persistence`; these
constructors fail instead of returning a memory-only kernel.

## Reviewed exclusions

- The mechanism SPI is re-exported for existing direct builtins users, but new
  custom compositions should depend on `context-kernel-mechanism`.
- Instruction rendering and diff/test adapter helpers are supported utilities
  inside the built-in composition family, not separate kernel lifecycle
  families.
- Concrete reducers, cache-key helpers, Packet28 policy/planner
  implementations, correlation helpers, and target strings remain internal.
- The instruction-prefix executable example is an evidence harness, not a
  public API family. Daemon runtime APIs are outside this crate.
