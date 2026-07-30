import { execFileSync, spawn } from "node:child_process";
import { chmodSync, existsSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const binDirectory = path.dirname(fileURLToPath(import.meta.url));
const platformPackages = {
  "darwin-arm64": "@packet28/darwin-arm64",
  "darwin-x64": "@packet28/darwin-x64",
  "linux-arm64": "@packet28/linux-arm64",
  "linux-x64": "@packet28/linux-x64",
};

function platformKey() {
  const cpu = { arm64: "arm64", x64: "x64" }[process.arch];
  const os = { darwin: "darwin", linux: "linux" }[process.platform];
  return cpu && os ? `${os}-${cpu}` : null;
}

function ensureExecutable(candidate) {
  if (!existsSync(candidate)) return false;
  try {
    const mode = statSync(candidate).mode;
    if (!(mode & 0o111)) chmodSync(candidate, mode | 0o755);
    return Boolean(statSync(candidate).mode & 0o111);
  } catch {
    return false;
  }
}

function resolveNative(name, { overrideEnv, searchPath }) {
  const override = overrideEnv && process.env[overrideEnv];
  if (override) {
    if (ensureExecutable(override)) return override;
    throw new Error(`${overrideEnv} is not executable: ${override}`);
  }

  const key = platformKey();
  if (!key || !platformPackages[key]) {
    throw new Error(
      `Unsupported platform: ${process.platform} (${process.arch}). ` +
        `Packet28 supports: ${Object.keys(platformPackages).join(", ")}.`,
    );
  }

  try {
    const manifest = require.resolve(`${platformPackages[key]}/package.json`);
    const candidate = path.join(path.dirname(manifest), "bin", name);
    if (ensureExecutable(candidate)) return candidate;
  } catch {
    // Optional platform package is absent; continue to local fallbacks.
  }

  if (searchPath) {
    try {
      const candidate = execFileSync("which", [name], {
        encoding: "utf-8",
      }).trim();
      if (candidate && ensureExecutable(candidate)) return candidate;
    } catch {
      // The binary is not on PATH; continue to the vendored fallback.
    }
  }

  const vendored = path.join(binDirectory, "..", "vendor", key, name);
  if (ensureExecutable(vendored)) return vendored;
  throw new Error(
    `Could not find executable ${name}. Reinstall: npm install -g packet28@latest`,
  );
}

export async function launchNative(
  name,
  args,
  { label = name, overrideEnv, searchPath = true } = {},
) {
  let binary;
  try {
    binary = resolveNative(name, { overrideEnv, searchPath });
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }

  if (name === "Packet28") {
    ensureExecutable(path.join(path.dirname(binary), "packet28d"));
  }

  const child = spawn(binary, args, {
    stdio: "inherit",
    env: { ...process.env, PACKET28_MANAGED_BY_NPM: "1" },
  });
  const forwarders = new Map();
  for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
    const forward = () => {
      if (!child.killed) {
        try {
          child.kill(signal);
        } catch {
          // The child may have exited between the check and signal.
        }
      }
    };
    forwarders.set(signal, forward);
    process.on(signal, forward);
  }

  const result = await new Promise((resolve) => {
    child.once("error", (error) => resolve({ error }));
    child.once("exit", (code, signal) =>
      resolve(signal ? { signal } : { code: code ?? 1 }),
    );
  });
  for (const [signal, forward] of forwarders) {
    process.off(signal, forward);
  }
  if (result.error) {
    console.error(`Failed to start ${label}: ${result.error.message}`);
    process.exit(1);
  }
  if (result.signal) {
    process.kill(process.pid, result.signal);
  }
  process.exit(result.code);
}
