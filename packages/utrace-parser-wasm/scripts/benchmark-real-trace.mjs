import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { performance } from "node:perf_hooks";

import initWasm, {
  ProgressiveUtraceSession,
  dashboardUtrace,
} from "../dist/wasm/utrace_parser_wasm.js";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const packageRoot = resolve(scriptDirectory, "..");
const repositoryRoot = resolve(packageRoot, "..", "..");
const arguments_ = parseArguments(process.argv.slice(2));
const inputPath = arguments_.input ?? process.env.UTRACE_BENCH_TRACE ?? resolve(
  repositoryRoot,
  "..",
  "tools",
  ".local",
  "traces",
  "funguys-build-30229-dev-20260807.utrace",
);

if (!existsSync(inputPath)) {
  process.stdout.write(`${JSON.stringify({
    status: "skipped",
    reason: "real trace fixture is absent",
    input: inputPath,
    runtime: runtimeName(),
  })}\n`);
  process.exit(0);
}

const readStart = performance.now();
const fileBytes = await readFile(inputPath);
const fileReadMs = performance.now() - readStart;

const copyStart = performance.now();
const inputBytes = Uint8Array.from(fileBytes);
const inputCopyMs = performance.now() - copyStart;

const wasmBytes = readFileSync(resolve(packageRoot, "dist", "wasm", "utrace_parser_wasm_bg.wasm"));
const initializationStart = performance.now();
const wasm = await initWasm({ module_or_path: wasmBytes });
const wasmInitializationMs = performance.now() - initializationStart;

const optionsJson = JSON.stringify({ max_frames: arguments_.maxFrames });
const parseStart = performance.now();
const outputJson = dashboardUtrace(inputPath, inputBytes, optionsJson);
const wasmParseAndSerializationMs = performance.now() - parseStart;

const jsonParseStart = performance.now();
const output = JSON.parse(outputJson);
const jsonParseMs = performance.now() - jsonParseStart;
const dashboardJson = JSON.stringify(output.dashboard);
const outputBytes = Buffer.byteLength(dashboardJson);
const outputSha256 = createHash("sha256").update(dashboardJson).digest("hex");

const report = {
  status: "ok",
  runtime: runtimeName(),
  input: inputPath,
  input_bytes: inputBytes.byteLength,
  max_frames: arguments_.maxFrames,
  timings_ms: {
    file_read: rounded(fileReadMs),
    input_copy: rounded(inputCopyMs),
    wasm_initialization: rounded(wasmInitializationMs),
    wasm_parse_and_serialization: rounded(wasmParseAndSerializationMs),
    json_parse: rounded(jsonParseMs),
  },
  wasm_linear_memory_bytes: wasm.memory.buffer.byteLength,
  process_rss_bytes: process.memoryUsage().rss,
  schema_version: output.schema_version,
  dashboard_families: Object.keys(output.dashboard).length,
  output_bytes: outputBytes,
  output_sha256: outputSha256,
  envelope_bytes: Buffer.byteLength(outputJson),
};

if (arguments_.profileStages) {
  report.progressive_stages = profileProgressiveStages({
    inputPath,
    inputBytes,
    optionsJson,
  });
  report.wasm_linear_memory_bytes_after_progressive = wasm.memory.buffer.byteLength;
  report.process_rss_bytes_after_progressive = process.memoryUsage().rss;
}

if (arguments_.nativeCli) {
  report.native_cli = benchmarkNative({
    executable: arguments_.nativeCli,
    inputPath,
    maxFrames: arguments_.maxFrames,
  });
}

process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);

function profileProgressiveStages({ inputPath: filename, inputBytes: bytes, optionsJson: options }) {
  const session = new ProgressiveUtraceSession(filename, bytes.byteLength, options);
  const chunkBytes = 1024 * 1024;
  const pushStart = performance.now();
  for (let offset = 0; offset < bytes.byteLength; offset += chunkBytes) {
    session.push_chunk(bytes.subarray(offset, Math.min(offset + chunkBytes, bytes.byteLength)));
  }
  const pushMs = performance.now() - pushStart;
  const finishStart = performance.now();
  const completed = session.finish();
  const finishMs = performance.now() - finishStart;
  session.free();
  return {
    chunk_bytes: chunkBytes,
    ingest_and_eager_timeline_ms: rounded(pushMs),
    finish_and_serialization_ms: rounded(finishMs),
    completion_bytes: Buffer.byteLength(completed),
  };
}

function benchmarkNative({ executable, inputPath: input, maxFrames }) {
  const start = performance.now();
  const result = spawnSync(
    executable,
    ["utrace", "dashboard", input, "--format", "json", "--max-frames", String(maxFrames)],
    { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  const elapsedMs = performance.now() - start;
  if (result.error) return { status: "error", message: result.error.message };
  if (result.status !== 0) {
    return { status: "error", exit_code: result.status, message: result.stderr.trim() };
  }
  const json = result.stdout.trimEnd();
  const dashboardJson = JSON.stringify(JSON.parse(json).dashboard);
  return {
    status: "ok",
    total_ms: rounded(elapsedMs),
    output_bytes: Buffer.byteLength(dashboardJson),
    output_sha256: createHash("sha256").update(dashboardJson).digest("hex"),
    envelope_bytes: Buffer.byteLength(json),
  };
}

function parseArguments(values) {
  const parsed = { maxFrames: 120, profileStages: false };
  for (let index = 0; index < values.length; index += 1) {
    const flag = values[index];
    if (flag === "--profile-stages") {
      parsed.profileStages = true;
      continue;
    }
    const value = values[index + 1];
    if (value === undefined) throw new Error(`Missing value for ${flag}`);
    index += 1;
    if (flag === "--input") parsed.input = resolve(value);
    else if (flag === "--native-cli") parsed.nativeCli = resolve(value);
    else if (flag === "--max-frames") {
      parsed.maxFrames = Number(value);
      if (!Number.isSafeInteger(parsed.maxFrames) || parsed.maxFrames < 0) {
        throw new Error("--max-frames must be an unsigned safe integer");
      }
    } else throw new Error(`Unknown option: ${flag}`);
  }
  return parsed;
}

function runtimeName() {
  return process.versions.bun ? `Bun ${process.versions.bun}` : `Node ${process.versions.node}`;
}

function rounded(value) {
  return Math.round(value * 100) / 100;
}
