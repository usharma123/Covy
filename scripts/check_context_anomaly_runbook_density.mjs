#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const repoRoot = join(dirname(scriptPath), "..");
const runbookPath = join(repoRoot, "docs/context-anomalies/RUNBOOK.md");
const maxLines = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES ?? "44",
  10,
);
const maxJsonBytes = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_JSON_MAX ?? "256",
  10,
);
const maxTableRowLength = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_ROW_MAX ?? "520",
  10,
);
const helpLines = [
  "Usage: node scripts/check_context_anomaly_runbook_density.mjs [--json|--self-test|--help]",
  "default: validate runbook line budget, row width, and required command entries",
  "--json: print ok, budgets, max_table_row, commands_checked, and max_json_bytes",
  "--self-test: verify line, width, missing-command, and JSON byte failure modes",
  "--help: print this help; bad flags fail with context_anomaly_runbook_density_unknown_option",
];
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
  console.log(helpLines.join("\n"));
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
  "node scripts/check_context_anomaly_runbook_density.mjs --json",
  "Packet28 digest --root . --json",
];

function evaluate(runbook, lineBudget) {
  const lineCount = runbook.endsWith("\n")
    ? runbook.split("\n").length - 1
    : runbook.split("\n").length;
  const tableRows = runbook
    .split("\n")
    .filter((line) => line.startsWith("|"));
  const maxActualTableRowLength = Math.max(
    0,
    ...tableRows.map((line) => line.length),
  );
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
  if (maxActualTableRowLength > maxTableRowLength) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_row_too_wide",
      max_table_row: maxActualTableRowLength,
      max_table_row_allowed: maxTableRowLength,
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
    max_table_row: maxActualTableRowLength,
    max_table_row_allowed: maxTableRowLength,
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

function assertEnvFailure(env, commandArgs, expectedCode) {
  try {
    execFileSync(process.execPath, [scriptPath, ...commandArgs], {
      encoding: "utf8",
      env: { ...process.env, ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (error) {
    const output = `${error.stdout ?? ""}${error.stderr ?? ""}`;
    if (output.includes(expectedCode)) {
      return;
    }
    console.error("context_anomaly_runbook_density_self_test_env_failed");
    console.error(`expected=${expectedCode}`);
    console.error(`actual=${output.trim()}`);
    process.exit(1);
  }
  console.error("context_anomaly_runbook_density_self_test_env_failed");
  console.error(`expected=${expectedCode}`);
  console.error("actual=ok");
  process.exit(1);
}

function assertHelpIncludes(expected) {
  const help = helpLines.join("\n");
  if (!help.includes(expected)) {
    console.error("context_anomaly_runbook_density_self_test_help_failed");
    console.error(`missing=${expected}`);
    process.exit(1);
  }
}

function successPayload(result, jsonBudget) {
  return {
    ok: true,
    line_count: result.line_count,
    max_lines: result.max_lines,
    max_table_row: result.max_table_row,
    max_table_row_allowed: result.max_table_row_allowed,
    commands_checked: result.commands_checked,
    max_json_bytes: jsonBudget,
  };
}

function jsonBudgetIssue(payload, jsonBudget) {
  const json = JSON.stringify(payload);
  if (json.length <= jsonBudget) {
    return null;
  }
  return {
    ok: false,
    code: "context_anomaly_runbook_density_json_too_long",
    actual_bytes: json.length,
    max_json_bytes: jsonBudget,
  };
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
  assertSelfTest(
    evaluate(
      runbook.replace(
        "node scripts/check_context_anomaly_runbook_density.mjs --json",
        "",
      ),
      maxLines,
    ),
    "context_anomaly_runbook_density_missing_commands",
  );
  assertEnvFailure(
    { P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES: "10" },
    [],
    "context_anomaly_runbook_density_too_many_lines",
  );
  assertEnvFailure(
    { P28_CONTEXT_ANOMALY_RUNBOOK_ROW_MAX: "10" },
    [],
    "context_anomaly_runbook_density_row_too_wide",
  );
  assertEnvFailure(
    { P28_CONTEXT_ANOMALY_RUNBOOK_JSON_MAX: "10" },
    ["--json"],
    "context_anomaly_runbook_density_json_too_long",
  );
  assertSelfTest(
    jsonBudgetIssue(successPayload(result, 10), 10) ?? { code: "ok" },
    "context_anomaly_runbook_density_json_too_long",
  );
  for (const expected of [
    "default",
    "--json",
    "--self-test",
    "--help",
    "context_anomaly_runbook_density_unknown_option",
  ]) {
    assertHelpIncludes(expected);
  }
  console.log("context_anomaly_runbook_density_self_test_ok");
  process.exit(0);
}

if (!result.ok) {
  const { code, ok, ...details } = result;
  fail(code, details);
}

const payload = successPayload(result, maxJsonBytes);
if (args.includes("--json")) {
  const issue = jsonBudgetIssue(payload, maxJsonBytes);
  if (issue) {
    const { code, ok, ...details } = issue;
    fail(code, details);
  }
  console.log(JSON.stringify(payload));
} else {
  console.log(
    `context_anomaly_runbook_density_ok lines=${payload.line_count}/${payload.max_lines} max_table_row=${payload.max_table_row}/${payload.max_table_row_allowed} commands=${payload.commands_checked}`,
  );
}
