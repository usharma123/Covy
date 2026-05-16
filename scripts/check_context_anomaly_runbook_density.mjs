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
const unknownArgs = args.filter(
  (arg) => !["--json", "--self-test", "--help"].includes(arg),
);
if (unknownArgs.length > 0) {
  console.error("context_anomaly_runbook_density_unknown_option");
  console.error(`option=${unknownArgs[0]}`);
  process.exit(2);
}
if (args.includes("--help")) {
  console.log(
    [
      "Usage: node scripts/check_context_anomaly_runbook_density.mjs [--json|--self-test|--help]",
      "default: validate runbook line budget and required command entries",
      "--json: print ok, line_count, max_lines, and commands_checked",
      "--self-test: verify line-budget and missing-command failure modes",
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
  "node scripts/check_context_anomaly_runbook_density.mjs --self-test",
  "Packet28 digest --root . --json",
];

function evaluate(runbook, lineBudget) {
  const lineCount = runbook.endsWith("\n")
    ? runbook.split("\n").length - 1
    : runbook.split("\n").length;
  const missingCommands = requiredCommands.filter(
    (command) => !runbook.includes(command),
  );
  if (lineCount > lineBudget) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_too_many_lines",
      line_count: lineCount,
      max_lines: lineBudget,
    };
  }
  if (missingCommands.length > 0) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_missing_commands",
      missing: missingCommands,
    };
  }
  return {
    ok: true,
    line_count: lineCount,
    max_lines: lineBudget,
    commands_checked: requiredCommands.length,
  };
}

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

function assertSelfTest(result, expectedCode) {
  if (result.code !== expectedCode) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected=${expectedCode}`);
    console.error(`actual=${result.code ?? "ok"}`);
    process.exit(1);
  }
}

const runbook = readFileSync(runbookPath, "utf8");
const result = evaluate(runbook, maxLines);
if (args.includes("--self-test")) {
  if (!result.ok) {
    console.error("context_anomaly_runbook_density_self_test_baseline_failed");
    console.error(`code=${result.code}`);
    process.exit(1);
  }
  assertSelfTest(
    evaluate(runbook, result.line_count - 1),
    "context_anomaly_runbook_density_too_many_lines",
  );
  assertSelfTest(
    evaluate(runbook.replace(requiredCommands[0], ""), maxLines),
    "context_anomaly_runbook_density_missing_commands",
  );
  assertSelfTest(
    evaluate(
      runbook.replace(
        "node scripts/check_context_anomaly_runbook_density.mjs --self-test",
        "",
      ),
      maxLines,
    ),
    "context_anomaly_runbook_density_missing_commands",
  );
  console.log("context_anomaly_runbook_density_self_test_ok");
  process.exit(0);
}

if (!result.ok) {
  const { code, ok, ...details } = result;
  fail(code, details);
}

const payload = {
  ok: true,
  line_count: result.line_count,
  max_lines: result.max_lines,
  commands_checked: result.commands_checked,
};
if (args.includes("--json")) {
  console.log(JSON.stringify(payload));
} else {
  console.log(
    `context_anomaly_runbook_density_ok lines=${payload.line_count}/${payload.max_lines} commands=${payload.commands_checked}`,
  );
}
