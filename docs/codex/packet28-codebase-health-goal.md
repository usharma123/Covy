You are working in the Packet28 codebase. Perform a large, careful codebase health refactor focused on maintainability, correctness, and long-term usability by both humans and LLM agents.

This is not a rewrite. Preserve Packet28’s core workflow, CLI behavior, reducer behavior, packet schema behavior, search/index behavior, tests, and public-facing semantics unless something is clearly deprecated, unused, redundant, or harmful.

Primary goals:

1. Remove dead code
- Identify and remove unreachable code, unused modules, unused functions, unused structs/enums, stale helpers, obsolete adapters, deprecated paths, and redundant compatibility layers.
- Do not remove smart fallbacks that are still useful for Packet28’s workflow.
- Before deleting anything non-trivial, verify references through static search, tests, and nearby documentation.

2. Remove unused imports and clean module boundaries
- Remove unused imports, unused dependencies, duplicated utilities, and unnecessary re-exports.
- Improve module organization so files have clear responsibilities.
- Prefer small, cohesive modules over large mixed-purpose files.

3. Apply Rust and CLI best practices
- Prefer simple, idiomatic Rust.
- Improve error handling where it is brittle, unclear, or overly broad.
- Avoid unnecessary cloning, string allocation, nested conditionals, and over-engineered abstractions.
- Keep behavior deterministic, especially for packet output, schemas, budgets, provenance, reducers, and search results.
- Preserve backwards compatibility where it matters for real Packet28 workflows.

4. Eliminate god classes / god modules
- Identify files, structs, or modules doing too many unrelated things.
- Split them into cohesive units with clear names.
- Keep the call graph understandable for both humans and LLM agents.
- Avoid creating abstraction layers unless they reduce complexity.

5. Reduce massive test classes / files
- Split huge test files into focused test modules.
- Group tests by behavior, command, reducer, packet type, or workflow.
- Remove duplicate tests while preserving meaningful coverage.
- Keep tests readable and deterministic.
- Do not weaken the test suite just to make refactoring easier.

6. Optimize the codebase for LLM and human navigation
- Prefer predictable file names, small modules, obvious boundaries, and explicit data flow.
- Keep important workflows easy to trace from CLI entrypoint to reducer/output.
- Avoid cleverness.
- Add brief comments only when they clarify non-obvious protocol, schema, fallback, or performance decisions.
- Remove stale comments and outdated documentation.

7. Remove deprecated or unnecessary workflow paths
- Audit old paths, flags, features, adapters, compatibility shims, and fallbacks.
- Remove anything not needed for the current Packet28 workflow.
- Keep only smart fallbacks that improve reliability, portability, or graceful degradation.
- If something looks deprecated but uncertain, isolate it and document the reason before removal.

Execution rules:

- Work incrementally.
- Start by inspecting the project structure, Cargo files, tests, CLI entrypoints, reducer modules, packet schema modules, search/index modules, and existing docs.
- Create a concise refactor plan with target areas.
- Refactor in small batches.
- After each meaningful batch, run the relevant formatter, linter, and tests.
- Prefer the fast batch gate for ordinary incremental commits:
  - scripts/validate_refactor_batch.sh
- During local iteration, use the test-only selector before the commit gate when the full lint path is slowing feedback:
  - scripts/validate_refactor_batch.sh --tests-only
  - scripts/validate_refactor_batch.sh --list
- The fast batch gate must run:
  - cargo fmt --check
  - package-scoped cargo clippy for changed Rust packages
  - targeted cargo test commands for changed crates or behavior filters
- Run the full workspace lint and test gate at major checkpoints, before final push, and before claiming the goal complete:
  - cargo clippy --all-targets --all-features -- -D warnings
  - cargo test --all-features
- If there are project-specific test or benchmark commands in the repo docs, run those too.
- All tests must pass before committing.
- Do not commit broken code.
- Do not push broken code.
- Do not force push.
- Do not hide failing tests.
- Do not delete tests unless they are truly redundant, obsolete, or testing removed dead behavior.

Git workflow:

- First check git status.
- If the working tree has unrelated user changes, do not overwrite them.
- Create or use a dedicated refactor branch, for example:
  refactor/codebase-health-goal
- Commit after each verified refactor batch.
- Each commit should be atomic and have a clear message, such as:
  - refactor: remove unused packet helpers
  - refactor: split reducer tests by behavior
  - refactor: simplify search fallback boundaries
  - chore: remove stale imports and dead modules
- Push each passing commit or passing batch to the remote branch.
- Before the final push, run the full validation suite again.

Final output:

When finished, provide:
1. Summary of what changed
2. List of removed dead/deprecated code
3. List of retained smart fallbacks and why they remain
4. Test and lint commands run, with pass/fail status
5. Commit hashes created
6. Remote branch pushed
7. Any remaining refactor targets that should be handled separately

Important constraints:

- Preserve Packet28’s core identity as a deterministic context packet/protocol layer for agents.
- Do not make cosmetic churn just for the sake of churn.
- Prefer boring, obvious, maintainable code.
- Optimize for correctness first, then simplicity, then performance.
- If a risky change is discovered, isolate it into its own commit or leave it as a documented follow-up instead of mixing it into unrelated cleanup.
