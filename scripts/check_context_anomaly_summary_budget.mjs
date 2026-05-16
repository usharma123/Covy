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
const maxJsonBytes = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_SUMMARY_JSON_MAX ?? "768",
  10,
);
const args = process.argv.slice(2);
const unknownArgs = args.filter(
  (arg) => !["--json", "--self-test", "--help"].includes(arg),
);
if (unknownArgs.length > 0) {
  console.error("context_anomaly_summary_budget_unknown_option");
  console.error(`option=${unknownArgs[0]}`);
  process.exit(2);
}
if (args.includes("--help")) {
  console.log(
    [
      "Usage: node scripts/check_context_anomaly_summary_budget.mjs [--json|--self-test|--help]",
      "default: validate workflow summary line count and template width",
      "--json: print ok, budgets, labels, and max_json_bytes under a byte cap",
      "--self-test: verify line, width, and missing-label failure modes",
      "--help: print this help; bad flags fail with context_anomaly_summary_budget_unknown_option",
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

function evaluate(workflowText, lineBudget, widthBudget) {
  const summaryLines = extractSummaryLines(workflowText);
  const labels = summaryLines.map(lineLabel);
  const missingLabels = requiredLabels.filter(
    (required) => !labels.some((label) => label.startsWith(required)),
  );
  const maxActualTemplateLineLength = Math.max(
    0,
    ...summaryLines.map((line) => line.length),
  );
  if (summaryLines.length > lineBudget) {
    return {
      ok: false,
      code: "context_anomaly_summary_budget_too_many_lines",
      line_count: summaryLines.length,
      max_lines: lineBudget,
    };
  }
  if (maxActualTemplateLineLength > widthBudget) {
    return {
      ok: false,
      code: "context_anomaly_summary_budget_line_too_long",
      max_template_line: maxActualTemplateLineLength,
      max_template_line_allowed: widthBudget,
    };
  }
  if (missingLabels.length > 0) {
    return {
      ok: false,
      code: "context_anomaly_summary_budget_missing_labels",
      missing: missingLabels,
    };
  }
  return {
    ok: true,
    line_count: summaryLines.length,
    max_lines: lineBudget,
    max_template_line: maxActualTemplateLineLength,
    max_template_line_allowed: widthBudget,
    labels,
  };
}

const workflowText = readFileSync(workflowPath, "utf8");

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
    console.error("context_anomaly_summary_budget_self_test_failed");
    console.error(`expected=${expectedCode}`);
    console.error(`actual=${result.code ?? "ok"}`);
    process.exit(1);
  }
}

const result = evaluate(workflowText, maxLines, maxTemplateLineLength);
if (args.includes("--self-test")) {
  if (!result.ok) {
    console.error("context_anomaly_summary_budget_self_test_baseline_failed");
    console.error(`code=${result.code}`);
    process.exit(1);
  }
  assertSelfTest(
    evaluate(workflowText, result.line_count - 1, maxTemplateLineLength),
    "context_anomaly_summary_budget_too_many_lines",
  );
  assertSelfTest(
    evaluate(workflowText, maxLines, result.max_template_line - 1),
    "context_anomaly_summary_budget_line_too_long",
  );
  assertSelfTest(
    evaluate(
      workflowText.replace(
        /\n\s+echo "- audit checksum: \$audit_output_checksum"/,
        "",
      ),
      maxLines,
      maxTemplateLineLength,
    ),
    "context_anomaly_summary_budget_missing_labels",
  );
  console.log("context_anomaly_summary_budget_self_test_ok");
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
  max_template_line: result.max_template_line,
  max_template_line_allowed: result.max_template_line_allowed,
  max_json_bytes: maxJsonBytes,
  labels: result.labels,
};
if (args.includes("--json")) {
  const json = JSON.stringify(payload);
  if (json.length > maxJsonBytes) {
    fail("context_anomaly_summary_budget_json_too_long", {
      actual_bytes: json.length,
      max_json_bytes: maxJsonBytes,
    });
  }
  console.log(json);
} else {
  console.log(
    `context_anomaly_summary_budget_ok lines=${payload.line_count}/${payload.max_lines} max_template_line=${payload.max_template_line}/${payload.max_template_line_allowed}`,
  );
}
