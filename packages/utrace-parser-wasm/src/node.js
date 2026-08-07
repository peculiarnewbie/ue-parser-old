import { readFile } from "node:fs/promises";

import initWasm from "./wasm/utrace_parser_wasm.js";
import {
  UtraceParserError,
  createProgressiveDashboard as createBrowserProgressiveDashboard,
  dashboard as browserDashboard,
  dashboardBundle as browserDashboardBundle,
  inspect as browserInspect,
  inventory as browserInventory,
} from "./index.js";

export * from "./index.js";

let initialization = null;

/** Instantiates the bundled WebAssembly bytes without using fetch(file://). */
export function init() {
  if (!initialization) {
    initialization = readFile(new URL("./wasm/utrace_parser_wasm_bg.wasm", import.meta.url))
      .then((bytes) => initWasm({ module_or_path: bytes }))
      .then(() => undefined)
      .catch((cause) => {
        initialization = null;
        throw new UtraceParserError(
          "Failed to initialize the UTrace parser WebAssembly module for Node.js",
          { cause },
        );
      });
  }
  return initialization;
}

export async function inspect(input) {
  await init();
  return browserInspect(input);
}

export async function inventory(input) {
  await init();
  return browserInventory(input);
}

export async function dashboard(input) {
  await init();
  return browserDashboard(input);
}

export async function dashboardBundle(input) {
  await init();
  return browserDashboardBundle(input);
}

export async function createProgressiveDashboard(input) {
  await init();
  return createBrowserProgressiveDashboard(input);
}
