#!/usr/bin/env node
import { launchNative } from "./native-launcher.js";

await launchNative("p28", process.argv.slice(2));
