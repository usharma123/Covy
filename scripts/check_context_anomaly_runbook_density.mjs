#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const repoRoot = join(dirname(scriptPath), "..");
const runbookPath = join(repoRoot, "docs/context-anomalies/RUNBOOK.md");
const workflowPath = join(repoRoot, ".github/workflows/context-anomalies.yml");
const maxLines = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES ?? "44",
  10,
);
const maxJsonBytes = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_JSON_MAX ?? "288",
  10,
);
const minJsonHeadroomBytes = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_JSON_HEADROOM_MIN ?? "16",
  10,
);
const maxTableRowLength = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_ROW_MAX ?? "520",
  10,
);
const maxDensityProseLineLength = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_PROSE_MAX ?? "420",
  10,
);
const helpLines = [
  "Usage: node scripts/check_context_anomaly_runbook_density.mjs [--json|--self-test|--help]",
  "default: validate runbook line budget, row width, density prose width, docs, and workflow density commands",
  "--json: print ok, budgets, width metrics, command counts, and max_json_bytes",
  "--self-test: verify line, width, missing-command, and JSON byte/headroom failure modes",
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
const requiredWorkflowDensityCommands = [
  "node scripts/check_context_anomaly_runbook_density.mjs --help",
  "node scripts/check_context_anomaly_runbook_density.mjs",
  "node scripts/check_context_anomaly_runbook_density.mjs --self-test",
];
const requiredFailureCodes = [
  "context_anomaly_runbook_density_too_many_lines",
  "context_anomaly_runbook_density_row_too_wide",
  "context_anomaly_runbook_density_missing_commands",
  "context_anomaly_runbook_density_workflow_missing_commands",
  "context_anomaly_runbook_density_missing_failure_docs",
  "context_anomaly_runbook_density_missing_output_docs",
  "context_anomaly_runbook_density_missing_env_docs",
  "context_anomaly_runbook_density_prose_too_wide",
  "context_anomaly_runbook_density_json_too_long",
];
const requiredOutputLabels = [
  "failure_codes",
  "workflow_commands",
  "prose",
  "json_headroom",
  "env_docs",
  "output_labels",
  "output_doc_phrases",
];
const requiredOutputDocPhrases = ["key=value"];
const requiredEnvDocs = [
  "P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES",
  "P28_CONTEXT_ANOMALY_RUNBOOK_ROW_MAX",
  "P28_CONTEXT_ANOMALY_RUNBOOK_PROSE_MAX",
  "P28_CONTEXT_ANOMALY_RUNBOOK_JSON_MAX",
  "P28_CONTEXT_ANOMALY_RUNBOOK_JSON_HEADROOM_MIN",
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
  const densityProseLines = runbook
    .split("\n")
    .filter(
      (line) =>
        !line.startsWith("|") &&
        line.includes("context_anomaly_runbook_density_"),
    );
  const maxActualDensityProseLineLength = Math.max(
    0,
    ...densityProseLines.map((line) => line.length),
  );
  const missingCommands = requiredCommands.filter(
    (command) => !runbook.includes(command),
  );
  const missingFailureCodes = requiredFailureCodes.filter(
    (code) => !runbook.includes(code),
  );
  const missingOutputLabels = requiredOutputLabels.filter(
    (label) => !runbook.includes(`\`${label}\``),
  );
  const missingOutputDocPhrases = requiredOutputDocPhrases.filter(
    (phrase) => !runbook.includes(`\`${phrase}\``),
  );
  const missingEnvDocs = requiredEnvDocs.filter(
    (envName) => !runbook.includes(`\`${envName}\``),
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
  if (maxActualDensityProseLineLength > maxDensityProseLineLength) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_prose_too_wide",
      max_density_prose_line: maxActualDensityProseLineLength,
      max_density_prose_line_allowed: maxDensityProseLineLength,
    };
  }
  if (missingCommands.length > 0) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_missing_commands",
      missing: missingCommands,
    };
  }
  if (missingFailureCodes.length > 0) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_missing_failure_docs",
      missing: missingFailureCodes,
    };
  }
  if (missingOutputLabels.length > 0 || missingOutputDocPhrases.length > 0) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_missing_output_docs",
      missing: [...missingOutputLabels, ...missingOutputDocPhrases],
    };
  }
  if (missingEnvDocs.length > 0) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_missing_env_docs",
      missing: missingEnvDocs,
    };
  }
  return {
    ok: true,
    line_count: lineCount,
    max_lines: lineBudget,
    max_table_row: maxActualTableRowLength,
    max_table_row_allowed: maxTableRowLength,
    max_density_prose_line: maxActualDensityProseLineLength,
    max_density_prose_line_allowed: maxDensityProseLineLength,
    commands_checked: requiredCommands.length,
    failure_codes_checked: requiredFailureCodes.length,
    env_docs_checked: requiredEnvDocs.length,
    output_labels_checked: requiredOutputLabels.length,
    output_doc_phrases_checked: requiredOutputDocPhrases.length,
  };
}

function evaluateWorkflow(workflow) {
  const workflowLines = workflow
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  const missingCommands = requiredWorkflowDensityCommands.filter(
    (command) => !workflowLines.includes(command),
  );
  if (missingCommands.length > 0) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_workflow_missing_commands",
      missing: missingCommands,
    };
  }
  return {
    ok: true,
    workflow_commands_checked: requiredWorkflowDensityCommands.length,
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

function successPayload(result, workflow, jsonBudget) {
  return {
    ok: true,
    line_count: result.line_count,
    max_lines: result.max_lines,
    max_table_row: result.max_table_row,
    max_table_row_allowed: result.max_table_row_allowed,
    max_density_prose_line: result.max_density_prose_line,
    max_density_prose_line_allowed: result.max_density_prose_line_allowed,
    commands_checked: result.commands_checked,
    failure_codes_checked: result.failure_codes_checked,
    workflow_commands_checked: workflow.workflow_commands_checked,
    max_json_bytes: jsonBudget,
  };
}

function jsonBudgetIssue(payload, jsonBudget) {
  const json = JSON.stringify(payload);
  if (json.length + minJsonHeadroomBytes <= jsonBudget) {
    return null;
  }
  return {
    ok: false,
    code: "context_anomaly_runbook_density_json_too_long",
    actual_bytes: json.length,
    max_json_bytes: jsonBudget,
    min_json_headroom_bytes: minJsonHeadroomBytes,
  };
}

function jsonHeadroomBytes(payload, jsonBudget) {
  return jsonBudget - JSON.stringify(payload).length;
}

function renderDefaultOutput(payload, resultDetails, jsonHeadroom) {
  return [
    `context_anomaly_runbook_density_ok lines=${payload.line_count}/${payload.max_lines}`,
    `max_table_row=${payload.max_table_row}/${payload.max_table_row_allowed}`,
    `commands=${payload.commands_checked}`,
    `failure_codes=${payload.failure_codes_checked}`,
    `env_docs=${resultDetails.env_docs_checked}`,
    `output_labels=${resultDetails.output_labels_checked}`,
    `output_doc_phrases=${resultDetails.output_doc_phrases_checked}`,
    `workflow_commands=${payload.workflow_commands_checked}`,
    `prose=${payload.max_density_prose_line}/${payload.max_density_prose_line_allowed}`,
    `json_headroom=${jsonHeadroom}`,
  ].join(" ");
}

function parseDefaultOutput(line) {
  const parts = line.trim().split(/\s+/);
  if (parts[0] !== "context_anomaly_runbook_density_ok") {
    return null;
  }
  return Object.fromEntries(parts.slice(1).map((part) => part.split("=")));
}

function defaultOutputIssue(parsed, resultDetails) {
  const expectedTextFields = [
    "lines",
    "max_table_row",
    "commands",
    "failure_codes",
    "env_docs",
    "output_labels",
    "output_doc_phrases",
    "workflow_commands",
    "prose",
    "json_headroom",
  ];
  for (const field of expectedTextFields) {
    if (!parsed?.[field]) {
      return `missing_default_output_field=${field}`;
    }
  }
  if (
    parsed.commands !== String(resultDetails.commands_checked) ||
    parsed.output_labels !== String(resultDetails.output_labels_checked)
  ) {
    return "default_output_parse_mismatch";
  }
  return null;
}

const runbook = readFileSync(runbookPath, "utf8");
const workflow = readFileSync(workflowPath, "utf8");
const result = evaluate(runbook, maxLines);
const workflowResult = evaluateWorkflow(workflow);
if (args.includes("--self-test")) {
  if (!result.ok) {
    console.error("context_anomaly_runbook_density_self_test_baseline_failed");
    console.error(`code=${result.code}`);
    process.exit(1);
  }
  if (!workflowResult.ok) {
    console.error("context_anomaly_runbook_density_self_test_baseline_failed");
    console.error(`code=${workflowResult.code}`);
    process.exit(1);
  }
  if (result.failure_codes_checked !== requiredFailureCodes.length) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_failure_codes=${requiredFailureCodes.length}`);
    console.error(`actual_failure_codes=${result.failure_codes_checked}`);
    process.exit(1);
  }
  if (result.env_docs_checked !== requiredEnvDocs.length) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_env_docs=${requiredEnvDocs.length}`);
    console.error(`actual_env_docs=${result.env_docs_checked}`);
    process.exit(1);
  }
  if (result.output_labels_checked !== requiredOutputLabels.length) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_output_labels=${requiredOutputLabels.length}`);
    console.error(`actual_output_labels=${result.output_labels_checked}`);
    process.exit(1);
  }
  if (result.output_doc_phrases_checked !== requiredOutputDocPhrases.length) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_output_doc_phrases=${requiredOutputDocPhrases.length}`,
    );
    console.error(
      `actual_output_doc_phrases=${result.output_doc_phrases_checked}`,
    );
    process.exit(1);
  }
  if (
    workflowResult.workflow_commands_checked !==
    requiredWorkflowDensityCommands.length
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_workflow_commands=${requiredWorkflowDensityCommands.length}`,
    );
    console.error(
      `actual_workflow_commands=${workflowResult.workflow_commands_checked}`,
    );
    process.exit(1);
  }
  const baselinePayload = successPayload(result, workflowResult, maxJsonBytes);
  const baselineHeadroom = jsonHeadroomBytes(baselinePayload, maxJsonBytes);
  if (baselineHeadroom < minJsonHeadroomBytes) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_headroom_at_least=${minJsonHeadroomBytes}`);
    console.error(`actual_headroom=${baselineHeadroom}`);
    process.exit(1);
  }
  if (
    jsonHeadroomBytes({ ...baselinePayload, extra: "x" }, maxJsonBytes) >=
    baselineHeadroom
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("headroom_did_not_change_after_payload_growth");
    process.exit(1);
  }
  const parsedDefaultOutput = parseDefaultOutput(
    renderDefaultOutput(baselinePayload, result, baselineHeadroom),
  );
  const defaultOutputError = defaultOutputIssue(parsedDefaultOutput, result);
  if (defaultOutputError) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(defaultOutputError);
    process.exit(1);
  }
  if (parseDefaultOutput("bad_prefix lines=44/44") !== null) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("malformed_default_output_prefix_accepted");
    process.exit(1);
  }
  const missingLabelError = defaultOutputIssue(
    parseDefaultOutput(
      renderDefaultOutput(baselinePayload, result, baselineHeadroom).replace(
        / output_labels=\d+/,
        "",
      ),
    ),
    result,
  );
  if (missingLabelError !== "missing_default_output_field=output_labels") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected=missing_default_output_field=output_labels`);
    console.error(`actual=${missingLabelError ?? "ok"}`);
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
  assertSelfTest(
    evaluate(
      runbook.replace(
        "context_anomaly_runbook_density_workflow_missing_commands",
        "",
      ),
      maxLines,
    ),
    "context_anomaly_runbook_density_missing_failure_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`workflow_commands`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`env_docs`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`output_labels`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`key=value`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(
      runbook.replace("`P28_CONTEXT_ANOMALY_RUNBOOK_JSON_HEADROOM_MIN`", ""),
      maxLines,
    ),
    "context_anomaly_runbook_density_missing_env_docs",
  );
  assertSelfTest(
    evaluateWorkflow(
      workflow.replace(
        "          node scripts/check_context_anomaly_runbook_density.mjs\n",
        "",
      ),
    ),
    "context_anomaly_runbook_density_workflow_missing_commands",
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
    { P28_CONTEXT_ANOMALY_RUNBOOK_PROSE_MAX: "10" },
    [],
    "context_anomaly_runbook_density_prose_too_wide",
  );
  assertEnvFailure(
    { P28_CONTEXT_ANOMALY_RUNBOOK_JSON_MAX: "10" },
    ["--json"],
    "context_anomaly_runbook_density_json_too_long",
  );
  assertEnvFailure(
    { P28_CONTEXT_ANOMALY_RUNBOOK_JSON_HEADROOM_MIN: "999" },
    ["--json"],
    "context_anomaly_runbook_density_json_too_long",
  );
  assertSelfTest(
    jsonBudgetIssue(successPayload(result, workflowResult, 10), 10) ?? {
      code: "ok",
    },
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
if (!workflowResult.ok) {
  const { code, ok, ...details } = workflowResult;
  fail(code, details);
}

const payload = successPayload(result, workflowResult, maxJsonBytes);
const jsonHeadroom = jsonHeadroomBytes(payload, maxJsonBytes);
if (args.includes("--json")) {
  const issue = jsonBudgetIssue(payload, maxJsonBytes);
  if (issue) {
    const { code, ok, ...details } = issue;
    fail(code, details);
  }
  console.log(JSON.stringify(payload));
} else {
  console.log(renderDefaultOutput(payload, result, jsonHeadroom));
}
