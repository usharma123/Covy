#!/usr/bin/env node

const args = new Set(process.argv.slice(2));
const unknownArgs = [...args].filter((arg) => !["--json", "--help"].includes(arg));
if (unknownArgs.length > 0) {
  console.error("prompt_cache_pointer_savings_unknown_option");
  console.error(`option=${unknownArgs[0]}`);
  process.exit(2);
}
if (args.has("--help")) {
  console.log(
    [
      "Usage: node docs/experiments/prompt-cache-context/pointer_savings.mjs [--json|--help]",
      "Simulates Packet28 handoff replay cost with full embedded context, pointer context, and critical-section compression.",
      "--json: print machine-readable scenario metrics",
      "--help: print this help",
    ].join("\n"),
  );
  process.exit(0);
}

const taskId = "task-prompt-cache-context";
const contextVersion = "ctx-prompt-cache-context";
const nextPrompt =
  "Continue the handoff, inspect the latest focused evidence, implement the smallest safe improvement, and run targeted verification.";
const minimumPointerSavingsPct = 90;
const minimumCompressionSavingsPct = 55;

function estimateTextTokens(value) {
  return Math.ceil(Buffer.byteLength(value, "utf8") / 4);
}

function estimateJsonTokens(value) {
  return estimateTextTokens(JSON.stringify(value));
}

function repeatedEvidence(label, repeats) {
  return Array.from(
    { length: repeats },
    (_, index) =>
      `${label} ${index + 1}: searched prompt-pressure, MCP prompt resources, cache indexes, and handoff tests; no user-facing decision changed.`,
  ).join("\n");
}

function buildHandoff(evidenceBlocks) {
  const sections = [
    {
      id: "objective",
      title: "Objective",
      body: "Measure whether Packet28-level prompt pointers and cached handoff artifacts reduce repeated context replay.",
    },
    {
      id: "next_action",
      title: "Next Action",
      body: "Run the pointer savings experiment, keep any code change focused, and report exact result numbers.",
    },
    {
      id: "context_debt",
      title: "Context Debt",
      body: "stale_paths=0 open_questions=0 unverified_edits=1 contradictions=0",
    },
    {
      id: "evidence_freshness",
      title: "Evidence Freshness",
      body: "fresh_reads=cmd_mcp_prompt_resource.rs,cmd_mcp_native_handoff.rs,context-memory-core",
    },
  ];
  for (let index = 0; index < evidenceBlocks; index += 1) {
    sections.push({
      id: `search_evidence_${index + 1}`,
      title: `Search Evidence ${index + 1}`,
      body: repeatedEvidence(`redundant-result-${index + 1}`, 36),
    });
  }
  return {
    context_version: contextVersion,
    artifact_id: contextVersion,
    task_id: taskId,
    brief:
      "## Task Objective\nMeasure prompt-cache context savings.\n\n## Next Action\nRun the deterministic experiment.",
    sections,
    evidence_artifact_ids: ["artifact-search", "artifact-test"],
    changed_paths_since_checkpoint: [
      "crates/suite-cli/src/cmd_mcp_prompt_resource.rs",
      "crates/suite-cli/src/cmd_mcp_native_handoff.rs",
    ],
    next_action_summary: "run prompt-cache savings experiment",
  };
}

function criticalSlice(payload) {
  return {
    ...payload,
    sections: payload.sections.filter((section) =>
      [
        "objective",
        "next_action",
        "context_debt",
        "evidence_freshness",
      ].includes(section.id),
    ),
  };
}

function pct(saved, total) {
  if (total === 0) {
    return 0;
  }
  return Math.round((saved / total) * 1000) / 10;
}

function continueTaskPointerPrompt() {
  return [
    `Continue Packet28 task \`${taskId}\`.`,
    "",
    `Status: version=${contextVersion}, handoff_ready=true, push=false`,
    "",
    `Read \`packet28://task/${taskId}/brief\` for full context. Let hooks handle reducer capture. Use \`packet28.write_intention\` for objective changes.`,
  ].join("\n");
}

function scenario(evidenceBlocks) {
  const payload = buildHandoff(evidenceBlocks);
  const compressed = criticalSlice(payload);
  const nextPromptTokens = estimateTextTokens(nextPrompt);
  const fullContextTokens = estimateJsonTokens(payload);
  const compressedContextTokens = estimateJsonTokens(compressed);
  const pointerContextTokens = estimateTextTokens(continueTaskPointerPrompt());
  const fullReplayTokens = fullContextTokens + nextPromptTokens;
  const compressedReplayTokens = compressedContextTokens + nextPromptTokens;
  const pointerReplayTokens = pointerContextTokens + nextPromptTokens;
  const pointerSavingsTokens = fullReplayTokens - pointerReplayTokens;
  const compressionSavingsTokens = fullReplayTokens - compressedReplayTokens;
  return {
    evidence_blocks: evidenceBlocks,
    full_replay_tokens: fullReplayTokens,
    pointer_replay_tokens: pointerReplayTokens,
    pointer_savings_tokens: pointerSavingsTokens,
    pointer_savings_pct: pct(pointerSavingsTokens, fullReplayTokens),
    compressed_replay_tokens: compressedReplayTokens,
    compression_savings_tokens: compressionSavingsTokens,
    compression_savings_pct: pct(compressionSavingsTokens, fullReplayTokens),
  };
}

const scenarios = [1, 2, 4].map(scenario);
const minPointerSavings = Math.min(...scenarios.map((item) => item.pointer_savings_pct));
const minCompressionSavings = Math.min(
  ...scenarios.map((item) => item.compression_savings_pct),
);
const ok =
  minPointerSavings >= minimumPointerSavingsPct &&
  minCompressionSavings >= minimumCompressionSavingsPct;
const payload = {
  ok,
  hypothesis:
    "Replacing embedded handoff context with Packet28 task brief pointers keeps replay anchors available while avoiding repeated context injection.",
  token_estimator: "ceil(utf8_bytes / 4), matching Packet28 prompt_pressure heuristic",
  thresholds: {
    minimum_pointer_savings_pct: minimumPointerSavingsPct,
    minimum_compression_savings_pct: minimumCompressionSavingsPct,
  },
  scenarios,
};

if (args.has("--json")) {
  console.log(JSON.stringify(payload, null, 2));
} else {
  console.log(`ok=${ok}`);
  console.log(`token_estimator="${payload.token_estimator}"`);
  console.log(
    "evidence_blocks full_replay pointer_replay pointer_saved pointer_pct compressed_replay compressed_saved compressed_pct",
  );
  for (const item of scenarios) {
    console.log(
      [
        item.evidence_blocks,
        item.full_replay_tokens,
        item.pointer_replay_tokens,
        item.pointer_savings_tokens,
        item.pointer_savings_pct,
        item.compressed_replay_tokens,
        item.compression_savings_tokens,
        item.compression_savings_pct,
      ].join(" "),
    );
  }
}

if (!ok) {
  process.exit(1);
}
