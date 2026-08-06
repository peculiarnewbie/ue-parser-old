import initWasm, {
  ProgressiveUtraceSession as RawProgressiveUtraceSession,
  dashboardBundleUtrace,
  dashboardUtrace,
  inspectUtrace,
  inventoryUtrace,
} from "./wasm/utrace_parser_wasm.js";

export { parserManifest } from "./manifest.js";
export { createUtraceParserWorker, UtraceParserWorkerError } from "./worker-client.js";

export const UTRACE_SCHEMA_VERSION = 2;

let initialization = null;

export class UtraceParserError extends Error {
  constructor(message, options = {}) {
    super(message, options);
    this.name = "UtraceParserError";
  }
}

/**
 * Instantiates the bundled browser WebAssembly module. All parse helpers call
 * this automatically, so explicit initialization is only useful for eager
 * loading.
 */
export function init() {
  if (!initialization) {
    initialization = Promise.resolve(initWasm()).catch((cause) => {
      initialization = null;
      throw new UtraceParserError("Failed to initialize the UTrace parser WebAssembly module", {
        cause,
      });
    });
  }
  return initialization;
}

export async function inspect(input) {
  const { bytes, filename } = readInput(input);
  await init();
  return decodeEnvelope("trace", callWasm("inspect the UTrace capture", () => inspectUtrace(filename, bytes)));
}

export async function inventory(input) {
  const { bytes, filename } = readInput(input);
  await init();
  return decodeEnvelope(
    "inventory",
    callWasm("build the UTrace inventory", () => inventoryUtrace(filename, bytes)),
  );
}

export async function dashboard(input) {
  const { bytes, filename, options } = readDashboardInput(input);
  await init();
  return decodeEnvelope(
    "dashboard",
    callWasm("build the UTrace dashboard", () => dashboardUtrace(filename, bytes, options)),
  );
}

export async function dashboardBundle(input) {
  const { bytes, filename, options } = readDashboardInput(input);
  await init();
  const value = decodeJson(
    callWasm("build the UTrace dashboard and inventory", () =>
      dashboardBundleUtrace(filename, bytes, options),
    ),
  );
  assertEnvelope(value, "dashboard");
  assertJsonObject(value.inventory, "UTrace inventory");
  return value;
}

export async function createProgressiveDashboard(input) {
  const { filename, totalBytes, options } = readProgressiveInput(input);
  await init();
  const raw = callWasm(
    "create the progressive UTrace dashboard",
    () => new RawProgressiveUtraceSession(filename, totalBytes, options),
  );
  return new ProgressiveDashboardSession(raw);
}

export class ProgressiveDashboardSession {
  #raw;
  #closed = false;

  constructor(raw) {
    this.#raw = raw;
  }

  get closed() {
    return this.#closed;
  }

  pushChunk(bytes) {
    this.#assertOpen();
    assertBytes(bytes);
    return decodeProgressEvents(
      callWasm("append a UTrace chunk", () => this.#raw.push_chunk(bytes)),
    );
  }

  analyzing() {
    this.#assertOpen();
    const event = decodeProgressEvent(
      decodeJson(callWasm("update UTrace analysis progress", () => this.#raw.analyzing())),
    );
    if (event.type !== "snapshot") {
      throw new UtraceParserError("The progressive UTrace session returned an invalid analysis event");
    }
    return event;
  }

  finish() {
    this.#assertOpen();
    this.#closed = true;
    try {
      const event = decodeProgressEvent(
        decodeJson(callWasm("finish the progressive UTrace dashboard", () => this.#raw.finish())),
      );
      if (event.type !== "complete") {
        throw new UtraceParserError("The progressive UTrace session returned an invalid completion event");
      }
      return event;
    } finally {
      this.#raw.free();
    }
  }

  dispose() {
    if (!this.#closed) {
      this.#closed = true;
      this.#raw.free();
    }
  }

  #assertOpen() {
    if (this.#closed) {
      throw new UtraceParserError("The progressive UTrace session is already closed");
    }
  }
}

function readInput(input) {
  assertJsonObject(input, "UTrace parser input");
  assertBytes(input.bytes);
  const filename = input.filename ?? "trace.utrace";
  if (typeof filename !== "string" || filename.length === 0) {
    throw new TypeError("UTrace parser input filename must be a non-empty string");
  }
  return { bytes: input.bytes, filename };
}

function readDashboardInput(input) {
  const parsed = readInput(input);
  return { ...parsed, options: encodeDashboardOptions(input.options) };
}

function readProgressiveInput(input) {
  assertJsonObject(input, "Progressive UTrace parser input");
  const filename = input.filename ?? "trace.utrace";
  if (typeof filename !== "string" || filename.length === 0) {
    throw new TypeError("Progressive UTrace parser input filename must be a non-empty string");
  }
  assertUnsignedInteger(input.totalBytes, "totalBytes", Number.MAX_SAFE_INTEGER);
  return {
    filename,
    totalBytes: input.totalBytes,
    options: encodeDashboardOptions(input.options),
  };
}

function encodeDashboardOptions(options = {}) {
  assertJsonObject(options, "Dashboard options");
  return JSON.stringify({
    max_frames: optionalUnsignedInteger(options.maxFrames, "maxFrames", Number.MAX_SAFE_INTEGER),
    frame: optionalUnsignedInteger(options.timelineFrame, "timelineFrame", 0xffff_ffff),
    timeline_limit: optionalUnsignedInteger(
      options.timelineLimit,
      "timelineLimit",
      Number.MAX_SAFE_INTEGER,
    ),
    gpu_frame: optionalUnsignedInteger(options.gpuTimelineFrame, "gpuTimelineFrame", 0xffff_ffff),
    gpu_timeline_limit: optionalUnsignedInteger(
      options.gpuTimelineLimit,
      "gpuTimelineLimit",
      Number.MAX_SAFE_INTEGER,
    ),
  });
}

function optionalUnsignedInteger(value, name, maximum) {
  if (value === undefined) return undefined;
  assertUnsignedInteger(value, name, maximum);
  return value;
}

function assertUnsignedInteger(value, name, maximum) {
  if (!Number.isSafeInteger(value) || value < 0 || value > maximum) {
    throw new TypeError(`${name} must be an unsigned safe integer no greater than ${maximum}`);
  }
}

function assertBytes(value) {
  if (!(value instanceof Uint8Array)) {
    throw new TypeError("UTrace parser input bytes must be a Uint8Array");
  }
}

function decodeEnvelope(bodyKey, text) {
  const value = decodeJson(text);
  assertEnvelope(value, bodyKey);
  return value;
}

function decodeProgressEvents(text) {
  const value = decodeJson(text);
  if (!Array.isArray(value)) {
    throw new UtraceParserError("The progressive UTrace session returned a non-array event payload");
  }
  return value.map(decodeProgressEvent);
}

function decodeProgressEvent(value) {
  assertJsonObject(value, "Progressive UTrace event");
  if (value.type !== "bootstrap" && value.type !== "snapshot" && value.type !== "complete") {
    throw new UtraceParserError("The progressive UTrace session returned an unknown event type");
  }
  if (!Number.isSafeInteger(value.protocol_version) || !Number.isSafeInteger(value.sequence)) {
    throw new UtraceParserError("The progressive UTrace session returned an invalid event sequence");
  }
  assertJsonObject(value.progress, "Progressive UTrace event progress");
  return value;
}

function decodeJson(text) {
  try {
    return JSON.parse(text);
  } catch (cause) {
    throw new UtraceParserError("The UTrace parser returned invalid JSON", { cause });
  }
}

function assertEnvelope(value, bodyKey) {
  assertJsonObject(value, "UTrace parser response");
  if (
    value.schema_version !== UTRACE_SCHEMA_VERSION ||
    value.status !== "ok" ||
    typeof value.path !== "string"
  ) {
    throw new UtraceParserError("The UTrace parser returned an incompatible response envelope");
  }
  assertJsonObject(value[bodyKey], `UTrace parser response ${bodyKey}`);
}

function assertJsonObject(value, name) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`${name} must be an object`);
  }
}

function callWasm(operation, invoke) {
  try {
    return invoke();
  } catch (cause) {
    throw new UtraceParserError(`Failed to ${operation}`, { cause });
  }
}
