#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const workflowPath = join(repoRoot, ".github/workflows/context-anomalies.yml");
const maxLines = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_SUMMARY_MAX_LINES ?? "24",
  10,
);
const maxTemplateLineLength = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_SUMMARY_MAX_LINE ?? "180",
  10,
);
const args = process.argv.slice(2);
const unknownArgs = args.filter((arg) => !["--json", "--help"].includes(arg));
if (unknownArgs.length > 0) {
  console.error("context_anomaly_summary_budget_unknown_option");
  console.error(`option=${unknownArgs[0]}`);
  process.exit(2);
}
if (args.includes("--help")) {
  console.log(
    [
      "Usage: node scripts/check_context_anomaly_summary_budget.mjs [--json|--help]",
      "default: validate workflow summary line count and template width",
      "--json: print ok, line_count, max_lines, max_template_line, and labels",
      "--help: print this help",
    ].join("\n"),
  );
  process.exit(0);
}

const requiredLabels = [
  "### Context anomalies",
  "- anomalies:",
  "- high:",
  "- hidden:",
  "- hidden samples:",
  "- trend status:",
  "- recurring hidden:",
  "- trend latest age ms:",
  "- trend oldest recurring hidden age ms:",
  "- trend repair hint:",
  "### Context anomaly fixture trend",
  "- fixture status:",
  "- fixture recurring hidden:",
  "- fixture hidden samples:",
  "- fixture oldest recurring hidden age ms:",
  "- audit mode:",
  "- audit checksum:",
  "- audit json checksum:",
  "- formatter smoke:",
  "- formatter budget:",
  "- formatter checksum:",
  "- command table:",
];

function extractSummaryLines(workflowText) {
  const start = workflowText.indexOf('echo "### Context anomalies"');
  if (start < 0) {
    throw new Error("context_anomaly_summary_budget_missing_start");
  }
  const end = workflowText.indexOf('} >> "$GITHUB_STEP_SUMMARY"', start);
  if (end < 0) {
    throw new Error("context_anomaly_summary_budget_missing_end");
  }
  return workflowText
    .slice(start, end)
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.startsWith('echo "'))
    .map((line) => line.slice('echo "'.length, line.endsWith('"') ? -1 : undefined));
}

function lineLabel(line) {
  const commandIndex = line.indexOf("$(");
  const variableIndex = line.indexOf("$", commandIndex < 0 ? 0 : commandIndex + 2);
  const splitIndexes = [commandIndex, variableIndex].filter((index) => index >= 0);
  const splitIndex = splitIndexes.length > 0 ? Math.min(...splitIndexes) : -1;
  return (splitIndex >= 0 ? line.slice(0, splitIndex) : line).trimEnd();
}

const summaryLines = extractSummaryLines(readFileSync(workflowPath, "utf8"));
const labels = summaryLines.map(lineLabel);
const missingLabels = requiredLabels.filter(
  (required) => !labels.some((label) => label.startsWith(required)),
);
const maxActualTemplateLineLength = Math.max(
  0,
  ...summaryLines.map((line) => line.length),
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

if (summaryLines.length > maxLines) {
  fail("context_anomaly_summary_budget_too_many_lines", {
    line_count: summaryLines.length,
    max_lines: maxLines,
  });
}
if (maxActualTemplateLineLength > maxTemplateLineLength) {
  fail("context_anomaly_summary_budget_line_too_long", {
    max_template_line: maxActualTemplateLineLength,
    max_template_line_allowed: maxTemplateLineLength,
  });
}
if (missingLabels.length > 0) {
  fail("context_anomaly_summary_budget_missing_labels", {
    missing: missingLabels,
  });
}

const payload = {
  ok: true,
  line_count: summaryLines.length,
  max_lines: maxLines,
  max_template_line: maxActualTemplateLineLength,
  max_template_line_allowed: maxTemplateLineLength,
  labels,
};
if (args.includes("--json")) {
  console.log(JSON.stringify(payload));
} else {
  console.log(
    `context_anomaly_summary_budget_ok lines=${payload.line_count}/${payload.max_lines} max_template_line=${payload.max_template_line}/${payload.max_template_line_allowed}`,
  );
}
