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

console.log(`context_anomaly_hidden_sample_fixture_ok=${actual}`);
