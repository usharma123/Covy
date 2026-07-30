#!/usr/bin/env node
import { launchNative } from "./native-launcher.js";

function serveArgs(args) {
  if (args.includes("--toolset")) return args;
  return [...args, "--toolset", process.env.PACKET28_MCP_TOOLSET || "core"];
}

await launchNative(
  "Packet28",
  ["mcp", "serve", ...serveArgs(process.argv.slice(2))],
  { label: "Packet28 MCP server", searchPath: false },
);
