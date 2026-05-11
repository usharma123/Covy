#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const [outDir, sourceRoot, repoDir, taskPattern] = process.argv.slice(2);
if (!outDir || !sourceRoot || !repoDir || !taskPattern) {
  console.error("usage: summarize_packet28_experiment.mjs <outDir> <sourceRoot> <repoDir> <taskPattern>");
  process.exit(2);
}

function readText(file) {
  return fs.existsSync(file) ? fs.readFileSync(file, "utf8") : "";
}

function parseMetrics() {
  const lines = readText(path.join(outDir, "metrics.tsv")).trim().split(/\n/);
  const header = lines.shift().split("\t");
  return lines.map((line) => {
    const cols = line.split("\t");
    return Object.fromEntries(header.map((key, idx) => [key, cols[idx] ?? ""]));
  });
}

function jsonFile(name) {
  try {
    return JSON.parse(readText(path.join(outDir, name)));
  } catch {
    return null;
  }
}

const metrics = parseMetrics();
const byName = Object.fromEntries(metrics.map((row) => [row.name, row]));
const nativeTokens = Number(byName.native_rg?.est_stdout_tokens ?? 0);
const packetRun = jsonFile("packet28_run_rg.out");
const packetRaw = Number(packetRun?.raw_est_tokens ?? 0);
const packetReduced = Number(packetRun?.reduced_est_tokens ?? 0);
const p28CompactTokens = Number(byName.p28_compact?.est_stdout_tokens ?? 0);
const p28JsonTokens = Number(byName.p28_json?.est_stdout_tokens ?? 0);
const nativeControlTokens = Number(byName.native_control_rg?.est_stdout_tokens ?? 0);
const indexedControlTokens = Number(byName.p28_indexed_control?.est_stdout_tokens ?? 0);
const compactSavings = nativeTokens
  ? ((1 - p28CompactTokens / nativeTokens) * 100)
  : 0;
const runSavings = packetRaw
  ? ((1 - packetReduced / packetRaw) * 100)
  : 0;

const indexStatus = jsonFile("daemon-index-status.json");
const finalStatus = jsonFile("daemon-status-final.json");
const p28CompactErr = readText(path.join(outDir, "p28_compact.err")).trim();
const p28IndexedControlErr = readText(path.join(outDir, "p28_indexed_control.err")).trim();
const p28Backend = p28CompactErr.match(/backend=([^\s]+)/)?.[1] ?? "unknown";
const p28Fallback = p28CompactErr.match(/fallback_reason=(.*)/)?.[1] ?? "";
const p28IndexedControlBackend = p28IndexedControlErr.match(/backend=([^\s]+)/)?.[1] ?? "not run";
const indexedControlSavings = nativeControlTokens
  ? ((1 - indexedControlTokens / nativeControlTokens) * 100)
  : 0;
const version = readText(path.join(outDir, "packet28-version.txt")).trim();
const fileCount = readText(path.join(outDir, "file-count.txt")).trim();
const tsFileCount = readText(path.join(outDir, "ts-file-count.txt")).trim();
const repoSize = readText(path.join(outDir, "repo-size.txt")).trim();

const report = `# Packet28 Claude Code Main Experiment

## Task

Use Packet28 on \`claude-code-main\` for a real code-navigation task, compare it with native shell tools, and record savings.

Task query:

\`\`\`text
Find hook, MCP, and tool-use plumbing in claude-code-main.
Search pattern: ${taskPattern}
\`\`\`

## Setup

- Source repo: \`${sourceRoot}\`
- Isolated experiment repo: \`${repoDir}\`
- Artifact directory: \`${outDir}\`
- Packet28 version: \`${version}\`
- Repo size: \`${repoSize}\`
- Files copied: \`${fileCount}\`
- TypeScript files: \`${tsFileCount}\`
- Daemon index ready: \`${Boolean(indexStatus?.ready)}\`
- Final daemon ready: \`${Boolean(finalStatus?.pid)}\`
- p28 backend: \`${p28Backend}\`
- p28 fallback reason: \`${p28Fallback || "none"}\`
- p28 indexed control backend: \`${p28IndexedControlBackend}\`

The isolated repo exists because Packet28 daemon commands resolve to the nearest git root. The source directory is inside a larger \`Buns\` git workspace, so the experiment copies only \`claude-code-main\` into a temporary git root and runs both native and Packet28 commands against that same copy.

## Commands

| Run | Command | Exit | Duration ms | Stdout bytes | Est stdout tokens |
|---|---:|---:|---:|---:|---:|
${metrics.map((row) => `| \`${row.name}\` | \`${row.command.replaceAll("|", "\\|")}\` | ${row.status} | ${row.duration_ms} | ${row.stdout_bytes} | ${row.est_stdout_tokens} |`).join("\n")}

## Savings

- Native broad search stdout estimate: ${nativeTokens.toLocaleString()} tokens.
- Packet28 reducer raw estimate for the same \`rg\` command: ${packetRaw.toLocaleString()} tokens.
- Packet28 reducer returned estimate: ${packetReduced.toLocaleString()} tokens.
- Packet28 reducer savings: ${runSavings.toFixed(2)}%.
- \`p28 --compact --stats\` stdout estimate: ${p28CompactTokens.toLocaleString()} tokens.
- \`p28 --compact\` savings against native broad search stdout: ${compactSavings.toFixed(2)}%.
- \`p28 --json --max-total-matches 50\` stdout estimate: ${p28JsonTokens.toLocaleString()} tokens.
- Native literal \`tool_use\` control estimate: ${nativeControlTokens.toLocaleString()} tokens.
- Indexed \`p28 tool_use\` compact estimate: ${indexedControlTokens.toLocaleString()} tokens.
- Indexed \`p28 tool_use\` savings against native literal search: ${indexedControlSavings.toFixed(2)}%.

## Monitoring Evidence

- \`daemon-start.out\`, \`daemon-index-rebuild.out\`, \`daemon-index-status.json\`, and \`daemon-status-final.json\` capture daemon/index state.
- \`index-poll.tsv\` records readiness, repo index progress, and regex index progress while the daemon builds.
- \`*.status\` files capture exit code, bytes, token estimate, duration, and command for each run.
- \`*.out\` and \`*.err\` files contain raw command artifacts.

## Notes

- \`Packet28 run --json\` reduced the broad \`rg\` command to metadata and a compact summary. In this run its \`reduced_est_tokens\` field is \`${packetReduced}\`, while the summary text is present inside the JSON payload.
- \`p28 --compact --stats\` is the clearest human-facing savings comparison for search output: it reports a compact summary instead of returning all matching lines.
`;

fs.writeFileSync(path.join(outDir, "summary.md"), report);
console.log(report);
