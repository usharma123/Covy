#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const fixturePath = join(
  repoRoot,
  "docs/context-anomalies/hidden-samples-delimiters.json",
);
const expectedPath = join(
  repoRoot,
  "docs/context-anomalies/hidden-samples-delimiters.summary",
);
const maxSummaryLength = Number.parseInt(
  process.env.P28_HIDDEN_SAMPLE_SUMMARY_MAX ?? "256",
  10,
);
const args = process.argv.slice(2);
const unknownArgs = args.filter(
  (arg) => !["--json", "--self-test", "--help"].includes(arg),
);
if (unknownArgs.length > 0) {
  console.error("context_anomaly_hidden_sample_unknown_option");
  console.error(`option=${unknownArgs[0]}`);
  process.exit(2);
}
if (args.includes("--help")) {
  console.log(
    [
      "Usage: node scripts/check_context_anomaly_hidden_samples.mjs [--json|--self-test|--help]",
      "default: validate fixture and print context_anomaly_hidden_sample_fixture_ok=...",
      "--json: print ok, actual_len, max_len, and summary as JSON",
      "--self-test: validate fixture, JSON payload, and budget checks",
      "--help: print this help",
    ].join("\n"),
  );
  process.exit(0);
}
const jsonOutput = args.includes("--json");
const selfTest = args.includes("--self-test");

function escapeSegment(value, escapeEquals) {
  let escaped = value
    .replaceAll("%", "%25")
    .replaceAll("\n", "%0A")
    .replaceAll(";", "%3B");
  if (escapeEquals) {
    escaped = escaped.replaceAll("=", "%3D");
  }
  return escaped;
}

function summarize(samples) {
  return samples
    .map(
      (sample) =>
        `${escapeSegment(sample.category ?? "", true)}=${escapeSegment(
          sample.signal ?? "",
          false,
        )}`,
    )
    .join(";");
}

const samples = JSON.parse(readFileSync(fixturePath, "utf8"));
const actual = summarize(samples);
const expected = readFileSync(expectedPath, "utf8").trim();

function jsonPayload(summary, maxLength) {
  return {
    ok: true,
    actual_len: summary.length,
    max_len: maxLength,
    summary,
  };
}

if (selfTest) {
  const payload = jsonPayload(actual, maxSummaryLength);
  const lowBudget = actual.length - 1;
  if (actual !== expected) {
    console.error("context_anomaly_hidden_sample_self_test_mismatch");
    process.exit(1);
  }
  if (
    payload.actual_len !== actual.length ||
    payload.max_len !== maxSummaryLength ||
    payload.summary !== actual
  ) {
    console.error("context_anomaly_hidden_sample_self_test_json_drift");
    process.exit(1);
  }
  if (actual.length <= lowBudget) {
    console.error("context_anomaly_hidden_sample_self_test_budget_drift");
    process.exit(1);
  }
  console.log("context_anomaly_hidden_sample_self_test_ok");
  process.exit(0);
}

if (actual !== expected) {
  console.error("context_anomaly_hidden_sample_fixture_mismatch");
  console.error(`expected=${expected}`);
  console.error(`actual=${actual}`);
  process.exit(1);
}

if (actual.length > maxSummaryLength) {
  console.error("context_anomaly_hidden_sample_fixture_too_long");
  console.error(`max=${maxSummaryLength}`);
  console.error(`actual_len=${actual.length}`);
  process.exit(1);
}

if (jsonOutput) {
  console.log(JSON.stringify(jsonPayload(actual, maxSummaryLength)));
} else {
  console.log(`context_anomaly_hidden_sample_fixture_ok=${actual}`);
}
