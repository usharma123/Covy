# Instruction rendering modes

Packet28 instruction-file rewriting is conservative and experiment-gated.
When no mode is selected, the daemon returns `passthrough` and the runtime reads
the original bytes. Invalid, unreadable, or unsupported experiment
configuration—including unknown fields or an unavailable renderer
version—also fails open to passthrough.

## Selecting a mode

A repository can opt in with `packet28-instruction.json` at its root:

```json
{
  "schema_version": 1,
  "mode": "stable",
  "stable_config": {
    "renderer_version": 1,
    "max_sections": 4,
    "max_lines_per_section": 6,
    "max_focus_terms": 12,
    "focus_terms": ["authentication", "release"]
  }
}
```

Repository config schema `1` currently supports renderer version `1` only.
Future renderer behavior must use an explicit new version rather than relabeling
v1 output.

The daemon protocol also accepts `render_mode` and `stable_config` on an
individual context-resolution request. Linux and macOS preload shims accept
`PACKET28_INSTRUCTION_MODE=passthrough|stable|adaptive`; a repository
configuration is the recommended selection point when every runtime backend
should use the same variant. An explicit request or shim environment value
overrides repository configuration; an invalid or non-Unicode environment
value selects passthrough conservatively.

The modes are:

- `passthrough` (default): return the source text byte-for-byte and bypass the
  local renderer cache.
- `stable`: render only from source bytes, normalized display path, effective
  schema, effective budget, renderer version, and normalized repository
  configuration. It is the only locally cached variant.
- `adaptive`: add the active task and a canonical, render-relevant snapshot
  projection. It deliberately bypasses the local renderer cache so same-task
  snapshot drift cannot return stale bytes.

Rewritten content is still fail-open: when the result is not smaller than the
source, Packet28 returns passthrough with `not_smaller_than_original`.
Instruction paths are resolved to repository-relative identities; lexical
escapes and existing symlinks that resolve outside the repository also return
passthrough. The repository experiment config is subject to the same ownership
boundary, so an external symlink cannot opt a workspace into rewriting.

## Stable prefix and mutable brief

The stable renderer excludes task ID, agent family, backend, provider, focus
paths, decisions, questions, and next actions from its identity. Those mutable
values remain in the versioned Packet28 broker brief, whose supersession header
lets a worker replace stale context after task changes, compaction, or handoff.

Adaptive snapshot telemetry hashes only the bounded fields that can affect its
rendering: the first six focus paths, first eight focus symbols, and first four
open-question texts. Unrelated event counts, tool history, and decision state
cannot create apparent prefix drift.

## Placement in a provider prompt

Keep repository instructions and tool definitions in a stable prefix. Place
changing task briefs after it. A supersession header changes which brief is
current; it does not require editing an earlier conversation message. During an
active conversation, append a brief update only when it adds useful state. At
compaction or fresh-worker resume, assemble the latest brief without copying
superseded briefs. Compaction can require warming a new provider prefix.

Generated runtime guidance uses this distinction. It shares one workflow across
formats while retaining backend-specific tool names, rule frontmatter, and hook
capabilities. Installing updated guidance changes the prefix once; leave it
unchanged during the task.

[OpenAI's prompt-caching guidance](https://developers.openai.com/api/docs/guides/prompt-caching#how-to-optimize-prompt-caching)
recommends stable instructions/tool definitions and preserving earlier messages.
[Anthropic's prompt-caching documentation](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
defines a cumulative prefix through tools, system content, and messages, with
cache breakpoints. A change before a reusable boundary can invalidate reuse.
The host that builds the model request owns cache keys, breakpoints, and usage
telemetry. Packet28's renderer cache cannot set or prove those provider choices.

## Where cache-augmented generation fits

The original [CAG implementation](https://github.com/hhhuang/CAG) preloads a
bounded knowledge collection into the model's KV cache. Packet28's durable
packet store is not a model KV cache. Loading the entire repository into each
prompt would also work against bounded context and make source changes costly.

A compatible experiment would preload a small, versioned set of stable
repository rules or reference material, then retrieve fresh task-specific
evidence after that prefix. Keep it opt-in, bound its size, and invalidate it
when its source bytes change. Do not pad prompts merely to reach a provider's
cache threshold. Compare useful context coverage, input tokens, cache reads and
writes, latency, and actual cost against the existing bounded assembly. Local
hash stability and fewer rendered bytes establish neither provider savings nor
instruction adherence. No provider-backed CAG execution is implemented here.

## Telemetry evidence boundary

`packet28.instruction_cache_experiment.v1` records local renderer reuse and
keeps these provider observations separate:

- provider name and version;
- prompt-component order and cache boundary;
- cache-creation and cache-read tokens;
- cache-creation and cache-read costs;
- compaction rewarm tokens; and
- adherence result and method.

Unavailable values are serialized as `unknown` with a reason; local byte/token
estimates never substitute for provider cache telemetry. Churn, reuse multiple,
effective cache cost, and compaction rewarm remain unknown if any required
provider observation is missing. Local cache eligibility is recorded separately
from a hit, so deliberately bypassed passthrough/adaptive requests are not
reported as misses.

Run the controlled local experiment with:

```sh
python3 docs/experiments/prompt-cache-context/run_instruction_prefix_experiment.py
```

It repeats passthrough, stable, and adaptive rendering across cold start,
second request, compaction, task A→B, same-task snapshot drift, and
fresh-worker handoff. The versioned artifacts under
`docs/experiments/prompt-cache-context/20260728/` are local renderer/cache
evidence only; they do not establish provider cache placement, pricing,
adherence, or net savings.

## Migration notes

Older clients can omit the new request fields; omission selects passthrough.
Older rewrite responses that lack the new mode and hash fields still
deserialize with compatibility defaults. Integrations that intentionally
tested the former implicit task-adaptive rewrite must now opt into `adaptive`
explicitly or commit a repository experiment configuration.
