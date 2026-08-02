import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const packageRoot = resolve(scriptDirectory, "..");
const repositoryRoot = resolve(packageRoot, "..", "..");
const packageManifest = JSON.parse(readFileSync(resolve(packageRoot, "package.json"), "utf8"));
const cargoManifest = readFileSync(resolve(repositoryRoot, "Cargo.toml"), "utf8");
const cargoVersion = /^version\s*=\s*"([^"]+)"$/m.exec(cargoManifest)?.[1];

if (!cargoVersion) throw new Error("Unable to read the Cargo package version");
if (packageManifest.version !== cargoVersion) {
  throw new Error(
    `npm package version ${packageManifest.version} does not match Cargo version ${cargoVersion}`,
  );
}

const distDirectory = resolve(packageRoot, "dist");
const gluePath = resolve(distDirectory, "wasm", "utrace_parser_wasm.js");
const wasmPath = resolve(distDirectory, "wasm", "utrace_parser_wasm_bg.wasm");
for (const requiredPath of [resolve(distDirectory, "index.js"), resolve(distDirectory, "index.d.ts"), gluePath, wasmPath]) {
  if (!existsSync(requiredPath)) throw new Error(`Missing package artifact: ${requiredPath}`);
}

const wasm = readFileSync(wasmPath);
if (wasm.length < 8 || wasm[0] !== 0 || wasm[1] !== 0x61 || wasm[2] !== 0x73 || wasm[3] !== 0x6d) {
  throw new Error("The generated UTrace parser artifact is not a WebAssembly binary");
}

const glue = readFileSync(gluePath, "utf8");
for (const exportName of [
  "inspectUtrace",
  "inventoryUtrace",
  "dashboardUtrace",
  "dashboardBundleUtrace",
  "ProgressiveUtraceSession",
]) {
  if (!glue.includes(exportName)) throw new Error(`Missing generated UTrace export: ${exportName}`);
}
if (glue.includes("uasset-inspect") || glue.includes("export function parse(")) {
  throw new Error("The npm package unexpectedly includes the combined UAsset WASM API");
}
