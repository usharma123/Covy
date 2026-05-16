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

const args = process.argv.slice(2);
const unknownArgs = args.filter((arg) => !["--strict", "--help"].includes(arg));
if (unknownArgs.length > 0) {
  console.error("context_anomaly_hidden_sample_audit_unknown_option");
  console.error(`option=${unknownArgs[0]}`);
  process.exit(2);
}
if (args.includes("--help")) {
  console.log(
    [
      "Usage: node scripts/audit_context_anomaly_hidden_samples.mjs [--strict|--help]",
      "default: tolerant audit with verifier --max-high 2",
      "--strict: release-like audit with verifier --max-high 0",
      "--help: print this help",
    ].join("\n"),
  );
  process.exit(0);
}
const strictMode = args.includes("--strict");
const maxHigh = strictMode ? "0" : "2";

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
  (() => {
    try {
      return run("target/debug/Packet28", [
        "verify",
        "context-anomalies",
        "--root",
        ".",
        "--max-high",
        maxHigh,
        "--json",
      ], { env: packetEnv });
    } catch (error) {
      const output = (error.stdout ?? "").trim();
      if (output) {
        return output;
      }
      throw error;
    }
  })(),
);
if (!verifier.ok) {
  if (strictMode) {
    console.error("context_anomaly_hidden_sample_audit_strict_failed");
    console.error("audit_mode=strict");
    console.error(`high=${verifier.high_count}`);
    console.error(`max_high=${verifier.max_high}`);
    process.exit(1);
  }
  console.error("context_anomaly_hidden_sample_audit_verifier_failed");
  console.error("audit_mode=tolerant");
  console.error(`high=${verifier.high_count}`);
  console.error(`max_high=${verifier.max_high}`);
  process.exit(1);
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
console.log(
  `audit_mode=${strictMode ? "strict" : "tolerant"} verifier=ok high=${verifier.high_count} max_high=${maxHigh}`,
);
console.log(`digest_anomalies=${digest.anomaly_count}`);
