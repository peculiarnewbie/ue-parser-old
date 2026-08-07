#!/usr/bin/env node

import { lstat, mkdir, open, readFile, rename, rm } from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";
import { randomUUID } from "node:crypto";

import { dashboard, parserManifest } from "./node.js";

const ExitCode = Object.freeze({ usage: 2, input: 3, parse: 4, output: 5 });

await main().catch((cause) => {
  emitStatus({ type: "error", stage: "internal", message: errorMessage(cause) });
  process.exitCode = 1;
});

async function main() {
  let command;
  try {
    command = parseArguments(process.argv.slice(2));
  } catch (cause) {
    emitStatus({ type: "error", stage: "usage", message: errorMessage(cause) });
    process.exitCode = ExitCode.usage;
    return;
  }

  let bytes;
  try {
    bytes = await readFile(command.input);
  } catch (cause) {
    emitStatus({ type: "error", stage: "input", message: errorMessage(cause) });
    process.exitCode = ExitCode.input;
    return;
  }

  let result;
  try {
    result = await dashboard({
      bytes,
      filename: command.input,
      options: command.maxFrames === undefined ? undefined : { maxFrames: command.maxFrames },
    });
    validateDashboard(result);
  } catch (cause) {
    emitStatus({ type: "error", stage: "parse", message: errorMessage(cause) });
    process.exitCode = ExitCode.parse;
    return;
  }

  let json;
  try {
    json = JSON.stringify(result);
    await writeAtomically(command.output, json);
  } catch (cause) {
    emitStatus({ type: "error", stage: "output", message: errorMessage(cause) });
    process.exitCode = ExitCode.output;
    return;
  }

  emitStatus({
    type: "success",
    operation: "dashboard",
    schema_version: result.schema_version,
    package_version: parserManifest.packageVersion,
    output: command.output,
    bytes: Buffer.byteLength(json),
  });
}

function parseArguments(arguments_) {
  if (arguments_[0] !== "dashboard") {
    throw new Error("Usage: utrace-parser-wasm dashboard --input <trace.utrace> --output <dashboard.json> [--max-frames <count>]");
  }
  const values = new Map();
  for (let index = 1; index < arguments_.length; index += 2) {
    const flag = arguments_[index];
    const value = arguments_[index + 1];
    if (!flag?.startsWith("--") || value === undefined) {
      throw new Error(`Missing value for ${flag ?? "argument"}`);
    }
    if (values.has(flag)) throw new Error(`Duplicate option: ${flag}`);
    if (flag !== "--input" && flag !== "--output" && flag !== "--max-frames") {
      throw new Error(`Unknown option: ${flag}`);
    }
    values.set(flag, value);
  }
  const input = values.get("--input");
  const output = values.get("--output");
  if (!input || !output) throw new Error("Both --input and --output are required");
  const maxFramesText = values.get("--max-frames");
  const maxFrames = maxFramesText === undefined ? undefined : Number(maxFramesText);
  if (maxFrames !== undefined && (!Number.isSafeInteger(maxFrames) || maxFrames < 0)) {
    throw new Error("--max-frames must be an unsigned safe integer");
  }
  return { input: resolve(input), output: resolve(output), maxFrames };
}

function validateDashboard(result) {
  if (
    result === null ||
    typeof result !== "object" ||
    result.schema_version !== 2 ||
    result.status !== "ok" ||
    result.dashboard === null ||
    typeof result.dashboard !== "object" ||
    Array.isArray(result.dashboard)
  ) {
    throw new Error("The parser returned an incompatible dashboard envelope");
  }
}

async function writeAtomically(outputPath, contents) {
  const outputDirectory = dirname(outputPath);
  await mkdir(outputDirectory, { recursive: true });
  try {
    await lstat(outputPath);
    throw new Error(`Refusing to overwrite existing output: ${outputPath}`);
  } catch (cause) {
    if (cause?.code !== "ENOENT") throw cause;
  }

  const temporaryPath = resolve(
    outputDirectory,
    `.${basename(outputPath)}.${process.pid}.${randomUUID()}.tmp`,
  );
  let temporaryCreated = false;
  try {
    const handle = await open(temporaryPath, "wx");
    temporaryCreated = true;
    try {
      await handle.writeFile(contents, "utf8");
      await handle.sync();
    } finally {
      await handle.close();
    }
    await rename(temporaryPath, outputPath);
    temporaryCreated = false;
  } finally {
    if (temporaryCreated) await rm(temporaryPath, { force: true });
  }
}

function emitStatus(status) {
  process.stderr.write(`${JSON.stringify(status)}\n`);
}

function errorMessage(cause) {
  return cause instanceof Error ? cause.message : String(cause);
}
