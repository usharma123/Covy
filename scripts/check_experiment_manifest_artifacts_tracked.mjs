#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const manifestPath = path.join(root, "docs/experiments/manifest.json");
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
const failures = [];

for (const experiment of manifest.experiments ?? []) {
  for (const artifact of experiment.artifacts ?? []) {
    const trimmed = String(artifact).trim();
    if (!trimmed) {
      continue;
    }
    const absolute = path.join(root, trimmed);
    if (!fs.existsSync(absolute)) {
      failures.push(`${experiment.id ?? "<missing-id>"}: missing ${trimmed}`);
      continue;
    }
    if (process.env.CI === "true") {
      const tracked = spawnSync("git", ["ls-files", "--error-unmatch", "--", trimmed], {
        cwd: root,
        encoding: "utf8",
      });
      if (tracked.status !== 0) {
        failures.push(`${experiment.id ?? "<missing-id>"}: untracked ${trimmed}`);
        continue;
      }
    }
    const result = spawnSync("git", ["check-ignore", "-q", "--", trimmed], {
      cwd: root,
      encoding: "utf8",
    });
    if (result.status === 0) {
      failures.push(`${experiment.id ?? "<missing-id>"}: ignored ${trimmed}`);
    }
  }
}

if (failures.length > 0) {
  console.error("Experiment manifest references artifacts that are missing or ignored:");
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log("Experiment manifest artifacts are present and not ignored.");
