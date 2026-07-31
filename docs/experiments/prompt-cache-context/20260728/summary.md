# Instruction-prefix experiment — 2026-07-28

This is controlled local renderer/cache evidence. It made no provider request and therefore does not establish provider cache placement, cache-token savings, price savings, compaction rewarm cost, or model adherence.

- Result: `PASS`
- Git HEAD: `832e40f4c4810d0f04abd354372be0ef1e6be862`
- Dirty source snapshot: `57531413f56b19327e66a05cdf84aba5df58d8c232c1bfd8bb66ed93e92706bc`
- Repetitions: `3`
- Scenarios per mode: cold start, second request, compaction, task A→B, same-task snapshot drift, fresh-worker handoff

## Local observations

| Mode | Requests | Cache-eligible | Local cache hits | Unique rendered-prefix hashes |
|---|---:|---:|---:|---:|
| `passthrough` | 18 | 0 | 0 | 1 |
| `stable` | 18 | 18 | 12 | 1 |
| `adaptive` | 18 | 0 | 0 | 3 |

Stable mode is expected to have one rendered-prefix hash across all transitions and to miss only on cold start and fresh-worker handoff. Passthrough and adaptive modes intentionally bypass the local renderer cache.

## Mechanically checked invariants

- PASS `passthrough_r1_local_cache_pattern` — expected_hits=[false, false, false, false, false, false] actual_hits=[false, false, false, false, false, false] expected_eligible=false actual_eligible=[false, false, false, false, false, false]
- PASS `passthrough_r1_exact_source_bytes` — source and rendered hashes must match for all six transitions
- PASS `stable_r1_local_cache_pattern` — expected_hits=[false, true, true, true, true, false] actual_hits=[false, true, true, true, true, false] expected_eligible=true actual_eligible=[true, true, true, true, true, true]
- PASS `stable_r1_byte_identity` — unique_rendered_prefix_hashes=1
- PASS `adaptive_r1_local_cache_pattern` — expected_hits=[false, false, false, false, false, false] actual_hits=[false, false, false, false, false, false] expected_eligible=false actual_eligible=[false, false, false, false, false, false]
- PASS `adaptive_r1_transition_identity` — compaction without render-relevant drift stays fixed; task A→B and same-task snapshot drift change bytes; a fresh worker reproduces the latest snapshot bytes
- PASS `passthrough_r2_local_cache_pattern` — expected_hits=[false, false, false, false, false, false] actual_hits=[false, false, false, false, false, false] expected_eligible=false actual_eligible=[false, false, false, false, false, false]
- PASS `passthrough_r2_exact_source_bytes` — source and rendered hashes must match for all six transitions
- PASS `stable_r2_local_cache_pattern` — expected_hits=[false, true, true, true, true, false] actual_hits=[false, true, true, true, true, false] expected_eligible=true actual_eligible=[true, true, true, true, true, true]
- PASS `stable_r2_byte_identity` — unique_rendered_prefix_hashes=1
- PASS `adaptive_r2_local_cache_pattern` — expected_hits=[false, false, false, false, false, false] actual_hits=[false, false, false, false, false, false] expected_eligible=false actual_eligible=[false, false, false, false, false, false]
- PASS `adaptive_r2_transition_identity` — compaction without render-relevant drift stays fixed; task A→B and same-task snapshot drift change bytes; a fresh worker reproduces the latest snapshot bytes
- PASS `passthrough_r3_local_cache_pattern` — expected_hits=[false, false, false, false, false, false] actual_hits=[false, false, false, false, false, false] expected_eligible=false actual_eligible=[false, false, false, false, false, false]
- PASS `passthrough_r3_exact_source_bytes` — source and rendered hashes must match for all six transitions
- PASS `stable_r3_local_cache_pattern` — expected_hits=[false, true, true, true, true, false] actual_hits=[false, true, true, true, true, false] expected_eligible=true actual_eligible=[true, true, true, true, true, true]
- PASS `stable_r3_byte_identity` — unique_rendered_prefix_hashes=1
- PASS `adaptive_r3_local_cache_pattern` — expected_hits=[false, false, false, false, false, false] actual_hits=[false, false, false, false, false, false] expected_eligible=false actual_eligible=[false, false, false, false, false, false]
- PASS `adaptive_r3_transition_identity` — compaction without render-relevant drift stays fixed; task A→B and same-task snapshot drift change bytes; a fresh worker reproduces the latest snapshot bytes
- PASS `stable_prefix_is_byte_identical_across_all_repetitions` — unique_rendered_prefix_hashes=1

## Explicitly unknown provider metrics

- `adaptive.churn_rate`: provider cache creation/read token observations are incomplete
- `adaptive.reuse_multiple`: provider cache creation/read token observations are incomplete
- `adaptive.effective_cache_cost_usd`: provider cache creation/read cost observations are incomplete
- `adaptive.compaction_rewarm_tokens`: compaction rewarm tokens are incomplete
- `passthrough.churn_rate`: provider cache creation/read token observations are incomplete
- `passthrough.reuse_multiple`: provider cache creation/read token observations are incomplete
- `passthrough.effective_cache_cost_usd`: provider cache creation/read cost observations are incomplete
- `passthrough.compaction_rewarm_tokens`: compaction rewarm tokens are incomplete
- `stable.churn_rate`: provider cache creation/read token observations are incomplete
- `stable.reuse_multiple`: provider cache creation/read token observations are incomplete
- `stable.effective_cache_cost_usd`: provider cache creation/read cost observations are incomplete
- `stable.compaction_rewarm_tokens`: compaction rewarm tokens are incomplete
