import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { once } from "node:events";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const testDirectory = path.dirname(fileURLToPath(import.meta.url));
const launcherUrl = pathToFileURL(
  path.join(testDirectory, "..", "bin", "native-launcher.js"),
).href;

async function waitFor(predicate, timeoutMs = 5_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error(`condition was not met within ${timeoutMs}ms`);
}

function processExists(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}

test("piped stdin closes the native child when the launcher is force-killed", async () => {
  const directory = mkdtempSync(path.join(tmpdir(), "packet28-launcher-test-"));
  const childScript = path.join(directory, "child.mjs");
  const harnessScript = path.join(directory, "harness.mjs");
  const pidFile = path.join(directory, "child.pid");
  const eofFile = path.join(directory, "child.eof");
  let childPid;
  let harness;

  try {
    writeFileSync(
      childScript,
      `import { writeFileSync } from "node:fs";\n` +
        `const [, , pidFile, eofFile] = process.argv;\n` +
        `writeFileSync(pidFile, String(process.pid));\n` +
        `process.stdin.resume();\n` +
        `process.stdin.once("end", () => writeFileSync(eofFile, "eof"));\n`,
    );
    writeFileSync(
      harnessScript,
      `import { launchNative } from ${JSON.stringify(launcherUrl)};\n` +
        `await launchNative("Packet28", ${JSON.stringify([
          childScript,
          pidFile,
          eofFile,
        ])}, { overrideEnv: "PACKET28_TEST_BINARY", pipeStdin: true });\n`,
    );

    harness = spawn(process.execPath, [harnessScript], {
      env: { ...process.env, PACKET28_TEST_BINARY: process.execPath },
      stdio: ["pipe", "ignore", "inherit"],
    });
    await waitFor(() => existsSync(pidFile));
    childPid = Number.parseInt(readFileSync(pidFile, "utf8"), 10);
    assert.ok(processExists(childPid));

    const harnessExit = once(harness, "exit");
    harness.kill("SIGKILL");
    await harnessExit;

    await waitFor(() => existsSync(eofFile));
    await waitFor(() => !processExists(childPid));
  } finally {
    if (harness && harness.exitCode === null && harness.signalCode === null) {
      harness.kill("SIGKILL");
    }
    if (childPid && processExists(childPid)) {
      process.kill(childPid, "SIGKILL");
    }
    rmSync(directory, { recursive: true, force: true });
  }
});
