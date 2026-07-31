#!/usr/bin/env node
import { launchNative } from "./native-launcher.js";

await launchNative("Packet28", process.argv.slice(2), {
  overrideEnv: "PACKET28_BINARY",
});
