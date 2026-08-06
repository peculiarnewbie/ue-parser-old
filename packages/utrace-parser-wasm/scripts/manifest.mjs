import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";

export async function readParserBuildConfig({ packageRoot, repositoryRoot }) {
  const packageManifest = JSON.parse(
    readFileSync(resolve(packageRoot, "package.json"), "utf8"),
  );
  const wasmSource = readFileSync(resolve(repositoryRoot, "src", "wasm.rs"), "utf8");
  const indexSource = readFileSync(resolve(packageRoot, "src", "index.js"), "utf8");
  const schemaVersion = readDecimalConstant(
    wasmSource,
    /const\s+UTRACE_SCHEMA_VERSION\s*:\s*u32\s*=\s*(\d+)\s*;/,
    "Rust UTRACE_SCHEMA_VERSION",
  );
  const indexSchemaVersion = readDecimalConstant(
    indexSource,
    /export\s+const\s+UTRACE_SCHEMA_VERSION\s*=\s*(\d+)\s*;/,
    "JavaScript UTRACE_SCHEMA_VERSION",
  );
  if (schemaVersion !== indexSchemaVersion) {
    throw new Error(
      `Rust UTrace schema ${schemaVersion} does not match JavaScript schema ${indexSchemaVersion}`,
    );
  }

  const maxInputBytes = evaluateRustIntegerExpression(
    readRustConstantExpression(
      readFileSync(resolve(repositoryRoot, "src", "utrace_session.rs"), "utf8"),
      /pub\(crate\)\s+const\s+MAX_INPUT_BYTES\s*:\s*usize\s*=\s*([^;]+);/,
      "Rust MAX_INPUT_BYTES",
    ),
  );
  const operationsModule = await import(
    pathToFileURL(resolve(packageRoot, "src", "worker-operations.js")).href,
  );
  const supportedOperations = [...operationsModule.UTRACE_WORKER_OPERATIONS];
  if (
    supportedOperations.length === 0 ||
    supportedOperations.some(
      (operation) => typeof operation !== "string" || operation.length === 0,
    ) ||
    new Set(supportedOperations).size !== supportedOperations.length
  ) {
    throw new Error("The UTrace Worker operation list must contain unique non-empty strings");
  }

  return {
    packageVersion: packageManifest.version,
    utraceSchemaVersion: schemaVersion,
    supportedOperations,
    maxInputBytes,
  };
}

export function createParserManifest({ packageRoot, repositoryRoot, config }) {
  const wasmPath = resolve(
    packageRoot,
    "dist",
    "wasm",
    "utrace_parser_wasm_bg.wasm",
  );
  const wasmSha256 = createHash("sha256")
    .update(readFileSync(wasmPath))
    .digest("hex");
  return {
    packageVersion: config.packageVersion,
    sourceCommit: readSourceCommit(repositoryRoot),
    wasmSha256,
    utraceSchemaVersion: config.utraceSchemaVersion,
    supportedOperations: config.supportedOperations,
    maxInputBytes: config.maxInputBytes,
  };
}

export function writeParserManifest({ packageRoot, manifest }) {
  const distDirectory = resolve(packageRoot, "dist");
  writeFileSync(
    resolve(distDirectory, "parser-manifest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );

  const manifestProperties = Object.entries(manifest)
    .map(([key, value]) => {
      if (key === "supportedOperations") {
        return `  ${key}: supportedOperations`;
      }
      return `  ${key}: ${JSON.stringify(value)}`;
    })
    .join(",\n");
  writeFileSync(
    resolve(distDirectory, "manifest.js"),
    `const supportedOperations = Object.freeze(${JSON.stringify(manifest.supportedOperations)});\n\n` +
      `export const parserManifest = Object.freeze({\n${manifestProperties}\n});\n\n` +
      `export default parserManifest;\n`,
  );

  const operationUnion = manifest.supportedOperations.map(JSON.stringify).join(" | ");
  const operationTuple = manifest.supportedOperations.map(JSON.stringify).join(", ");
  writeFileSync(
    resolve(distDirectory, "manifest.d.ts"),
    `export type UtraceParserOperation = ${operationUnion};\n\n` +
      `export type UtraceParserManifest = Readonly<{\n` +
      `  packageVersion: string;\n` +
      `  sourceCommit: string;\n` +
      `  wasmSha256: string;\n` +
      `  utraceSchemaVersion: ${manifest.utraceSchemaVersion};\n` +
      `  supportedOperations: readonly [${operationTuple}];\n` +
      `  maxInputBytes: ${manifest.maxInputBytes};\n` +
      `}>;\n\n` +
      `export const parserManifest: UtraceParserManifest;\n` +
      `export default parserManifest;\n`,
  );
}

function readDecimalConstant(source, pattern, label) {
  const match = source.match(pattern);
  if (!match) throw new Error(`Unable to read ${label}`);
  const value = Number(match[1]);
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`${label} is not a safe unsigned integer`);
  }
  return value;
}

function readRustConstantExpression(source, pattern, label) {
  const match = source.match(pattern);
  if (!match) throw new Error(`Unable to read ${label}`);
  return match[1];
}

function evaluateRustIntegerExpression(expression) {
  const normalized = expression.replaceAll(/\s|_/g, "");
  if (!/^\d+(?:\*\d+)*$/.test(normalized)) {
    throw new Error(`Unsupported integer expression in parser configuration: ${expression}`);
  }
  return normalized.split("*").reduce((total, term) => {
    const value = Number(term);
    const result = total * value;
    if (!Number.isSafeInteger(result)) {
      throw new Error(`Parser configuration exceeds JavaScript's safe integer range: ${expression}`);
    }
    return result;
  }, 1);
}

function readSourceCommit(repositoryRoot) {
  const result = spawnSync("git", ["rev-parse", "HEAD"], {
    cwd: repositoryRoot,
    encoding: "utf8",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`Unable to determine source commit: ${result.stderr.trim()}`);
  }
  const commit = result.stdout.trim();
  if (!/^[0-9a-f]{40}$/i.test(commit)) {
    throw new Error(`Git returned an invalid source commit: ${commit}`);
  }
  return commit;
}
