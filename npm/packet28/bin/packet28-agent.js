#!/usr/bin/env node
import { launchNative } from "./native-launcher.js";

await launchNative("packet28-agent", process.argv.slice(2));
