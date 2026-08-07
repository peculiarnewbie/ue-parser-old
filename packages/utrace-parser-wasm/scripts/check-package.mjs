import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  createParserManifest,
  readParserBuildConfig,
} from "./manifest.mjs";

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

const config = await readParserBuildConfig({ packageRoot, repositoryRoot });
const distDirectory = resolve(packageRoot, "dist");
const gluePath = resolve(distDirectory, "wasm", "utrace_parser_wasm.js");
const wasmPath = resolve(distDirectory, "wasm", "utrace_parser_wasm_bg.wasm");
const requiredPaths = [
  resolve(distDirectory, "index.js"),
  resolve(distDirectory, "index.d.ts"),
  resolve(distDirectory, "node.js"),
  resolve(distDirectory, "node.d.ts"),
  resolve(distDirectory, "cli.js"),
  resolve(distDirectory, "worker.js"),
  resolve(distDirectory, "worker.d.ts"),
  resolve(distDirectory, "worker-client.js"),
  resolve(distDirectory, "worker-operations.js"),
  resolve(distDirectory, "manifest.js"),
  resolve(distDirectory, "manifest.d.ts"),
  resolve(distDirectory, "parser-manifest.json"),
  gluePath,
  wasmPath,
];
for (const requiredPath of requiredPaths) {
  if (!existsSync(requiredPath)) throw new Error(`Missing package artifact: ${requiredPath}`);
}

const wasm = readFileSync(wasmPath);
if (wasm.length < 8 || wasm[0] !== 0 || wasm[1] !== 0x61 || wasm[2] !== 0x73 || wasm[3] !== 0x6d) {
  throw new Error("The generated UTrace parser artifact is not a WebAssembly binary");
}

const expectedManifest = createParserManifest({
  packageRoot,
  repositoryRoot,
  config,
});
const manifest = JSON.parse(readFileSync(resolve(distDirectory, "parser-manifest.json"), "utf8"));
assert.deepEqual(manifest, expectedManifest, "Generated parser manifest is stale");

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

const expectedExports = [".", "./manifest", "./node", "./parser-manifest.json", "./worker"];
assert.deepEqual(
  Object.keys(packageManifest.exports).sort(),
  expectedExports.sort(),
  "Package exports contain unexpected or missing public entries",
);
for (const exportName of expectedExports) {
  if (!(exportName in packageManifest.exports)) {
    throw new Error(`Package export is missing: ${exportName}`);
  }
}
if (packageManifest.bin?.["utrace-parser-wasm"] !== "dist/cli.js") {
  throw new Error("Package CLI bin entry is missing or incorrect");
}
