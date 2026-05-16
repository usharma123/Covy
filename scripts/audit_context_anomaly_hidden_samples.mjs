#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

function run(command, args, options = {}) {
  return execFileSync(command, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  }).trim();
}

function expectFailure(command, args, expected, options = {}) {
  try {
    run(command, args, options);
  } catch (error) {
    const output = `${error.stdout ?? ""}${error.stderr ?? ""}`;
    if (output.includes(expected)) {
      return;
    }
    throw new Error(`expected failure containing ${expected}`);
  }
  throw new Error(`expected ${command} ${args.join(" ")} to fail`);
}

const packetEnv = {
  ...process.env,
  HOME: mkdtempSync(join(tmpdir(), "p28-context-anomaly-audit-")),
};

const smoke = run("node", ["scripts/check_context_anomaly_hidden_samples.mjs"]);
if (!smoke.startsWith("context_anomaly_hidden_sample_fixture_ok=")) {
  throw new Error("default smoke output drifted");
}

const smokeJson = JSON.parse(
  run("node", ["scripts/check_context_anomaly_hidden_samples.mjs", "--json"]),
);
if (!smokeJson.ok || smokeJson.actual_len > smokeJson.max_len) {
  throw new Error("smoke JSON budget drifted");
}

const selfTest = run("node", [
  "scripts/check_context_anomaly_hidden_samples.mjs",
  "--self-test",
]);
if (selfTest !== "context_anomaly_hidden_sample_self_test_ok") {
  throw new Error("self-test output drifted");
}

const helpLines = run("node", [
  "scripts/check_context_anomaly_hidden_samples.mjs",
  "--help",
])
  .split("\n")
  .filter(Boolean).length;
if (helpLines > 8) {
  throw new Error("help output grew beyond compact budget");
}

expectFailure(
  "node",
  ["scripts/check_context_anomaly_hidden_samples.mjs", "--bad-flag"],
  "context_anomaly_hidden_sample_unknown_option",
);
expectFailure(
  "node",
  ["scripts/check_context_anomaly_hidden_samples.mjs"],
  "context_anomaly_hidden_sample_fixture_too_long",
  {
    env: { ...process.env, P28_HIDDEN_SAMPLE_SUMMARY_MAX: "10" },
  },
);

const verifier = JSON.parse(
  run("target/debug/Packet28", [
    "verify",
    "context-anomalies",
    "--root",
    ".",
    "--max-high",
    "2",
    "--json",
  ], { env: packetEnv }),
);
if (!verifier.ok) {
  throw new Error("context anomaly verifier did not pass");
}

const dashboard = JSON.parse(
  run("target/debug/Packet28", [
    "dashboard",
    "--root",
    ".",
    "--context-anomaly-history",
    "docs/context-anomalies/history.jsonl",
    "--json",
  ], { env: packetEnv }),
);
const contextTile = dashboard.context_anomalies ?? {};
if (
  contextTile.latest_status !== "ready" ||
  !(contextTile.recurring_hidden_categories ?? []).includes("fallback_provenance")
) {
  throw new Error("fixture dashboard replay drifted");
}

const digest = JSON.parse(
  run("target/debug/Packet28", ["digest", "--root", ".", "--json"], {
    env: packetEnv,
  }),
);

console.log("context_anomaly_hidden_sample_audit_ok");
console.log("smoke_modes=default,json,self-test,help,budget-fail,bad-flag");
console.log(`formatter_budget=${smokeJson.actual_len}/${smokeJson.max_len}`);
console.log(`formatter_checksum=${smokeJson.checksum}`);
console.log(
  `fixture_dashboard=${contextTile.latest_status} recurring_hidden=${contextTile.recurring_hidden_categories.join(",")}`,
);
console.log(`verifier=ok high=${verifier.high_count}`);
console.log(`digest_anomalies=${digest.anomaly_count}`);
