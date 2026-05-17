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
// 640 keeps full-field JSON parity output compact while preserving the
// explicit headroom gate after adding default_output_iterations.
const defaultMaxJsonBytes = 640;
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
const minDefaultOutputHeadroom = 8;
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
const defaultOutputFieldOrder = [
  "lines",
  "row",
  "cmds",
  "fc",
  "env",
  "lbl",
  "phr",
  "adocs",
  "dphr",
  "anc",
  "soft",
  "prs",
  "wf",
  "prose",
  "dlab",
  "jhead",
  "jpar",
  "wdocs",
  "thead",
  "tw",
];
const requiredOutputLabels = [...defaultOutputFieldOrder];
const defaultTextFields = [...defaultOutputFieldOrder];
const defaultOutputDocPhrase = `\`key=value\`: ${defaultOutputFieldOrder
  .map((label) => `\`${label}\``)
  .join(", ")};`;
const jsonPayloadParityFieldOrder = [
  "output_doc_phrases_checked",
  "alias_docs_checked",
  "density_doc_phrases_checked",
  "density_doc_anchors_checked",
  "parsed_fields_checked",
  "json_parity_fields_checked",
  "density_label_line_width",
  "default_output_headroom",
  "default_output_iterations",
  "text_width_docs_checked",
  "row_soft_ok",
  "row_soft_max",
];
const requiredOutputDocPhrases = [
  "`key=value`",
  "JSON keeps full field names",
  defaultOutputDocPhrase,
  "`output_doc_phrases_checked`",
  "`alias_docs_checked`",
  "`row_soft_ok`",
  "`row_soft_max`",
  "`density_doc_phrases_checked`",
  "`density_doc_anchors_checked`",
  "`parsed_fields_checked`",
  "`json_parity_fields_checked`",
  "`density_label_line_width`",
  "`default_output_headroom`",
  "`default_output_iterations`",
  "`text_width_docs_checked`",
  "`density_label_line_width`,`default_output_headroom`,`default_output_iterations`,`text_width_docs_checked`",
  "`ok:false`",
  "`no-succ`",
  "`ok:false`;`no-succ`;h:`help<=120`",
  "`max_json_bytes=640`",
  "`thead>=8`",
  "`help<=120`",
];
const requiredAliasDocPhrases = [
  "`fc`=failure codes",
  "`wf`=workflow commands",
  "`jhead`=JSON headroom",
  "`jpar`=JSON parity",
  "`adocs`=alias docs",
  "`wdocs`=width docs",
  "`dlab`=density label width",
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
const pairedEnvDocExclusions = [
  "P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX",
  "P28_CONTEXT_ANOMALY_RUNBOOK_JSON_HEADROOM_MIN",
];
const expectedPairedEnvDocExclusionCount = 2;
const pairedEnvDocExclusionCount = pairedEnvDocExclusions.length;
const requiredPlainEnvDocs = requiredEnvDocs.filter(
  (envName) => !pairedEnvDocExclusions.includes(envName),
);

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
        line.includes("`tw`") &&
        line.includes("`P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX`"),
    );
  const hasJsonHeadroomEnvDoc = runbook
    .split("\n")
    .some(
      (line) =>
        line.includes("`jhead`") &&
        line.includes("`P28_CONTEXT_ANOMALY_RUNBOOK_JSON_HEADROOM_MIN`"),
    );
  const hasStaleFailureAlias = runbook.includes("`no-success`");
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
    !hasTextWidthEnvDoc ||
    !hasJsonHeadroomEnvDoc ||
    hasStaleFailureAlias
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
          : ["tw:P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX"]),
        ...(hasJsonHeadroomEnvDoc
          ? []
          : ["jhead:P28_CONTEXT_ANOMALY_RUNBOOK_JSON_HEADROOM_MIN"]),
        ...(hasStaleFailureAlias ? ["stale:no-success"] : []),
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
    json_parity_fields_checked: jsonPayloadParityFieldOrder.length,
    density_label_line_width: densityLabelLineLength(runbook),
    text_width_docs_checked: hasTextWidthEnvDoc ? 1 : 0,
  };
}

function densityLabelLineLength(runbook) {
  return (
    runbook
      .split("\n")
      .find((line) => line.startsWith("Density labels:"))?.length ?? 0
  );
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

function failIssue(issue) {
  const { code, ok, ...details } = issue;
  fail(code, details);
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

function assertSelfTestMissing(result, expectedCode, expectedMissing, caseName) {
  assertSelfTest(result, expectedCode, caseName);
  if (!result.missing?.includes(expectedMissing)) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`case=${caseName}`);
    console.error(`missing_detail=${expectedMissing}`);
    console.error(`actual_missing=${result.missing?.join(",") ?? ""}`);
    process.exit(1);
  }
}

function invariantDetailValue(value) {
  return Array.isArray(value) ? value.join(",") : value;
}

const invariantArrayDetailSample = ["array", "detail"];
const invariantScalarDetailSample = "scalar";
const expectedInvariantArrayDetail = invariantArrayDetailSample.join(",");
const expectedInvariantScalarDetail = invariantScalarDetailSample;

function expectedInvariantDetailFormats() {
  return {
    expectedInvariantArrayDetail,
    expectedInvariantScalarDetail,
  };
}

function invariantDetailFormatSamples() {
  return {
    actualInvariantArrayDetail: invariantDetailValue(invariantArrayDetailSample),
    actualInvariantScalarDetail: invariantDetailValue(
      invariantScalarDetailSample,
    ),
  };
}

function invariantDetailFormatsMismatch(expectedFormats, actualFormats) {
  return (
    actualFormats.actualInvariantArrayDetail !==
      expectedFormats.expectedInvariantArrayDetail ||
    actualFormats.actualInvariantScalarDetail !==
      expectedFormats.expectedInvariantScalarDetail
  );
}

function invariantDetailFormatMismatchDetails(expectedFormats, actualFormats) {
  return {
    expected_invariant_array_detail_format:
      expectedFormats.expectedInvariantArrayDetail,
    expected_invariant_scalar_detail_format:
      expectedFormats.expectedInvariantScalarDetail,
    actual_invariant_array_detail_format:
      actualFormats.actualInvariantArrayDetail,
    actual_invariant_scalar_detail_format:
      actualFormats.actualInvariantScalarDetail,
  };
}

function failSelfTestInvariant(details) {
  console.error("context_anomaly_runbook_density_self_test_failed");
  for (const [key, value] of Object.entries(details)) {
    console.error(`${key}=${invariantDetailValue(value)}`);
  }
  process.exit(1);
}

function assertInvariantDetailFormats() {
  const expectedInvariantFormats = expectedInvariantDetailFormats();
  const actualInvariantFormats = invariantDetailFormatSamples();
  if (
    invariantDetailFormatsMismatch(
      expectedInvariantFormats,
      actualInvariantFormats,
    )
  ) {
    failSelfTestInvariant(
      invariantDetailFormatMismatchDetails(
        expectedInvariantFormats,
        actualInvariantFormats,
      ),
    );
  }
}

function assertPairedEnvDocExclusionCount() {
  if (pairedEnvDocExclusionCount !== expectedPairedEnvDocExclusionCount) {
    failSelfTestInvariant({
      expected_paired_env_doc_exclusion_count:
        expectedPairedEnvDocExclusionCount,
      actual_paired_env_doc_exclusion_count: pairedEnvDocExclusionCount,
    });
  }
}

function assertRequiredPlainEnvDocCount() {
  if (
    requiredPlainEnvDocs.length !==
    requiredEnvDocs.length - pairedEnvDocExclusionCount
  ) {
    failSelfTestInvariant({
      expected_plain_env_docs:
        requiredEnvDocs.length - pairedEnvDocExclusionCount,
      actual_plain_env_docs: requiredPlainEnvDocs.length,
    });
  }
}

function assertPairedEnvDocExclusionsOmitted() {
  for (const excludedEnvName of pairedEnvDocExclusions) {
    if (requiredPlainEnvDocs.includes(excludedEnvName)) {
      failSelfTestInvariant({
        unexpected_plain_env_doc: excludedEnvName,
      });
    }
  }
}

function assertEnvDocInvariants() {
  assertPairedEnvDocExclusionCount();
  assertRequiredPlainEnvDocCount();
  assertPairedEnvDocExclusionsOmitted();
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

function assertEnvFailureOutput(env, commandArgs, expectedTexts) {
  try {
    execFileSync(process.execPath, [scriptPath, ...commandArgs], {
      encoding: "utf8",
      env: { ...process.env, ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (error) {
    const output = `${error.stdout ?? ""}${error.stderr ?? ""}`;
    const missing = expectedTexts.filter((text) => !output.includes(text));
    if (missing.length === 0) {
      return;
    }
    console.error("context_anomaly_runbook_density_self_test_env_failed");
    console.error(`missing=${missing.join(",")}`);
    console.error(`actual=${output.trim()}`);
    process.exit(1);
  }
  console.error("context_anomaly_runbook_density_self_test_env_failed");
  console.error(`expected=${expectedTexts.join(",")}`);
  console.error("actual=ok");
  process.exit(1);
}

function assertEnvFailureExcludes(env, commandArgs, forbiddenText) {
  try {
    const output = execFileSync(process.execPath, [scriptPath, ...commandArgs], {
      encoding: "utf8",
      env: { ...process.env, ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    console.error("context_anomaly_runbook_density_self_test_env_failed");
    console.error(`expected_failure_without=${forbiddenText}`);
    console.error(`actual=${output.trim()}`);
    process.exit(1);
  } catch (error) {
    const output = `${error.stdout ?? ""}${error.stderr ?? ""}`;
    if (!output.includes(forbiddenText)) {
      return;
    }
    console.error("context_anomaly_runbook_density_self_test_env_failed");
    console.error(`forbidden=${forbiddenText}`);
    console.error(`actual=${output.trim()}`);
    process.exit(1);
  }
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

function successPayload(
  result,
  workflow,
  jsonBudget,
  defaultOutputHeadroom,
  defaultOutputIterations,
) {
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
    output_doc_phrases_checked: result.output_doc_phrases_checked,
    alias_docs_checked: result.alias_docs_checked,
    density_doc_phrases_checked: result.density_doc_phrases_checked,
    density_doc_anchors_checked: result.density_doc_anchors_checked,
    parsed_fields_checked: result.parsed_fields_checked,
    json_parity_fields_checked: result.json_parity_fields_checked,
    density_label_line_width: result.density_label_line_width,
    ...(defaultOutputHeadroom === undefined
      ? {}
      : { default_output_headroom: defaultOutputHeadroom }),
    ...(defaultOutputIterations === undefined
      ? {}
      : { default_output_iterations: defaultOutputIterations }),
    text_width_docs_checked: result.text_width_docs_checked,
    row_soft_ok: result.row_soft_ok,
    row_soft_max: result.row_soft_max,
    workflow_commands_checked: workflow.workflow_commands_checked,
    max_json_bytes: jsonBudget,
  };
}

function jsonPayloadParityExpectedFields(result, derived = {}) {
  const values = {
    output_doc_phrases_checked: result.output_doc_phrases_checked,
    alias_docs_checked: result.alias_docs_checked,
    density_doc_phrases_checked: result.density_doc_phrases_checked,
    density_doc_anchors_checked: result.density_doc_anchors_checked,
    parsed_fields_checked: result.parsed_fields_checked,
    json_parity_fields_checked: result.json_parity_fields_checked,
    density_label_line_width: result.density_label_line_width,
    default_output_headroom: derived.default_output_headroom,
    default_output_iterations: derived.default_output_iterations,
    text_width_docs_checked: result.text_width_docs_checked,
    row_soft_ok: result.row_soft_ok,
    row_soft_max: result.row_soft_max,
  };
  return Object.fromEntries(
    jsonPayloadParityFieldOrder.map((field) => [field, values[field]]),
  );
}

function jsonPayloadParityIssue(payload, result, derived = {}) {
  const expectedFields = jsonPayloadParityExpectedFields(result, derived);
  for (const [field, expected] of Object.entries(expectedFields)) {
    if (!Object.prototype.hasOwnProperty.call(payload, field)) {
      return `missing_json_payload_field=${field}`;
    }
    if (payload[field] !== expected) {
      return `json_payload_mismatch=${field}`;
    }
  }
  return null;
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

function defaultOutputIssue(
  payload,
  resultDetails,
  jsonHeadroom,
  textBudget = maxDefaultOutputLength,
) {
  const line = renderDefaultOutputWithWidth(
    payload,
    resultDetails,
    jsonHeadroom,
    textBudget,
  );
  if (line.length > textBudget) {
    return {
      ok: false,
      code: "context_anomaly_runbook_density_text_too_wide",
      default_output_len: line.length,
      max_default_output_len: textBudget,
    };
  }
  return null;
}

function buildSuccessArtifacts(
  resultDetails,
  workflow,
  jsonBudget,
  textBudget = maxDefaultOutputLength,
) {
  let artifacts = buildSuccessArtifactsWithIteration(
    resultDetails,
    workflow,
    jsonBudget,
    textBudget,
    undefined,
  );
  for (let i = 0; i < 3; i += 1) {
    const nextArtifacts = buildSuccessArtifactsWithIteration(
      resultDetails,
      workflow,
      jsonBudget,
      textBudget,
      artifacts.iterations,
    );
    if (
      nextArtifacts.iterations === artifacts.iterations &&
      nextArtifacts.payload.default_output_iterations ===
        nextArtifacts.iterations
    ) {
      return nextArtifacts;
    }
    artifacts = nextArtifacts;
  }
  return artifacts;
}

function buildSuccessArtifactsWithIteration(
  resultDetails,
  workflow,
  jsonBudget,
  textBudget,
  defaultOutputIterations,
) {
  let payload = successPayload(
    resultDetails,
    workflow,
    jsonBudget,
    0,
    defaultOutputIterations,
  );
  let jsonHeadroom = jsonHeadroomBytes(payload, jsonBudget);
  let defaultOutputLine = renderDefaultOutputWithWidth(
    payload,
    resultDetails,
    jsonHeadroom,
    textBudget,
  );
  for (let i = 0; i < 5; i += 1) {
    const defaultOutputHeadroom = textBudget - defaultOutputLine.length;
    const nextPayload = successPayload(
      resultDetails,
      workflow,
      jsonBudget,
      defaultOutputHeadroom,
      defaultOutputIterations,
    );
    const nextJsonHeadroom = jsonHeadroomBytes(nextPayload, jsonBudget);
    const nextDefaultOutputLine = renderDefaultOutputWithWidth(
      nextPayload,
      resultDetails,
      nextJsonHeadroom,
      textBudget,
    );
    if (
      JSON.stringify(nextPayload) === JSON.stringify(payload) &&
      nextJsonHeadroom === jsonHeadroom &&
      nextDefaultOutputLine === defaultOutputLine
    ) {
      return {
        payload,
        jsonHeadroom,
        defaultOutputLine,
        defaultOutputHeadroom,
        iterations: i + 1,
      };
    }
    payload = nextPayload;
    jsonHeadroom = nextJsonHeadroom;
    defaultOutputLine = nextDefaultOutputLine;
  }
  const defaultOutputHeadroom = textBudget - defaultOutputLine.length;
  return {
    payload,
    jsonHeadroom,
    defaultOutputLine,
    defaultOutputHeadroom,
    iterations: 5,
  };
}

function renderDefaultOutput(
  payload,
  resultDetails,
  jsonHeadroom,
  textWidth,
  textHeadroom,
) {
  return [
    `context_anomaly_runbook_density_ok lines=${payload.line_count}/${payload.max_lines}`,
    `row=${payload.max_table_row}/${payload.max_table_row_allowed}`,
    `cmds=${payload.commands_checked}`,
    `fc=${payload.failure_codes_checked}`,
    `env=${resultDetails.env_docs_checked}`,
    `lbl=${resultDetails.output_labels_checked}`,
    `phr=${resultDetails.output_doc_phrases_checked}`,
    `adocs=${resultDetails.alias_docs_checked}`,
    `dphr=${resultDetails.density_doc_phrases_checked}`,
    `anc=${resultDetails.density_doc_anchors_checked}`,
    `soft=${resultDetails.row_soft_ok ? "ok" : "over"}`,
    `prs=${resultDetails.parsed_fields_checked}`,
    `wf=${payload.workflow_commands_checked}`,
    `prose=${payload.max_density_prose_line}/${payload.max_density_prose_line_allowed}`,
    `dlab=${resultDetails.density_label_line_width}`,
    `jhead=${jsonHeadroom}`,
    `jpar=${resultDetails.json_parity_fields_checked}`,
    `wdocs=${resultDetails.text_width_docs_checked}`,
    textHeadroom === undefined ? null : `thead=${textHeadroom}`,
    textWidth === undefined ? null : `tw=${textWidth}`,
  ]
    .filter(Boolean)
    .join(" ");
}

function renderDefaultOutputWithWidth(
  payload,
  resultDetails,
  jsonHeadroom,
  textBudget = maxDefaultOutputLength,
) {
  let textWidth = 0;
  let textHeadroom = 0;
  let line = renderDefaultOutput(
    payload,
    resultDetails,
    jsonHeadroom,
    textWidth,
    textHeadroom,
  );
  for (let i = 0; i < 5; i += 1) {
    const nextTextWidth = line.length;
    const nextTextHeadroom = textBudget - nextTextWidth;
    const next = renderDefaultOutput(
      payload,
      resultDetails,
      jsonHeadroom,
      nextTextWidth,
      nextTextHeadroom,
    );
    if (
      next.length === line.length &&
      nextTextWidth === textWidth &&
      nextTextHeadroom === textHeadroom
    ) {
      return next;
    }
    textWidth = nextTextWidth;
    textHeadroom = nextTextHeadroom;
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

function defaultOutputExpectedValues(resultDetails) {
  const expectedValues = {
    lines: `${resultDetails.line_count}/${resultDetails.max_lines}`,
    row: `${resultDetails.max_table_row}/${resultDetails.max_table_row_allowed}`,
    cmds: String(resultDetails.commands_checked),
    fc: String(resultDetails.failure_codes_checked),
    env: String(resultDetails.env_docs_checked),
    lbl: String(resultDetails.output_labels_checked),
    phr: String(resultDetails.output_doc_phrases_checked),
    prose: `${resultDetails.max_density_prose_line}/${resultDetails.max_density_prose_line_allowed}`,
    dlab: String(resultDetails.density_label_line_width),
    jpar: String(resultDetails.json_parity_fields_checked),
    adocs: String(resultDetails.alias_docs_checked),
    dphr: String(resultDetails.density_doc_phrases_checked),
    anc: String(resultDetails.density_doc_anchors_checked),
    soft: resultDetails.row_soft_ok ? "ok" : "over",
    prs: String(resultDetails.parsed_fields_checked),
    wdocs: String(resultDetails.text_width_docs_checked),
  };
  if (resultDetails.workflow_commands_checked !== undefined) {
    expectedValues.wf = String(resultDetails.workflow_commands_checked);
  }
  if (resultDetails.json_headroom !== undefined) {
    expectedValues.jhead = String(resultDetails.json_headroom);
  }
  if (resultDetails.default_output_headroom !== undefined) {
    expectedValues.thead = String(resultDetails.default_output_headroom);
  }
  if (resultDetails.default_output_width !== undefined) {
    expectedValues.tw = String(resultDetails.default_output_width);
  }
  return expectedValues;
}

function defaultOutputParseIssue(parsed, resultDetails) {
  for (const field of defaultTextFields) {
    if (!parsed?.[field]) {
      return `missing_default_output_field=${field}`;
    }
  }
  const expectedValues = defaultOutputExpectedValues(resultDetails);
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
  const actualDensityLabelLineLength = densityLabelLineLength(runbook);
  if (result.density_label_line_width !== actualDensityLabelLineLength) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_density_label_width=${actualDensityLabelLineLength}`);
    console.error(`actual_density_label_width=${result.density_label_line_width}`);
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
    runbook.replace("JSON:", `JSON:${" widened".repeat(8)}`),
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
  const compactDensityLabelLineLength = densityLabelLineLength(runbook);
  const spacedDensityLabelsRunbook = runbook.replace(
    "Density labels:`Env:`=env,`JSON:`=fields,`h:`=help,`jpar`=JSON parity,`adocs`=alias docs,`wdocs`=width docs,`jhead` uses",
    "Density labels: `Env:`=env, `JSON:`=fields, `h:`=help, `jpar`=JSON parity, `adocs`=alias docs, `wdocs`=width docs, `jhead` uses",
  );
  if (spacedDensityLabelsRunbook === runbook) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("density_label_spacing_mutation_noop");
    process.exit(1);
  }
  const spacedDensityLabelLineLength = densityLabelLineLength(
    spacedDensityLabelsRunbook,
  );
  if (spacedDensityLabelLineLength <= compactDensityLabelLineLength) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("density_label_spacing_width_did_not_grow");
    process.exit(1);
  }
  const spacedDensityLabelResult = evaluate(spacedDensityLabelsRunbook, maxLines);
  if (
    !spacedDensityLabelResult.ok ||
    spacedDensityLabelResult.density_label_line_width <=
      result.density_label_line_width
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("density_label_spacing_metric_did_not_grow");
    process.exit(1);
  }
  const spacedDensityLabelPayload = buildSuccessArtifacts(
    spacedDensityLabelResult,
    workflowResult,
    maxJsonBytes,
  ).payload;
  if (
    spacedDensityLabelPayload.density_label_line_width <=
    result.density_label_line_width
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("density_label_spacing_json_metric_did_not_grow");
    process.exit(1);
  }
  if (result.text_width_docs_checked !== 1) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("expected_text_width_docs=1");
    console.error(`actual_text_width_docs=${result.text_width_docs_checked}`);
    process.exit(1);
  }
  if (result.json_parity_fields_checked !== jsonPayloadParityFieldOrder.length) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_json_parity_fields=${jsonPayloadParityFieldOrder.length}`,
    );
    console.error(
      `actual_json_parity_fields=${result.json_parity_fields_checked}`,
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
  const baselineArtifacts = buildSuccessArtifacts(
    result,
    workflowResult,
    maxJsonBytes,
  );
  const {
    payload: baselinePayload,
    jsonHeadroom: baselineHeadroom,
    defaultOutputLine,
    defaultOutputHeadroom,
  } = baselineArtifacts;
  const rebuiltBaselineArtifacts = buildSuccessArtifacts(
    result,
    workflowResult,
    maxJsonBytes,
  );
  if (
    JSON.stringify(rebuiltBaselineArtifacts.payload) !==
      JSON.stringify(baselinePayload) ||
    rebuiltBaselineArtifacts.jsonHeadroom !== baselineHeadroom ||
    rebuiltBaselineArtifacts.defaultOutputLine !== defaultOutputLine ||
    rebuiltBaselineArtifacts.defaultOutputHeadroom !== defaultOutputHeadroom
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("success_artifacts_did_not_converge");
    process.exit(1);
  }
  if (baselineArtifacts.iterations > 3) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`success_artifacts_iterations=${baselineArtifacts.iterations}`);
    console.error(`success_artifacts_len=${defaultOutputLine.length}`);
    console.error(`success_artifacts_headroom=${defaultOutputHeadroom}`);
    process.exit(1);
  }
  if (
    baselinePayload.default_output_iterations !== baselineArtifacts.iterations
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_default_output_iterations=${baselineArtifacts.iterations}`,
    );
    console.error(
      `actual_default_output_iterations=${baselinePayload.default_output_iterations}`,
    );
    process.exit(1);
  }
  const exactWidthArtifacts = buildSuccessArtifacts(
    result,
    workflowResult,
    maxJsonBytes,
    196,
  );
  if (
    exactWidthArtifacts.defaultOutputHeadroom !== 0 ||
    exactWidthArtifacts.defaultOutputLine.length !== 196 ||
    !exactWidthArtifacts.defaultOutputLine.includes("thead=0 tw=196") ||
    exactWidthArtifacts.payload.default_output_headroom !== 0
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("exact_width_artifacts_did_not_converge");
    console.error(`actual_exact_width_len=${exactWidthArtifacts.defaultOutputLine.length}`);
    console.error(`actual_exact_width_headroom=${exactWidthArtifacts.defaultOutputHeadroom}`);
    console.error(
      `actual_exact_width_json_headroom=${exactWidthArtifacts.payload.default_output_headroom}`,
    );
    process.exit(1);
  }
  if (exactWidthArtifacts.iterations > 3) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`exact_width_iterations=${exactWidthArtifacts.iterations}`);
    console.error(`exact_width_len=${exactWidthArtifacts.defaultOutputLine.length}`);
    console.error(`exact_width_headroom=${exactWidthArtifacts.defaultOutputHeadroom}`);
    process.exit(1);
  }
  const lowWidthArtifacts = buildSuccessArtifacts(
    result,
    workflowResult,
    maxJsonBytes,
    195,
  );
  const lowWidthIssue = defaultOutputIssue(
    lowWidthArtifacts.payload,
    result,
    lowWidthArtifacts.jsonHeadroom,
    195,
  );
  if (lowWidthIssue?.code !== "context_anomaly_runbook_density_text_too_wide") {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("low_width_artifact_issue_missing");
    process.exit(1);
  }
  if (
    lowWidthIssue.default_output_len !==
      lowWidthArtifacts.defaultOutputLine.length ||
    lowWidthIssue.max_default_output_len !== 195
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("low_width_artifact_issue_details_mismatch");
    process.exit(1);
  }
  const jsonPayloadError = jsonPayloadParityIssue(baselinePayload, result, {
    default_output_headroom: defaultOutputHeadroom,
    default_output_iterations: baselineArtifacts.iterations,
  });
  if (jsonPayloadError) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(jsonPayloadError);
    process.exit(1);
  }
  if (baselinePayload.density_label_line_width !== actualDensityLabelLineLength) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_json_dlab=${actualDensityLabelLineLength}`);
    console.error(
      `actual_json_dlab=${baselinePayload.density_label_line_width}`,
    );
    process.exit(1);
  }
  const jsonParityFieldOrder = Object.keys(
    jsonPayloadParityExpectedFields(result, {
      default_output_headroom: defaultOutputHeadroom,
      default_output_iterations: baselineArtifacts.iterations,
    }),
  );
  // Keep parity expectations in success-payload order so JSON readers see the
  // same field sequence that the self-test validates.
  const payloadParityFieldOrder = Object.keys(baselinePayload).filter((field) =>
    jsonParityFieldOrder.includes(field),
  );
  if (payloadParityFieldOrder.join(",") !== jsonParityFieldOrder.join(",")) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_json_parity_order=${jsonParityFieldOrder.join(",")}`,
    );
    console.error(
      `actual_json_parity_order=${payloadParityFieldOrder.join(",")}`,
    );
    process.exit(1);
  }
  const jsonWidthFieldSequence = [
    "density_label_line_width",
    "default_output_headroom",
    "default_output_iterations",
    "text_width_docs_checked",
  ];
  const actualJsonWidthFieldSequence = payloadParityFieldOrder.filter((field) =>
    jsonWidthFieldSequence.includes(field),
  );
  if (
    actualJsonWidthFieldSequence.join(",") !==
    jsonWidthFieldSequence.join(",")
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_json_width_field_order=${jsonWidthFieldSequence.join(",")}`,
    );
    console.error(
      `actual_json_width_field_order=${actualJsonWidthFieldSequence.join(",")}`,
    );
    process.exit(1);
  }
  const jsonIterationFieldSequence = [
    "default_output_headroom",
    "default_output_iterations",
    "text_width_docs_checked",
  ];
  const actualJsonIterationFieldSequence = payloadParityFieldOrder.filter(
    (field) => jsonIterationFieldSequence.includes(field),
  );
  if (
    actualJsonIterationFieldSequence.join(",") !==
    jsonIterationFieldSequence.join(",")
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_json_iteration_field_order=${jsonIterationFieldSequence.join(",")}`,
    );
    console.error(
      `actual_json_iteration_field_order=${actualJsonIterationFieldSequence.join(",")}`,
    );
    process.exit(1);
  }
  // Stale JSON values must differ from the rendered payload; the helper below
  // fails fast if any mutation becomes a no-op.
  const staleJsonPayloadValues = {
    output_doc_phrases_checked: 0,
    alias_docs_checked: 0,
    row_soft_ok: !baselinePayload.row_soft_ok,
    row_soft_max: 0,
    density_doc_phrases_checked: 0,
    density_doc_anchors_checked: 0,
    parsed_fields_checked: 0,
    json_parity_fields_checked: 0,
    density_label_line_width: 0,
    default_output_headroom: 0,
    default_output_iterations: 0,
    text_width_docs_checked: 0,
  };
  const missingJsonParityMutationFields = Object.keys(
    jsonPayloadParityExpectedFields(result, {
      default_output_headroom: defaultOutputHeadroom,
      default_output_iterations: baselineArtifacts.iterations,
    }),
  ).filter((field) => staleJsonPayloadValues[field] === undefined);
  if (missingJsonParityMutationFields.length > 0) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `missing_json_parity_mutation_fields=${missingJsonParityMutationFields.join(",")}`,
    );
    process.exit(1);
  }
  const assertJsonParityMutation = (field, staleValue) => {
    const missingJsonPayload = { ...baselinePayload };
    delete missingJsonPayload[field];
    const missingJsonPayloadError = jsonPayloadParityIssue(
      missingJsonPayload,
      result,
      {
        default_output_headroom: defaultOutputHeadroom,
        default_output_iterations: baselineArtifacts.iterations,
      },
    );
    if (missingJsonPayloadError !== `missing_json_payload_field=${field}`) {
      console.error("context_anomaly_runbook_density_self_test_failed");
      console.error(`expected=missing_json_payload_field=${field}`);
      console.error(`actual=${missingJsonPayloadError ?? "ok"}`);
      process.exit(1);
    }
    if (staleValue === baselinePayload[field]) {
      console.error("context_anomaly_runbook_density_self_test_failed");
      console.error(`json_parity_mutation_noop=${field}`);
      process.exit(1);
    }
    const staleJsonPayloadError = jsonPayloadParityIssue(
      { ...baselinePayload, [field]: staleValue },
      result,
      {
        default_output_headroom: defaultOutputHeadroom,
        default_output_iterations: baselineArtifacts.iterations,
      },
    );
    if (staleJsonPayloadError !== `json_payload_mismatch=${field}`) {
      console.error("context_anomaly_runbook_density_self_test_failed");
      console.error(`expected=json_payload_mismatch=${field}`);
      console.error(`actual=${staleJsonPayloadError ?? "ok"}`);
      process.exit(1);
    }
  };
  for (const [field, staleValue] of Object.entries(staleJsonPayloadValues)) {
    assertJsonParityMutation(field, staleValue);
  }
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
  const defaultParseDetails = {
    ...result,
    workflow_commands_checked: baselinePayload.workflow_commands_checked,
    json_headroom: baselineHeadroom,
    default_output_headroom: defaultOutputHeadroom,
    default_output_width: defaultOutputLine.length,
  };
  // These fields are derived outside evaluate(), but parser parity still
  // requires exact values for every compact default-output label.
  const requiredDefaultParseDetailFields = [
    "workflow_commands_checked",
    "json_headroom",
    "default_output_headroom",
    "default_output_width",
  ];
  const missingDefaultParseDetailFields =
    requiredDefaultParseDetailFields.filter(
      (field) => defaultParseDetails[field] === undefined,
    );
  if (missingDefaultParseDetailFields.length > 0) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `missing_default_parse_details=${missingDefaultParseDetailFields.join(",")}`,
    );
    process.exit(1);
  }
  const expectedParserFields = Object.keys(
    defaultOutputExpectedValues(defaultParseDetails),
  ).sort();
  const requiredParserFields = [...defaultTextFields].sort();
  if (expectedParserFields.join(",") !== requiredParserFields.join(",")) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_parser_fields=${requiredParserFields.join(",")}`,
    );
    console.error(`actual_parser_fields=${expectedParserFields.join(",")}`);
    process.exit(1);
  }
  const parsedDefaultOutput = parseDefaultOutput(defaultOutputLine);
  const parsedDefaultOutputFields = Object.keys(parsedDefaultOutput);
  if (
    parsedDefaultOutputFields.join(",") !== defaultOutputFieldOrder.join(",")
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_field_order=${defaultOutputFieldOrder.join(",")}`);
    console.error(`actual_field_order=${parsedDefaultOutputFields.join(",")}`);
    process.exit(1);
  }
  const expectedDefaultOutputSuffix = ["thead", "tw"];
  const actualDefaultOutputSuffix = parsedDefaultOutputFields.slice(
    -expectedDefaultOutputSuffix.length,
  );
  if (
    actualDefaultOutputSuffix.join(",") !==
    expectedDefaultOutputSuffix.join(",")
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_default_output_suffix=${expectedDefaultOutputSuffix.join(",")}`,
    );
    console.error(
      `actual_default_output_suffix=${actualDefaultOutputSuffix.join(",")}`,
    );
    process.exit(1);
  }
  const defaultOutputError = defaultOutputParseIssue(
    parsedDefaultOutput,
    defaultParseDetails,
  );
  if (defaultOutputError) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(defaultOutputError);
    process.exit(1);
  }
  if (parsedDefaultOutput.dlab !== String(actualDensityLabelLineLength)) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_dlab=${actualDensityLabelLineLength}`);
    console.error(`actual_dlab=${parsedDefaultOutput.dlab}`);
    process.exit(1);
  }
  if (parsedDefaultOutput.tw !== String(defaultOutputLine.length)) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(`expected_text_width=${defaultOutputLine.length}`);
    console.error(`actual_text_width=${parsedDefaultOutput.tw}`);
    process.exit(1);
  }
  if (
    parsedDefaultOutput.thead !==
    String(maxDefaultOutputLength - defaultOutputLine.length)
  ) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_text_headroom=${maxDefaultOutputLength - defaultOutputLine.length}`,
    );
    console.error(`actual_text_headroom=${parsedDefaultOutput.thead}`);
    process.exit(1);
  }
  if (defaultOutputHeadroom < minDefaultOutputHeadroom) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error(
      `expected_text_headroom_at_least=${minDefaultOutputHeadroom}`,
    );
    console.error(`actual_text_headroom=${defaultOutputHeadroom}`);
    process.exit(1);
  }
  if (parseDefaultOutput("bad_prefix lines=44/44") !== null) {
    console.error("context_anomaly_runbook_density_self_test_failed");
    console.error("malformed_default_output_prefix_accepted");
    process.exit(1);
  }
  // Each stale value must be parseable, field-specific, and impossible to
  // equal the current rendered value; the loop below rejects no-op mutations.
  const staleDefaultOutputValues = {
    lines: "0/0",
    row: "0/0",
    cmds: "0",
    fc: "0",
    wf: "0",
    prose: "0/0",
    jhead: "0",
    env: "0",
    lbl: "0",
    phr: "0",
    adocs: "0",
    dphr: "0",
    anc: "0",
    dlab: "0",
    jpar: "0",
    soft: "over",
    prs: "0",
    thead: "0",
    tw: "0",
    wdocs: "0",
  };
  const missingStaleMutationFields = defaultTextFields.filter(
    (field) => staleDefaultOutputValues[field] === undefined,
  );
  const failDefaultOutputMutation = (details) => {
    failSelfTestInvariant(details);
  };
  const missingStaleMutationFieldsDetails = (fields) => ({
    missing_stale_mutation_fields: fields,
  });
  const missingMutationNoopDetails = (field) => ({
    missing_mutation_noop: field,
  });
  const isDefaultOutputMissingMutationNoop = (originalLine, missingLine) =>
    missingLine === originalLine;
  const staleMutationNoopDetails = (field) => ({
    stale_mutation_noop: field,
  });
  const parsedDefaultOutputFieldValue = (field) => parsedDefaultOutput[field];
  const isDefaultOutputStaleMutationNoop = (
    originalLine,
    staleLine,
    field,
    staleValue,
  ) =>
    staleLine === originalLine ||
    staleValue === parsedDefaultOutputFieldValue(field);
  const missingDefaultOutputFieldExpectation = (field) =>
    `missing_default_output_field=${field}`;
  const staleDefaultOutputFieldExpectation = (field) =>
    `default_output_parse_mismatch=${field}`;
  const defaultOutputFieldPattern = (field) => new RegExp(` ${field}=\\S+`);
  const defaultOutputWithoutField = (line, field) =>
    line.replace(defaultOutputFieldPattern(field), "");
  const defaultOutputWithStaleField = (line, field, staleValue) =>
    line.replace(defaultOutputFieldPattern(field), ` ${field}=${staleValue}`);
  const defaultOutputMutationActualDetail = (actual) => actual ?? "ok";
  const defaultOutputMutationMismatchDetails = (expected, actual) => ({
    expected,
    actual: defaultOutputMutationActualDetail(actual),
  });
  const assertStaleDefaultOutputValuesCovered = () => {
    if (missingStaleMutationFields.length > 0) {
      failDefaultOutputMutation(
        missingStaleMutationFieldsDetails(missingStaleMutationFields),
      );
    }
  };
  const assertDefaultOutputMissingMutation = (field) => {
    const missingOutputLine = defaultOutputWithoutField(defaultOutputLine, field);
    if (
      isDefaultOutputMissingMutationNoop(defaultOutputLine, missingOutputLine)
    ) {
      failDefaultOutputMutation(missingMutationNoopDetails(field));
    }
    const missingFieldError = defaultOutputParseIssue(
      parseDefaultOutput(missingOutputLine),
      defaultParseDetails,
    );
    const expectedMissingFieldError =
      missingDefaultOutputFieldExpectation(field);
    if (missingFieldError !== expectedMissingFieldError) {
      failDefaultOutputMutation(
        defaultOutputMutationMismatchDetails(
          expectedMissingFieldError,
          missingFieldError,
        ),
      );
    }
  };
  const assertDefaultOutputStaleMutation = (field, staleValue) => {
    const staleOutputLine = defaultOutputWithStaleField(
      defaultOutputLine,
      field,
      staleValue,
    );
    if (
      isDefaultOutputStaleMutationNoop(
        defaultOutputLine,
        staleOutputLine,
        field,
        staleValue,
      )
    ) {
      failDefaultOutputMutation(staleMutationNoopDetails(field));
    }
    const staleFieldError = defaultOutputParseIssue(
      parseDefaultOutput(staleOutputLine),
      defaultParseDetails,
    );
    const expectedStaleFieldError = staleDefaultOutputFieldExpectation(field);
    if (staleFieldError !== expectedStaleFieldError) {
      failDefaultOutputMutation(
        defaultOutputMutationMismatchDetails(
          expectedStaleFieldError,
          staleFieldError,
        ),
      );
    }
  };
  const assertDefaultOutputMutation = (field, staleValue) => {
    assertDefaultOutputMissingMutation(field);
    assertDefaultOutputStaleMutation(field, staleValue);
  };
  const staleDefaultOutputValue = (field) => staleDefaultOutputValues[field];
  const defaultOutputMutationCase = (field) => ({
    mutationField: field,
    staleValue: staleDefaultOutputValue(field),
  });
  const assertDefaultOutputMutationCase = (field) => {
    const { mutationField, staleValue } = defaultOutputMutationCase(field);
    assertDefaultOutputMutation(mutationField, staleValue);
  };
  const defaultOutputMutationFields = () => defaultTextFields;
  const expectedDefaultOutputMutationFieldListCount = () =>
    defaultOutputMutationFields().length;
  const actualStaleDefaultOutputValueCount = () =>
    Object.keys(staleDefaultOutputValues).length;
  const defaultOutputMutationFieldCountSample = () => ({
    expectedMutationFields: expectedDefaultOutputMutationFieldListCount(),
    actualMutationFields: actualStaleDefaultOutputValueCount(),
  });
  const defaultOutputMutationFieldCountsMismatch = (sample) =>
    sample.actualMutationFields !== sample.expectedMutationFields;
  const defaultOutputMutationFieldCountDetails = (sample) => ({
    expected_default_output_mutation_fields: sample.expectedMutationFields,
    actual_default_output_mutation_fields: sample.actualMutationFields,
  });
  const expectedDefaultOutputMutationIdentityFields = () => defaultTextFields;
  const actualDefaultOutputMutationIdentityFields = () =>
    defaultOutputMutationFields();
  const defaultOutputMutationIdentityFieldSample = () => ({
    expectedMutationFields: expectedDefaultOutputMutationIdentityFields(),
    actualMutationFields: actualDefaultOutputMutationIdentityFields(),
  });
  const defaultOutputMutationIdentityFieldDetails = (sample) => ({
    expected_default_output_mutation_fields: sample.expectedMutationFields,
    actual_default_output_mutation_fields: sample.actualMutationFields,
  });
  const defaultOutputMutationIdentityFieldStringPair = (sample) => ({
    actualJoinedMutationFieldString: sample.actualMutationFields.join(","),
    expectedJoinedMutationFieldString: sample.expectedMutationFields.join(","),
  });
  const defaultOutputMutationIdentityFieldStringPairMatches = (
    fieldStringPair,
  ) =>
    fieldStringPair.actualJoinedMutationFieldString ===
    fieldStringPair.expectedJoinedMutationFieldString;
  const sampleDefaultOutputMutationIdentityFieldStringPair = (sample) =>
    defaultOutputMutationIdentityFieldStringPair(sample);
  const sampledDefaultOutputMutationIdentityFieldStringPairMatches = (sample) => {
    const fieldStringPair =
      sampleDefaultOutputMutationIdentityFieldStringPair(sample);
    return defaultOutputMutationIdentityFieldStringPairMatches(fieldStringPair);
  };
  const failDefaultOutputMutationFieldIdentity = (sample) => {
    failDefaultOutputMutation(defaultOutputMutationIdentityFieldDetails(sample));
  };
  const sampleDefaultOutputMutationIdentityFields = () =>
    defaultOutputMutationIdentityFieldSample();
  const assertSampledDefaultOutputMutationIdentityFieldPair = (sample) => {
    if (!sampledDefaultOutputMutationIdentityFieldStringPairMatches(sample)) {
      failDefaultOutputMutationFieldIdentity(sample);
    }
  };
  const assertSampledDefaultOutputMutationIdentityFieldPairSample = (
    sample,
  ) => {
    assertSampledDefaultOutputMutationIdentityFieldPair(sample);
  };
  const assertSampledDefaultOutputMutationIdentityFieldSample = () => {
    const sample = sampleDefaultOutputMutationIdentityFields();
    assertSampledDefaultOutputMutationIdentityFieldPairSample(sample);
  };
  const assertDefaultOutputMutationIdentityFieldSample = () => {
    assertSampledDefaultOutputMutationIdentityFieldSample();
  };
  const defaultOutputMutationFieldCountFailureDetails = (sample) =>
    defaultOutputMutationFieldCountDetails(sample);
  const failDefaultOutputMutationFieldCount = (sample) => {
    const details = defaultOutputMutationFieldCountFailureDetails(sample);
    failDefaultOutputMutation(details);
  };
  const assertDefaultOutputMutationFieldCountSample = () => {
    const sample = defaultOutputMutationFieldCountSample();
    if (defaultOutputMutationFieldCountsMismatch(sample)) {
      failDefaultOutputMutationFieldCount(sample);
    }
  };
  const assertDefaultOutputMutationCases = () => {
    defaultOutputMutationFields().forEach(assertDefaultOutputMutationCase);
  };
  const assertDefaultOutputMutations = () => {
    assertStaleDefaultOutputValuesCovered();
    assertDefaultOutputMutationIdentityFieldSample();
    assertDefaultOutputMutationFieldCountSample();
    assertDefaultOutputMutationCases();
  };
  const assertDefaultOutputMutationSelfTest = () => {
    assertDefaultOutputMutations();
  };
  const runContextAnomalyDensitySelfTests = () => {
    assertDefaultOutputMutationSelfTest();
    const assertInvariantDetailFormatSelfTest = () => {
      assertInvariantDetailFormats();
    };
    assertInvariantDetailFormatSelfTest();
    const assertEnvDocInvariantSelfTest = () => {
      assertEnvDocInvariants();
    };
    assertEnvDocInvariantSelfTest();
    const assertLineCountBoundarySelfTest = () => {
      assertSelfTest(
        evaluate(runbook, result.line_count - 1),
        "context_anomaly_runbook_density_too_many_lines",
      );
    };
    assertLineCountBoundarySelfTest();
    const assertRequiredCommandMutationSelfTests = () => {
      for (const [commandIndex, command] of requiredCommands.entries()) {
        assertSelfTestMissing(
          evaluate(
            runbook.replaceAll(
              command,
              `drifted_required_command_${commandIndex}`,
            ),
            maxLines,
          ),
          "context_anomaly_runbook_density_missing_commands",
          command,
          `drifted_required_command_${commandIndex}`,
        );
      }
    };
    assertRequiredCommandMutationSelfTests();
    const assertRequiredFailureCodeMutationSelfTests = () => {
      for (const [
        failureCodeIndex,
        failureCode,
      ] of requiredFailureCodes.entries()) {
        assertSelfTestMissing(
          evaluate(
            runbook.replace(
              failureCode,
              `drifted_failure_code_${failureCodeIndex}`,
            ),
            maxLines,
          ),
          "context_anomaly_runbook_density_missing_failure_docs",
          failureCode,
          `drifted_${failureCode}`,
        );
      }
    };
    assertRequiredFailureCodeMutationSelfTests();
    const assertOutputLabelMutationSelfTests = () => {
      assertSelfTestMissing(
        evaluate(runbook.replaceAll("`wf`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "wf",
        "missing_wf_output_label",
      );
      assertSelfTestMissing(
        evaluate(runbook.replace("`env`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "env",
        "missing_env_output_label",
      );
      assertSelfTestMissing(
        evaluate(runbook.replace("`lbl`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "lbl",
        "missing_lbl_output_label",
      );
      assertSelfTestMissing(
        evaluate(runbook.replace("`phr`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "phr",
        "missing_phr_output_label",
      );
      assertSelfTestMissing(
        evaluate(runbook.replaceAll("`adocs`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "adocs",
        "missing_adocs_output_label",
      );
      assertSelfTestMissing(
        evaluate(runbook.replace("`dphr`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "dphr",
        "missing_dphr_output_label",
      );
      assertSelfTestMissing(
        evaluate(runbook.replace("`anc`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "anc",
        "missing_anc_output_label",
      );
      assertSelfTestMissing(
        evaluate(runbook.replace("`prs`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "prs",
        "missing_prs_output_label",
      );
      assertSelfTestMissing(
        evaluate(runbook.replace("`soft`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "soft",
        "missing_soft_output_label",
      );
    };
    assertOutputLabelMutationSelfTests();
    const assertKeyValueOutputDocMutationSelfTest = () => {
      assertSelfTestMissing(
        evaluate(runbook.replace("`key=value`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "`key=value`",
        "missing_key_value_output_doc",
      );
    };
    assertKeyValueOutputDocMutationSelfTest();
    const assertDefaultOutputPhraseMutationSelfTests = () => {
      for (const label of defaultOutputFieldOrder) {
        assertSelfTestMissing(
          evaluate(
            runbook.replace(
              defaultOutputDocPhrase,
              defaultOutputDocPhrase.replace(
                `\`${label}\``,
                `\`${label}_drift\``,
              ),
            ),
            maxLines,
          ),
          "context_anomaly_runbook_density_missing_output_docs",
          defaultOutputDocPhrase,
          `drifted_${label}_default_output_doc_phrase`,
        );
      }
    };
    assertDefaultOutputPhraseMutationSelfTests();
    const assertOutputOrderSwapMutationSelfTests = () => {
      assertSelfTestMissing(
        evaluate(
          runbook.replace("`env`, `lbl`", "`lbl`, `env`"),
          maxLines,
        ),
        "context_anomaly_runbook_density_missing_output_docs",
        defaultOutputDocPhrase,
        "swapped_env_lbl_output_order",
      );
      assertSelfTestMissing(
        evaluate(
          runbook.replace("`dlab`, `jhead`", "`jhead`, `dlab`"),
          maxLines,
        ),
        "context_anomaly_runbook_density_missing_output_docs",
        defaultOutputDocPhrase,
        "swapped_dlab_jhead_output_order",
      );
      assertSelfTestMissing(
        evaluate(runbook.replace("`thead`, `tw`", "`tw`, `thead`"), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        defaultOutputDocPhrase,
        "swapped_thead_tw_output_order",
      );
    };
    assertOutputOrderSwapMutationSelfTests();
    const assertJsonFieldNameDocMutationSelfTest = () => {
      assertSelfTestMissing(
        evaluate(runbook.replace("JSON keeps full field names", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "JSON keeps full field names",
        "missing_json_full_field_names_doc",
      );
    };
    assertJsonFieldNameDocMutationSelfTest();
    const assertJsonFieldDocMutationSelfTests = () => {
      const assertJsonFieldDocPresenceMutationSelfTests = (
        requiredJsonFieldDocs,
      ) => {
        for (const [fieldDoc, caseName] of requiredJsonFieldDocs) {
          assertSelfTestMissing(
            evaluate(runbook.replace(fieldDoc, ""), maxLines),
            "context_anomaly_runbook_density_missing_output_docs",
            fieldDoc,
            caseName,
          );
        }
      };
      const assertJsonFieldOrderDocMutationSelfTests = () => {
        const jsonHeadroomOrderDoc = [
          "`density_label_line_width`",
          "`default_output_headroom`",
          "`default_output_iterations`",
          "`text_width_docs_checked`",
        ].join(",");
        const swappedJsonHeadroomOrderDoc = [
          "`density_label_line_width`",
          "`text_width_docs_checked`",
          "`default_output_iterations`",
          "`default_output_headroom`",
        ].join(",");
        const jsonIterationsTextWidthPairDoc =
          "`default_output_iterations`,`text_width_docs_checked`";
        const swappedJsonIterationsTextWidthPairDoc =
          "`text_width_docs_checked`,`default_output_iterations`";
        const jsonFieldOrderMutations = [
          {
            caseName: "swapped_default_output_headroom_json_doc_order",
            expectedFullOrderDoc: jsonHeadroomOrderDoc,
            originalOrderDoc: jsonHeadroomOrderDoc,
            swappedOrderDoc: swappedJsonHeadroomOrderDoc,
          },
          {
            caseName: "swapped_default_output_iterations_json_doc_order",
            expectedFullOrderDoc: jsonHeadroomOrderDoc,
            originalOrderDoc: jsonIterationsTextWidthPairDoc,
            swappedOrderDoc: swappedJsonIterationsTextWidthPairDoc,
          },
        ];
        const assertJsonFieldOrderMutation = (jsonFieldOrderMutation) => {
          const {
            caseName,
            expectedFullOrderDoc,
            originalOrderDoc,
            swappedOrderDoc,
          } = jsonFieldOrderMutation;
          assertSelfTestMissing(
            evaluate(
              runbook.replace(originalOrderDoc, swappedOrderDoc),
              maxLines,
            ),
            "context_anomaly_runbook_density_missing_output_docs",
            expectedFullOrderDoc,
            caseName,
          );
        };
        for (const jsonFieldOrderMutation of jsonFieldOrderMutations) {
          assertJsonFieldOrderMutation(jsonFieldOrderMutation);
        }
      };
      const leadingJsonFieldDocs = [
        ["`alias_docs_checked`", "missing_alias_docs_checked_json_doc"],
        ["`row_soft_ok`", "missing_row_soft_ok_json_doc"],
        [
          "`density_doc_phrases_checked`",
          "missing_density_doc_phrases_checked_json_doc",
        ],
        [
          "`density_doc_anchors_checked`",
          "missing_density_doc_anchors_checked_json_doc",
        ],
        ["`parsed_fields_checked`", "missing_parsed_fields_checked_json_doc"],
        [
          "`json_parity_fields_checked`",
          "missing_json_parity_fields_checked_json_doc",
        ],
        [
          "`density_label_line_width`",
          "missing_density_label_line_width_json_doc",
        ],
        [
          "`default_output_headroom`",
          "missing_default_output_headroom_json_doc",
        ],
        [
          "`default_output_iterations`",
          "missing_default_output_iterations_json_doc",
        ],
      ];
      const trailingJsonFieldDocs = [
        ["`text_width_docs_checked`", "missing_text_width_docs_checked_json_doc"],
      ];
      assertJsonFieldDocPresenceMutationSelfTests(leadingJsonFieldDocs);
      assertJsonFieldOrderDocMutationSelfTests();
      assertJsonFieldDocPresenceMutationSelfTests(trailingJsonFieldDocs);
    };
    assertJsonFieldDocMutationSelfTests();
    const assertJsonByteHelpCapMutationSelfTests = () => {
      const jsonByteHelpCapDocs = [
        {
          caseName: "missing_max_json_bytes_doc",
          docText: `\`max_json_bytes=${defaultMaxJsonBytes}\``,
        },
        {
          caseName: "missing_help_cap_doc",
          docText: `\`help<=${maxHelpLineLength}\``,
        },
      ];
      for (const { caseName, docText } of jsonByteHelpCapDocs) {
        assertSelfTestMissing(
          evaluate(runbook.replace(docText, ""), maxLines),
          "context_anomaly_runbook_density_missing_output_docs",
          docText,
          caseName,
        );
      }
    };
    assertJsonByteHelpCapMutationSelfTests();
    const assertEnvLineAnchorMutationSelfTest = () => {
      assertSelfTestMissing(
        evaluate(runbook.replaceAll("Env:", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "Env:line",
        "missing_env_line_anchor",
      );
    };
    assertEnvLineAnchorMutationSelfTest();
    const assertSectionAnchorMutationSelfTests = () => {
      const sectionAnchorMutations = [
        {
          originalText: "`JSON:`=fields",
          replacementText: "",
        },
        {
          originalText: "JSON:`alias_docs_checked`",
          replacementText: "`alias_docs_checked`",
        },
        {
          originalText: "Env:`P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES`",
          replacementText: "`P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES`",
        },
        {
          originalText: "Density failures cont.:",
          replacementText: "Density failures merged:",
        },
        {
          originalText: "Density doc failures cont.:",
          replacementText: "Density doc failures merged:",
        },
      ];
      const assertSectionAnchorMutation = (sectionAnchorMutation) => {
        const { originalText, replacementText } = sectionAnchorMutation;
        assertSelfTest(
          evaluate(runbook.replace(originalText, replacementText), maxLines),
          "context_anomaly_runbook_density_missing_output_docs",
        );
      };
      for (const sectionAnchorMutation of sectionAnchorMutations) {
        assertSectionAnchorMutation(sectionAnchorMutation);
      }
    };
    assertSectionAnchorMutationSelfTests();
    const assertAliasGlossaryMutationSelfTests = () => {
      const aliasGlossaryDocs = [
        {
          caseName: "missing_fc_alias_glossary",
          docText: "`fc`=failure codes",
        },
        {
          caseName: "missing_jhead_alias_glossary",
          docText: "`jhead`=JSON headroom",
        },
        {
          caseName: "missing_adocs_alias_glossary",
          docText: "`adocs`=alias docs",
        },
        {
          caseName: "missing_dlab_alias_glossary",
          docText: "`dlab`=density label width",
        },
      ];
      const assertAliasGlossaryMutation = (aliasGlossaryDoc) => {
        const { caseName, docText } = aliasGlossaryDoc;
        assertSelfTest(
          evaluate(runbook.replace(docText, ""), maxLines),
          "context_anomaly_runbook_density_missing_output_docs",
          caseName,
        );
      };
      for (const aliasGlossaryDoc of aliasGlossaryDocs) {
        assertAliasGlossaryMutation(aliasGlossaryDoc);
      }
    };
    assertAliasGlossaryMutationSelfTests();
    const assertWidthEnvPairMutationSelfTest = () => {
      assertSelfTestMissing(
        evaluate(
          runbook.replace("`tw` cap:`P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX`", ""),
          maxLines,
        ),
        "context_anomaly_runbook_density_missing_output_docs",
        "tw:P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX",
        "missing_width_env_pair",
      );
    };
    assertWidthEnvPairMutationSelfTest();
    const assertTextHeadroomMutationSelfTest = () => {
      assertSelfTestMissing(
        evaluate(runbook.replace("`thead>=8`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "`thead>=8`",
        "missing_default_text_headroom_doc",
      );
    };
    assertTextHeadroomMutationSelfTest();
    const assertJsonErrorShapeMutationSelfTest = () => {
      assertSelfTestMissing(
        evaluate(runbook.replace("`ok:false`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "`ok:false`",
        "missing_json_error_shape_doc",
      );
    };
    assertJsonErrorShapeMutationSelfTest();
    const assertFailureSuccessFieldMutationSelfTest = () => {
      assertSelfTestMissing(
        evaluate(runbook.replace("`no-succ`", ""), maxLines),
        "context_anomaly_runbook_density_missing_output_docs",
        "`no-succ`",
        "missing_json_failure_success_field_doc",
      );
    };
    assertFailureSuccessFieldMutationSelfTest();
    const assertStaleFailureAliasMutationSelfTest = () => {
      const staleFailureAliasResult = evaluate(
        runbook.replace("`no-succ`", "`no-success`"),
        maxLines,
      );
      assertSelfTestMissing(
        staleFailureAliasResult,
        "context_anomaly_runbook_density_missing_output_docs",
        "stale:no-success",
        "stale_json_failure_success_field_alias",
      );
    };
    assertStaleFailureAliasMutationSelfTest();
    const assertJsonErrorHelpAdjacencyMutationSelfTest = () => {
      const jsonErrorHelpAdjacencyDoc = "`ok:false`;`no-succ`;h:`help<=120`";
      const driftedJsonErrorHelpAdjacencyDoc =
        "`ok:false`;`no-succ`;x:`help<=120`";
      assertSelfTestMissing(
        evaluate(
          runbook.replace(
            jsonErrorHelpAdjacencyDoc,
            driftedJsonErrorHelpAdjacencyDoc,
          ),
          maxLines,
        ),
        "context_anomaly_runbook_density_missing_output_docs",
        jsonErrorHelpAdjacencyDoc,
        "missing_json_error_help_adjacency_doc",
      );
    };
    assertJsonErrorHelpAdjacencyMutationSelfTest();
    const assertJsonHeadroomEnvPairMutationSelfTest = () => {
      const jsonHeadroomEnvPairDoc =
        "`jhead` uses `P28_CONTEXT_ANOMALY_RUNBOOK_JSON_HEADROOM_MIN`";
      const driftedJsonHeadroomEnvPairDoc = "`jhead` has headroom";
      assertSelfTestMissing(
        evaluate(
          runbook.replace(
            jsonHeadroomEnvPairDoc,
            driftedJsonHeadroomEnvPairDoc,
          ),
          maxLines,
        ),
        "context_anomaly_runbook_density_missing_output_docs",
        "jhead:P28_CONTEXT_ANOMALY_RUNBOOK_JSON_HEADROOM_MIN",
        "missing_jhead_env_pair",
      );
    };
    assertJsonHeadroomEnvPairMutationSelfTest();
    const assertPlainEnvDocMutationSelfTests = () => {
      for (const [envIndex, envName] of requiredPlainEnvDocs.entries()) {
        const driftedEnvDoc = `\`DRIFTED_ENV_DOC_${envIndex}\``;
        assertSelfTestMissing(
          evaluate(
            runbook.replaceAll(
              `\`${envName}\``,
              driftedEnvDoc,
            ),
            maxLines,
          ),
          "context_anomaly_runbook_density_missing_env_docs",
          envName,
          `drifted_${envName}`,
        );
      }
    };
    assertPlainEnvDocMutationSelfTests();
    const assertWorkflowCommandMutationSelfTests = () => {
      for (const [commandIndex, command] of requiredWorkflowDensityCommands.entries()) {
        const driftedWorkflowCommand =
          `drifted_workflow_command_${commandIndex}`;
        assertSelfTestMissing(
          evaluateWorkflow(
            workflow.replace(
              `          ${command}\n`,
              `          ${driftedWorkflowCommand}\n`,
            ),
          ),
          "context_anomaly_runbook_density_workflow_missing_commands",
          command,
          driftedWorkflowCommand,
        );
      }
    };
    assertWorkflowCommandMutationSelfTests();
    const assertEnvLimitFailureSelfTests = () => {
      const envLimitFailures = [
        {
          env: { P28_CONTEXT_ANOMALY_RUNBOOK_MAX_LINES: "10" },
          expectedCode: "context_anomaly_runbook_density_too_many_lines",
        },
        {
          env: { P28_CONTEXT_ANOMALY_RUNBOOK_ROW_MAX: "10" },
          expectedCode: "context_anomaly_runbook_density_row_too_wide",
        },
      ];
      for (const { env, expectedCode } of envLimitFailures) {
        assertEnvFailure(env, [], expectedCode);
      }
    };
    assertEnvLimitFailureSelfTests();
    const assertSoftRowOutputSelfTests = () => {
      const softRowOutputEnv = {
        P28_CONTEXT_ANOMALY_RUNBOOK_ROW_SOFT_MAX: "10",
      };
      const softRowOutputChecks = [
        {
          args: [],
          expectedText: "soft=over",
        },
        {
          args: ["--json"],
          expectedText: '"row_soft_ok":false',
        },
      ];
      for (const { args, expectedText } of softRowOutputChecks) {
        assertEnvOutput(softRowOutputEnv, args, expectedText);
      }
    };
    assertSoftRowOutputSelfTests();
    const assertProseTextEnvFailureSelfTests = () => {
      const proseTextEnvFailures = [
        {
          args: [],
          env: { P28_CONTEXT_ANOMALY_RUNBOOK_PROSE_MAX: "10" },
          expectedCode: "context_anomaly_runbook_density_prose_too_wide",
        },
        {
          args: [],
          env: { P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX: "10" },
          expectedCode: "context_anomaly_runbook_density_text_too_wide",
        },
        {
          args: [],
          env: { P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX: "195" },
          expectedCode: "context_anomaly_runbook_density_text_too_wide",
        },
        {
          args: ["--json"],
          env: { P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX: "195" },
          expectedCode: "context_anomaly_runbook_density_text_too_wide",
        },
      ];
      for (const { args, env, expectedCode } of proseTextEnvFailures) {
        assertEnvFailure(env, args, expectedCode);
      }
    };
    assertProseTextEnvFailureSelfTests();
    const assertTextJsonFailureShapeSelfTests = () => {
      const textJsonFailureEnv = {
        P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX: "195",
      };
      const textJsonFailureArgs = ["--json"];
      const expectedTextJsonFailureOutput = [
        '"ok":false',
        '"code":"context_anomaly_runbook_density_text_too_wide"',
        '"default_output_len":197',
        '"max_default_output_len":195',
      ];
      const excludedTextJsonFailureOutput = '"default_output_iterations"';
      assertEnvFailureOutput(
        textJsonFailureEnv,
        textJsonFailureArgs,
        expectedTextJsonFailureOutput,
      );
      assertEnvFailureExcludes(
        textJsonFailureEnv,
        textJsonFailureArgs,
        excludedTextJsonFailureOutput,
      );
    };
    assertTextJsonFailureShapeSelfTests();
    const assertTextHeadroomOutputSelfTests = () => {
      const textHeadroomOutputEnv = {
        P28_CONTEXT_ANOMALY_RUNBOOK_TEXT_MAX: "196",
      };
      const textHeadroomOutputChecks = [
        {
          args: [],
          expectedText: "thead=0 tw=196",
        },
        {
          args: ["--json"],
          expectedText: '"default_output_headroom":0',
        },
        {
          args: ["--json"],
          expectedText: '"default_output_iterations":1',
        },
      ];
      for (const { args, expectedText } of textHeadroomOutputChecks) {
        assertEnvOutput(textHeadroomOutputEnv, args, expectedText);
      }
    };
    assertTextHeadroomOutputSelfTests();
    const assertJsonMaxFailureShapeSelfTests = () => {
      const jsonMaxFailureEnv = {
        P28_CONTEXT_ANOMALY_RUNBOOK_JSON_MAX: "10",
      };
      const jsonFailureArgs = ["--json"];
      const expectedJsonMaxFailureOutput = [
        '"ok":false',
        '"code":"context_anomaly_runbook_density_json_too_long"',
      ];
      const excludedJsonMaxFailureOutput = '"default_output_iterations"';
      assertEnvFailure(
        jsonMaxFailureEnv,
        jsonFailureArgs,
        "context_anomaly_runbook_density_json_too_long",
      );
      assertEnvFailureOutput(
        jsonMaxFailureEnv,
        jsonFailureArgs,
        expectedJsonMaxFailureOutput,
      );
      assertEnvFailureExcludes(
        jsonMaxFailureEnv,
        jsonFailureArgs,
        excludedJsonMaxFailureOutput,
      );
    };
    assertJsonMaxFailureShapeSelfTests();
    const assertJsonHeadroomMinFailureShapeSelfTests = () => {
      const jsonHeadroomMinFailureEnv = {
        P28_CONTEXT_ANOMALY_RUNBOOK_JSON_HEADROOM_MIN: "999",
      };
      const jsonFailureArgs = ["--json"];
      const expectedJsonHeadroomMinFailureOutput = [
        '"ok":false',
        '"code":"context_anomaly_runbook_density_json_too_long"',
      ];
      const excludedJsonHeadroomMinFailureOutput =
        '"default_output_iterations"';

      assertEnvFailure(
        jsonHeadroomMinFailureEnv,
        jsonFailureArgs,
        "context_anomaly_runbook_density_json_too_long",
      );
      assertEnvFailureOutput(
        jsonHeadroomMinFailureEnv,
        jsonFailureArgs,
        expectedJsonHeadroomMinFailureOutput,
      );
      assertEnvFailureExcludes(
        jsonHeadroomMinFailureEnv,
        jsonFailureArgs,
        excludedJsonHeadroomMinFailureOutput,
      );
    };
    assertJsonHeadroomMinFailureShapeSelfTests();
    const assertJsonBudgetIssueMutationSelfTest = () => {
      const jsonBudgetIssueLimit = 10;
      const fallbackJsonBudgetIssueResult = { code: "ok" };
      const expectedJsonBudgetIssueCode =
        "context_anomaly_runbook_density_json_too_long";

      assertSelfTest(
        jsonBudgetIssue(
          successPayload(result, workflowResult, jsonBudgetIssueLimit),
          jsonBudgetIssueLimit,
        ) ?? fallbackJsonBudgetIssueResult,
        expectedJsonBudgetIssueCode,
      );
    };
    assertJsonBudgetIssueMutationSelfTest();
    const assertHelpIncludesSelfTests = () => {
      const expectedHelpIncludes = [
        "default",
        "--json",
        "--self-test",
        "--help",
        "context_anomaly_runbook_density_unknown_option",
        "P28_CONTEXT_ANOMALY_RUNBOOK_ROW_SOFT_MAX",
      ];

      for (const expected of expectedHelpIncludes) {
        assertHelpIncludes(expected);
      }
    };
    assertHelpIncludesSelfTests();
    const assertHelpLineWidthSelfTest = () => {
      const helpLineWidthFailureCode =
        "context_anomaly_runbook_density_self_test_help_failed";
      const maxHelpLineLengthLabel = "max_help_line_len";
      const actualHelpLineLengthLabel = "actual_help_line_len";
      const tooWideHelpLine = helpLines.find(
        (line) => line.length > maxHelpLineLength,
      );
      if (tooWideHelpLine) {
        console.error(helpLineWidthFailureCode);
        console.error(`${maxHelpLineLengthLabel}=${maxHelpLineLength}`);
        console.error(`${actualHelpLineLengthLabel}=${tooWideHelpLine.length}`);
        process.exit(1);
      }
    };
    assertHelpLineWidthSelfTest();
    const finishSelfTestOk = () => {
      const selfTestSuccessOutput =
        "context_anomaly_runbook_density_self_test_ok";

      console.log(selfTestSuccessOutput);
      process.exit(0);
    };
    finishSelfTestOk();
  };
  runContextAnomalyDensitySelfTests();
}

if (!result.ok) {
  failIssue(result);
}
if (!workflowResult.ok) {
  failIssue(workflowResult);
}

const {
  payload,
  jsonHeadroom,
  defaultOutputLine,
  defaultOutputHeadroom,
} = buildSuccessArtifacts(result, workflowResult, maxJsonBytes);
const defaultOutputWidthIssue = defaultOutputIssue(payload, result, jsonHeadroom);
if (defaultOutputWidthIssue) {
  failIssue(defaultOutputWidthIssue);
}
if (args.includes("--json")) {
  const jsonParityFailureCode =
    "context_anomaly_runbook_density_json_parity_failed";
  const jsonParityIssueDetailKey = "issue";
  const jsonPayloadError = jsonPayloadParityIssue(payload, result, {
    default_output_headroom: defaultOutputHeadroom,
    default_output_iterations: payload.default_output_iterations,
  });
  if (jsonPayloadError) {
    fail(jsonParityFailureCode, {
      [jsonParityIssueDetailKey]: jsonPayloadError,
    });
  }
  const issue = jsonBudgetIssue(payload, maxJsonBytes);
  if (issue) {
    failIssue(issue);
  }
  console.log(JSON.stringify(payload));
} else {
  console.log(defaultOutputLine);
}
