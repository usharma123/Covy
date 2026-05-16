#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const runbookPath = join(repoRoot, "docs/context-anomalies/RUNBOOK.md");
const maxLines = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES ?? "44",
  10,
);
const args = process.argv.slice(2);
const unknownArgs = args.filter((arg) => !["--json", "--help"].includes(arg));
if (unknownArgs.length > 0) {
  console.error("context_anomaly_runbook_density_unknown_option");
  console.error(`option=${unknownArgs[0]}`);
  process.exit(2);
}
if (args.includes("--help")) {
  console.log(
    [
      "Usage: node scripts/check_context_anomaly_runbook_density.mjs [--json|--help]",
      "default: validate runbook line budget and required command entries",
      "--json: print ok, line_count, max_lines, and commands_checked",
      "--help: print this help",
    ].join("\n"),
  );
  process.exit(0);
}

const requiredCommands = [
  "Packet28 verify context-anomalies --root . --json",
  "Packet28 dashboard --root . --context-anomaly-history docs/context-anomalies/history.jsonl --json",
  "node scripts/check_context_anomaly_hidden_samples.mjs",
  "node scripts/check_context_anomaly_hidden_samples.mjs --json",
  "node scripts/check_context_anomaly_hidden_samples.mjs --self-test",
  "node scripts/check_context_anomaly_hidden_samples.mjs --help",
  "node scripts/audit_context_anomaly_hidden_samples.mjs",
  "node scripts/audit_context_anomaly_hidden_samples.mjs --strict",
  "node scripts/audit_context_anomaly_hidden_samples.mjs --help",
  "node scripts/check_context_anomaly_summary_budget.mjs --self-test",
  "node scripts/check_context_anomaly_summary_budget.mjs --json",
  "Packet28 digest --root . --json",
];

const runbook = readFileSync(runbookPath, "utf8");
const lineCount = runbook.endsWith("\n")
  ? runbook.split("\n").length - 1
  : runbook.split("\n").length;
const missingCommands = requiredCommands.filter(
  (command) => !runbook.includes(command),
);

function fail(code, details) {
  if (args.includes("--json")) {
    console.log(JSON.stringify({ ok: false, code, ...details }));
  } else {
    console.error(code);
    for (const [key, value] of Object.entries(details)) {
      console.error(`${key}=${Array.isArray(value) ? value.join(",") : value}`);
    }
  }
  process.exit(1);
}

if (lineCount > maxLines) {
  fail("context_anomaly_runbook_density_too_many_lines", {
    line_count: lineCount,
    max_lines: maxLines,
  });
}
if (missingCommands.length > 0) {
  fail("context_anomaly_runbook_density_missing_commands", {
    missing: missingCommands,
  });
}

const payload = {
  ok: true,
  line_count: lineCount,
  max_lines: maxLines,
  commands_checked: requiredCommands.length,
};
if (args.includes("--json")) {
  console.log(JSON.stringify(payload));
} else {
  console.log(
    `context_anomaly_runbook_density_ok lines=${payload.line_count}/${payload.max_lines} commands=${payload.commands_checked}`,
  );
}
