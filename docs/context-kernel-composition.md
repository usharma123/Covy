# Context-kernel composition boundary

Packet28 separates the kernel into three dependency layers:

1. `context-kernel-mechanism` owns target-neutral execution, budgets, cache and
   persistence lifecycle, scheduling, cancellation, governance interfaces, and
   reactive-plan application. It has no dependency on a built-in reducer crate
   and contains no built-in target names.
2. `context-kernel-builtins` owns the 16 version-one target registrations,
   concrete reducer adapters, Guardy policy implementation, built-in cache
   identity rules, and task-aware reactive planning.
3. `context-kernel-core` is the `0.2` compatibility facade. Its only normal
   dependency is `context-kernel-builtins`, and it re-exports the supported
   legacy root API.

The dependency direction is:

```text
context-kernel-core
        |
        v
context-kernel-builtins
        |
        v
context-kernel-mechanism
```

## Existing callers

No migration is required for `0.2` callers. The following remain source
compatible:

- `Kernel::new()`
- `Kernel::with_persistence(...)`
- `Kernel::with_v1_reducers()`
- `Kernel::with_v1_reducers_and_persistence(...)`
- custom `register_reducer` closures
- execution, sequence, observer, cache, persistence, and context-store methods
- root wire types, errors, renderer/diff helpers, and free functions

The facade's compile test names the full supported root surface, while the
built-in behavior suite and exact registry test protect routing and wire
behavior.

## Custom compositions

New applications that do not need Packet28's catalog can depend directly on
`context-kernel-mechanism`. Construct a `KernelMechanism`, inject
`KernelServices` when custom governance or reactive planning is required, and
register only the application's reducer targets.

Applications that want Packet28's supported catalog without the legacy crate
name can depend on `context-kernel-builtins` and use its `Kernel`.

Do not add tool-specific dependencies or target strings to
`context-kernel-mechanism`. The architecture gate checks normal and transitive
Cargo edges, rejects concrete source identifiers and target literals in the
mechanism, verifies the exact 16-target registry owner, and keeps
`context-kernel-core` a one-edge facade.
