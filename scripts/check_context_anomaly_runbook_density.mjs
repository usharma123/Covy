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
const defaultMaxJsonBytes = 480;
const maxJsonBytes = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_JSON_MAX ??
    String(defaultMaxJsonBytes),
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
const softTableRowLength = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_ROW_SOFT_MAX ?? "480",
  10,
);
const maxDensityProseLineLength = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_PROSE_MAX ?? "420",
  10,
);
const maxDefaultOutputLength = Number.parseInt(
  process.env.P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX ?? "210",
  10,
);
const maxHelpLineLength = 120;
const helpLines = [
  "Usage: node scripts/check_context_anomaly_runbook_density.mjs [--json|--self-test|--help]",
  "default: validate lines, rows, soft env P28_CONTEXT_ANOMALY_RUNBOOK_ROW_SOFT_MAX, docs, and workflow commands",
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
  "context_anomaly_runbook_density_text_too_wide",
  "context_anomaly_runbook_density_json_too_long",
];
const requiredOutputLabels = [
  "lines",
  "row",
  "cmds",
  "fc",
  "wf",
  "prose",
  "json",
  "env",
  "labels",
  "phrases",
  "adocs",
  "dphr",
  "anc",
  "soft",
  "parsed",
  "width",
  "wdocs",
];
const defaultTextFields = [...requiredOutputLabels];
const requiredOutputDocPhrases = [
  "`key=value`",
  "JSON keeps full field names",
  "`alias_docs_checked`",
  "`row_soft_ok`",
  "`row_soft_max`",
  "`density_doc_phrases_checked`",
  "`density_doc_anchors_checked`",
  "`parsed_fields_checked`",
  "`text_width_docs_checked`",
  "`max_json_bytes=480`",
  "`help<=120`",
];
const requiredAliasDocPhrases = [
  "`fc`=failure codes",
  "`wf`=workflow commands",
  "`json`=remaining JSON headroom",
  "`adocs`=alias docs",
  "`wdocs`=width docs",
];
const requiredDensityDocPhrases = [
  "Env:",
  "`Env:`=env",
  "`JSON:`=fields",
  "`h:`=help",
];
const requiredDensityDocLinePrefixes = [
  "Env:",
  "JSON:",
  "Density failures cont.:",
  "Density doc failures cont.:",
];
const requiredEnvDocs = [
  "P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES",
  "P28_CONTEXT_ANOMALY_RUNBOOK_ROW_MAX",
  "P28_CONTEXT_ANOMALY_RUNBOOK_ROW_SOFT_MAX",
  "P28_CONTEXT_ANOMALY_RUNBOOK_PROSE_MAX",
  "P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX",
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
        (line.startsWith("Density ") ||
          line.startsWith("Env:") ||
          line.startsWith("JSON:") ||
          line.includes("context_anomaly_runbook_density_")),
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
    (phrase) => !runbook.includes(phrase),
  );
  const missingAliasDocPhrases = requiredAliasDocPhrases.filter(
    (phrase) => !runbook.includes(phrase),
  );
  const missingDensityDocPhrases = requiredDensityDocPhrases.filter(
    (phrase) => !runbook.includes(phrase),
  );
  const missingDensityDocLinePrefixes = requiredDensityDocLinePrefixes.filter(
    (prefix) => !runbook.split("\n").some((line) => line.startsWith(prefix)),
  );
  const hasTextWidthEnvDoc = runbook
    .split("\n")
    .some(
      (line) =>
        line.includes("`width`") &&
        line.includes("`P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX`"),
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
  if (
    missingOutputLabels.length > 0 ||
    missingOutputDocPhrases.length > 0 ||
    missingAliasDocPhrases.length > 0 ||
    missingDensityDocPhrases.length > 0 ||
    missingDensityDocLinePrefixes.length > 0 ||
    !hasTextWidthEnvDoc
  ) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_missing_output_docs",
      missing: [
        ...missingOutputLabels,
        ...missingOutputDocPhrases,
        ...missingAliasDocPhrases,
        ...missingDensityDocPhrases,
        ...missingDensityDocLinePrefixes.map((prefix) => `${prefix}line`),
        ...(hasTextWidthEnvDoc
          ? []
          : ["width:P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX"]),
      ],
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
    row_soft_ok: maxActualTableRowLength <= softTableRowLength,
    row_soft_max: softTableRowLength,
    max_density_prose_line: maxActualDensityProseLineLength,
    max_density_prose_line_allowed: maxDensityProseLineLength,
    commands_checked: requiredCommands.length,
    failure_codes_checked: requiredFailureCodes.length,
    env_docs_checked: requiredEnvDocs.length,
    output_labels_checked: requiredOutputLabels.length,
    output_doc_phrases_checked: requiredOutputDocPhrases.length,
    alias_docs_checked: requiredAliasDocPhrases.length,
    density_doc_phrases_checked: requiredDensityDocPhrases.length,
    density_doc_anchors_checked: requiredDensityDocLinePrefixes.length,
    parsed_fields_checked: defaultTextFields.length,
    text_width_docs_checked: hasTextWidthEnvDoc ? 1 : 0,
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

let selfTestCaseIndex = 0;
function assertSelfTest(result, expectedCode, caseName = "") {
  selfTestCaseIndex += 1;
  if (result.code !== expectedCode) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`case_index=${selfTestCaseIndex}`);
    if (caseName) {
      console.error(`case=${caseName}`);
    }
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

function assertEnvOutput(env, commandArgs, expectedText) {
  const output = execFileSync(process.execPath, [scriptPath, ...commandArgs], {
    encoding: "utf8",
    env: { ...process.env, ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (!output.includes(expectedText)) {
    console.error("context_anomaly_runbook_density_self_test_env_output_failed");
    console.error(`expected=${expectedText}`);
    console.error(`actual=${output.trim()}`);
    process.exit(1);
  }
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
    alias_docs_checked: result.alias_docs_checked,
    density_doc_phrases_checked: result.density_doc_phrases_checked,
    density_doc_anchors_checked: result.density_doc_anchors_checked,
    parsed_fields_checked: result.parsed_fields_checked,
    text_width_docs_checked: result.text_width_docs_checked,
    row_soft_ok: result.row_soft_ok,
    row_soft_max: result.row_soft_max,
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

function defaultOutputIssue(payload, resultDetails, jsonHeadroom) {
  const line = renderDefaultOutputWithWidth(payload, resultDetails, jsonHeadroom);
  if (line.length > maxDefaultOutputLength) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_text_too_wide",
      default_output_len: line.length,
      max_default_output_len: maxDefaultOutputLength,
    };
  }
  return null;
}

function renderDefaultOutput(payload, resultDetails, jsonHeadroom, textWidth) {
  return [
    `context_anomaly_runbook_density_ok lines=${payload.line_count}/${payload.max_lines}`,
    `row=${payload.max_table_row}/${payload.max_table_row_allowed}`,
    `cmds=${payload.commands_checked}`,
    `fc=${payload.failure_codes_checked}`,
    `env=${resultDetails.env_docs_checked}`,
    `labels=${resultDetails.output_labels_checked}`,
    `phrases=${resultDetails.output_doc_phrases_checked}`,
    `adocs=${resultDetails.alias_docs_checked}`,
    `dphr=${resultDetails.density_doc_phrases_checked}`,
    `anc=${resultDetails.density_doc_anchors_checked}`,
    `soft=${resultDetails.row_soft_ok ? "ok" : "over"}`,
    `parsed=${resultDetails.parsed_fields_checked}`,
    `wf=${payload.workflow_commands_checked}`,
    `prose=${payload.max_density_prose_line}/${payload.max_density_prose_line_allowed}`,
    `json=${jsonHeadroom}`,
    `wdocs=${resultDetails.text_width_docs_checked}`,
    textWidth === undefined ? null : `width=${textWidth}`,
  ]
    .filter(Boolean)
    .join(" ");
}

function renderDefaultOutputWithWidth(payload, resultDetails, jsonHeadroom) {
  let line = renderDefaultOutput(payload, resultDetails, jsonHeadroom, 0);
  for (let i = 0; i < 3; i += 1) {
    const next = renderDefaultOutput(
      payload,
      resultDetails,
      jsonHeadroom,
      line.length,
    );
    if (next.length === line.length) {
      return next;
    }
    line = next;
  }
  return line;
}

function parseDefaultOutput(line) {
  const parts = line.trim().split(/\s+/);
  if (parts[0] !== "context_anomaly_runbook_density_ok") {
    return null;
  }
  return Object.fromEntries(parts.slice(1).map((part) => part.split("=")));
}

function defaultOutputParseIssue(parsed, resultDetails) {
  for (const field of defaultTextFields) {
    if (!parsed?.[field]) {
      return `missing_default_output_field=${field}`;
    }
  }
  const expectedValues = {
    cmds: String(resultDetails.commands_checked),
    labels: String(resultDetails.output_labels_checked),
    adocs: String(resultDetails.alias_docs_checked),
    dphr: String(resultDetails.density_doc_phrases_checked),
    anc: String(resultDetails.density_doc_anchors_checked),
    soft: resultDetails.row_soft_ok ? "ok" : "over",
    parsed: String(resultDetails.parsed_fields_checked),
    wdocs: String(resultDetails.text_width_docs_checked),
  };
  for (const [field, expected] of Object.entries(expectedValues)) {
    if (parsed[field] !== expected) {
      return `default_output_parse_mismatch=${field}`;
    }
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
  if (result.alias_docs_checked !== requiredAliasDocPhrases.length) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_alias_docs=${requiredAliasDocPhrases.length}`);
    console.error(`actual_alias_docs=${result.alias_docs_checked}`);
    process.exit(1);
  }
  if (result.density_doc_phrases_checked !== requiredDensityDocPhrases.length) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_density_doc_phrases=${requiredDensityDocPhrases.length}`,
    );
    console.error(
      `actual_density_doc_phrases=${result.density_doc_phrases_checked}`,
    );
    process.exit(1);
  }
  if (
    result.density_doc_anchors_checked !==
    requiredDensityDocLinePrefixes.length
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_density_doc_anchors=${requiredDensityDocLinePrefixes.length}`,
    );
    console.error(
      `actual_density_doc_anchors=${result.density_doc_anchors_checked}`,
    );
    process.exit(1);
  }
  if (result.parsed_fields_checked !== defaultTextFields.length) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_parsed_fields=${defaultTextFields.length}`);
    console.error(`actual_parsed_fields=${result.parsed_fields_checked}`);
    process.exit(1);
  }
  if (!result.row_soft_ok) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_row_soft_max=${result.row_soft_max}`);
    console.error(`actual_row=${result.max_table_row}`);
    process.exit(1);
  }
  const widenedEnvProseResult = evaluate(
    runbook.replace(
      "Env:",
      "Env: widened-env-prose-width-sentinel-extra-long",
    ),
    maxLines,
  );
  if (
    !widenedEnvProseResult.ok ||
    widenedEnvProseResult.max_density_prose_line <=
      result.max_density_prose_line
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("env_prose_width_did_not_grow");
    process.exit(1);
  }
  const widenedJsonProseResult = evaluate(
    runbook.replace("JSON:", `JSON:${" widened".repeat(32)}`),
    maxLines,
  );
  if (
    !widenedJsonProseResult.ok ||
    widenedJsonProseResult.max_density_prose_line <=
      result.max_density_prose_line
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("json_prose_width_did_not_grow");
    process.exit(1);
  }
  if (result.text_width_docs_checked !== 1) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected_text_width_docs=1");
    console.error(`actual_text_width_docs=${result.text_width_docs_checked}`);
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
  if (
    baselinePayload.alias_docs_checked !== result.alias_docs_checked ||
    baselinePayload.row_soft_ok !== result.row_soft_ok ||
    baselinePayload.row_soft_max !== result.row_soft_max ||
    baselinePayload.density_doc_phrases_checked !==
      result.density_doc_phrases_checked ||
    baselinePayload.density_doc_anchors_checked !==
      result.density_doc_anchors_checked ||
    baselinePayload.parsed_fields_checked !== result.parsed_fields_checked ||
    baselinePayload.text_width_docs_checked !==
      result.text_width_docs_checked
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("json_payload_mismatch");
    process.exit(1);
  }
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
  const defaultOutputLine = renderDefaultOutputWithWidth(
    baselinePayload,
    result,
    baselineHeadroom,
  );
  const parsedDefaultOutput = parseDefaultOutput(defaultOutputLine);
  const defaultOutputError = defaultOutputParseIssue(parsedDefaultOutput, result);
  if (defaultOutputError) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(defaultOutputError);
    process.exit(1);
  }
  if (parsedDefaultOutput.width !== String(defaultOutputLine.length)) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_text_width=${defaultOutputLine.length}`);
    console.error(`actual_text_width=${parsedDefaultOutput.width}`);
    process.exit(1);
  }
  if (parseDefaultOutput("bad_prefix lines=44/44") !== null) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("malformed_default_output_prefix_accepted");
    process.exit(1);
  }
  const missingLabelError = defaultOutputParseIssue(
    parseDefaultOutput(
      renderDefaultOutput(baselinePayload, result, baselineHeadroom).replace(
        / labels=\d+/,
        "",
      ),
    ),
    result,
  );
  if (missingLabelError !== "missing_default_output_field=labels") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected=missing_default_output_field=labels`);
    console.error(`actual=${missingLabelError ?? "ok"}`);
    process.exit(1);
  }
  const missingTextWidthDocsError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ wdocs=\d+/, "")),
    result,
  );
  if (
    missingTextWidthDocsError !==
    "missing_default_output_field=wdocs"
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected=missing_default_output_field=wdocs`);
    console.error(`actual=${missingTextWidthDocsError ?? "ok"}`);
    process.exit(1);
  }
  const staleTextWidthDocsError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ wdocs=\d+/, " wdocs=0")),
    result,
  );
  if (staleTextWidthDocsError !== "default_output_parse_mismatch=wdocs") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=default_output_parse_mismatch=wdocs");
    console.error(`actual=${staleTextWidthDocsError ?? "ok"}`);
    process.exit(1);
  }
  const staleCommandCountError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ cmds=\d+/, " cmds=0")),
    result,
  );
  if (staleCommandCountError !== "default_output_parse_mismatch=cmds") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=default_output_parse_mismatch=cmds");
    console.error(`actual=${staleCommandCountError ?? "ok"}`);
    process.exit(1);
  }
  const staleLabelCountError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ labels=\d+/, " labels=0")),
    result,
  );
  if (staleLabelCountError !== "default_output_parse_mismatch=labels") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=default_output_parse_mismatch=labels");
    console.error(`actual=${staleLabelCountError ?? "ok"}`);
    process.exit(1);
  }
  const staleDensityDocCountError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ dphr=\d+/, " dphr=0")),
    result,
  );
  if (staleDensityDocCountError !== "default_output_parse_mismatch=dphr") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=default_output_parse_mismatch=dphr");
    console.error(`actual=${staleDensityDocCountError ?? "ok"}`);
    process.exit(1);
  }
  const staleAliasDocCountError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ adocs=\d+/, " adocs=0")),
    result,
  );
  if (staleAliasDocCountError !== "default_output_parse_mismatch=adocs") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=default_output_parse_mismatch=adocs");
    console.error(`actual=${staleAliasDocCountError ?? "ok"}`);
    process.exit(1);
  }
  const staleDensityDocAnchorError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ anc=\d+/, " anc=0")),
    result,
  );
  if (staleDensityDocAnchorError !== "default_output_parse_mismatch=anc") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=default_output_parse_mismatch=anc");
    console.error(`actual=${staleDensityDocAnchorError ?? "ok"}`);
    process.exit(1);
  }
  const missingDensityDocAnchorError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ anc=\d+/, "")),
    result,
  );
  if (missingDensityDocAnchorError !== "missing_default_output_field=anc") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=missing_default_output_field=anc");
    console.error(`actual=${missingDensityDocAnchorError ?? "ok"}`);
    process.exit(1);
  }
  const staleSoftStatusError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ soft=\w+/, " soft=over")),
    result,
  );
  if (staleSoftStatusError !== "default_output_parse_mismatch=soft") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=default_output_parse_mismatch=soft");
    console.error(`actual=${staleSoftStatusError ?? "ok"}`);
    process.exit(1);
  }
  const missingSoftStatusError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ soft=\w+/, "")),
    result,
  );
  if (missingSoftStatusError !== "missing_default_output_field=soft") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=missing_default_output_field=soft");
    console.error(`actual=${missingSoftStatusError ?? "ok"}`);
    process.exit(1);
  }
  const missingParsedFieldCountError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ parsed=\d+/, "")),
    result,
  );
  if (
    missingParsedFieldCountError !== "missing_default_output_field=parsed"
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=missing_default_output_field=parsed");
    console.error(`actual=${missingParsedFieldCountError ?? "ok"}`);
    process.exit(1);
  }
  const staleParsedFieldCountError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ parsed=\d+/, " parsed=0")),
    result,
  );
  if (staleParsedFieldCountError !== "default_output_parse_mismatch=parsed") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=default_output_parse_mismatch=parsed");
    console.error(`actual=${staleParsedFieldCountError ?? "ok"}`);
    process.exit(1);
  }
  const missingDensityDocCountError = defaultOutputParseIssue(
    parseDefaultOutput(defaultOutputLine.replace(/ dphr=\d+/, "")),
    result,
  );
  if (
    missingDensityDocCountError !== "missing_default_output_field=dphr"
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected=missing_default_output_field=dphr");
    console.error(`actual=${missingDensityDocCountError ?? "ok"}`);
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
    evaluate(runbook.replaceAll("`wf`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`env`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`labels`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`phrases`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replaceAll("`adocs`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`dphr`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`anc`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`parsed`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`soft`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`key=value`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("JSON keeps full field names", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`alias_docs_checked`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`row_soft_ok`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`density_doc_phrases_checked`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`density_doc_anchors_checked`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`parsed_fields_checked`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`text_width_docs_checked`", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(
      runbook.replace(`\`max_json_bytes=${defaultMaxJsonBytes}\``, ""),
      maxLines,
    ),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace(`\`help<=${maxHelpLineLength}\``, ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replaceAll("Env:", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`JSON:`=fields", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("JSON:`row_soft_ok`", "`row_soft_ok`"), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(
      runbook.replace(
        "Env:`P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES`",
        "`P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES`",
      ),
      maxLines,
    ),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(
      runbook.replace("Density failures cont.:", "Density failures merged:"),
      maxLines,
    ),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(
      runbook.replace(
        "Density doc failures cont.:",
        "Density doc failures merged:",
      ),
      maxLines,
    ),
    "context_anomaly_runbook_density_missing_output_docs",
  );
  assertSelfTest(
    evaluate(runbook.replace("`fc`=failure codes", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
    "missing_fc_alias_glossary",
  );
  assertSelfTest(
    evaluate(runbook.replace("`adocs`=alias docs", ""), maxLines),
    "context_anomaly_runbook_density_missing_output_docs",
    "missing_adocs_alias_glossary",
  );
  assertSelfTest(
    evaluate(
      runbook.replace(
        "`width` is capped by `P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX`",
        "`width` has a text cap",
      ),
      maxLines,
    ),
    "context_anomaly_runbook_density_missing_output_docs",
    "missing_width_env_pair",
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
  assertEnvOutput(
    { P28_CONTEXT_ANOMALY_RUNBOOK_ROW_SOFT_MAX: "10" },
    [],
    "soft=over",
  );
  assertEnvOutput(
    { P28_CONTEXT_ANOMALY_RUNBOOK_ROW_SOFT_MAX: "10" },
    ["--json"],
    '"row_soft_ok":false',
  );
  assertEnvFailure(
    { P28_CONTEXT_ANOMALY_RUNBOOK_PROSE_MAX: "10" },
    [],
    "context_anomaly_runbook_density_prose_too_wide",
  );
  assertEnvFailure(
    { P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX: "10" },
    [],
    "context_anomaly_runbook_density_text_too_wide",
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
    "P28_CONTEXT_ANOMALY_RUNBOOK_ROW_SOFT_MAX",
  ]) {
    assertHelpIncludes(expected);
  }
  const tooWideHelpLine = helpLines.find(
    (line) => line.length > maxHelpLineLength,
  );
  if (tooWideHelpLine) {
    console.error("context_anomaly_runbook_density_self_test_help_failed");
    console.error(`max_help_line_len=${maxHelpLineLength}`);
    console.error(`actual_help_line_len=${tooWideHelpLine.length}`);
    process.exit(1);
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
const defaultOutputWidthIssue = defaultOutputIssue(payload, result, jsonHeadroom);
if (args.includes("--json")) {
  const issue = jsonBudgetIssue(payload, maxJsonBytes);
  if (issue) {
    const { code, ok, ...details } = issue;
    fail(code, details);
  }
  console.log(JSON.stringify(payload));
} else {
  if (defaultOutputWidthIssue) {
    const { code, ok, ...details } = defaultOutputWidthIssue;
    fail(code, details);
  }
  console.log(renderDefaultOutputWithWidth(payload, result, jsonHeadroom));
}
