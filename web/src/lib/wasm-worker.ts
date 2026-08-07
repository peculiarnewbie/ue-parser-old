/// <reference lib="webworker" />

type WasmParseRequest = {
  id: number;
  kind: "uasset-inspect" | "utrace-inventory" | "utrace-dashboard" | "utrace-dashboard-bundle";
  filename: string;
  bytes: ArrayBuffer;
  options: Record<string, number | undefined>;
};

type WasmProgressRequest =
  | { id: number; kind: "utrace-progress-start"; session_id: number; filename: string; total_bytes: number; options: Record<string, number | undefined> }
  | { id: number; kind: "utrace-progress-chunk"; session_id: number; bytes: ArrayBuffer }
  | { id: number; kind: "utrace-progress-analyzing"; session_id: number }
  | { id: number; kind: "utrace-progress-finish"; session_id: number }
  | { id: number; kind: "utrace-progress-query"; session_id: number; options: Record<string, number | string | undefined> }
  | { id: number; kind: "utrace-progress-gpu-query"; session_id: number; options: Record<string, number | undefined> }
  | { id: number; kind: "utrace-progress-cancel"; session_id: number };

type WasmRequest = WasmParseRequest | WasmProgressRequest;

type WasmResponse =
  | { id: number; ok: true; json: string; timing: WorkerTiming; sent_at: number }
  | { id: number; ok: false; error: string; timing: WorkerTiming; sent_at: number };

export type WorkerTiming = {
  worker_startup_ms?: number;
  wasm_threads?: boolean;
  wasm_copy_ms: number;
  parse_ms: number;
  /** Measured in the page so it uses one clock for send and receive. */
  worker_round_trip_ms?: number;
};

type WasmModule = {
  default: () => Promise<void>;
  initThreadPool?: (threads: number) => Promise<void>;
  parse: (kind: string, filename: string, bytes: Uint8Array, options: string) => string;
  ProgressiveUtraceSession: new (
    filename: string,
    totalBytes: number,
    options: string,
  ) => {
    push_chunk: (bytes: Uint8Array) => string;
    analyzing: () => string;
    finish: () => string;
    query_timeline: (options: string) => string;
    query_gpu_timeline: (options: string) => string;
    free: () => void;
  };
};

let modulePromise: Promise<WasmModule> | null = null;
let threadPoolPromise: Promise<void> | null = null;
const wasmThreads = self.crossOriginIsolated && typeof SharedArrayBuffer !== "undefined";
const progressiveSessions = new Map<number, InstanceType<WasmModule["ProgressiveUtraceSession"]>>();

async function wasm(): Promise<WasmModule> {
  if (!modulePromise) {
    modulePromise = wasmThreads
      ? import("../generated/wasm/uasset_parser_wasm.js") as Promise<WasmModule>
      : import("../generated/wasm-single/uasset_parser_wasm.js") as Promise<WasmModule>;
  }
  const loaded = await modulePromise;
  await loaded.default();
  if (loaded.initThreadPool && !threadPoolPromise) {
    const hardwareThreads = navigator.hardwareConcurrency || 1;
    const workerThreads = Math.max(1, Math.min(8, hardwareThreads - 1));
    threadPoolPromise = loaded.initThreadPool(workerThreads);
  }
  await threadPoolPromise;
  return loaded;
}

self.onmessage = async (event: MessageEvent<WasmRequest>) => {
  const request = event.data;
  const started = performance.now();
  try {
    const loaded = await wasm();
    const afterInit = performance.now();
    if (request.kind === "utrace-progress-start") {
      progressiveSessions.get(request.session_id)?.free();
      progressiveSessions.set(
        request.session_id,
        new loaded.ProgressiveUtraceSession(
          request.filename,
          request.total_bytes,
          JSON.stringify(request.options),
        ),
      );
      self.postMessage({
        id: request.id,
        ok: true,
        json: "[]",
        timing: {
          worker_startup_ms: afterInit - started,
          wasm_threads: wasmThreads,
          wasm_copy_ms: 0,
          parse_ms: performance.now() - afterInit,
        },
        sent_at: performance.now(),
      } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-cancel") {
      progressiveSessions.get(request.session_id)?.free();
      progressiveSessions.delete(request.session_id);
      self.postMessage({ id: request.id, ok: true, json: "[]", timing: { wasm_copy_ms: 0, parse_ms: 0 }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-chunk") {
      const session = progressiveSessions.get(request.session_id);
      if (!session) throw new Error("unknown progressive WASM session");
      const beforeParse = performance.now();
      const json = session.push_chunk(new Uint8Array(request.bytes));
      self.postMessage({ id: request.id, ok: true, json, timing: { wasm_copy_ms: beforeParse - afterInit, parse_ms: performance.now() - beforeParse }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-finish") {
      const session = progressiveSessions.get(request.session_id);
      if (!session) throw new Error("unknown progressive WASM session");
      const beforeParse = performance.now();
      const json = session.finish();
      self.postMessage({ id: request.id, ok: true, json, timing: { wasm_copy_ms: 0, parse_ms: performance.now() - beforeParse }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-query") {
      const session = progressiveSessions.get(request.session_id);
      if (!session) throw new Error("unknown progressive WASM session");
      const beforeParse = performance.now();
      const json = session.query_timeline(JSON.stringify(request.options));
      self.postMessage({ id: request.id, ok: true, json, timing: { wasm_copy_ms: 0, parse_ms: performance.now() - beforeParse }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-gpu-query") {
      const session = progressiveSessions.get(request.session_id);
      if (!session) throw new Error("unknown progressive WASM session");
      const beforeParse = performance.now();
      const json = session.query_gpu_timeline(JSON.stringify(request.options));
      self.postMessage({ id: request.id, ok: true, json, timing: { wasm_copy_ms: 0, parse_ms: performance.now() - beforeParse }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-analyzing") {
      const session = progressiveSessions.get(request.session_id);
      if (!session) throw new Error("unknown progressive WASM session");
      const beforeParse = performance.now();
      const json = session.analyzing();
      self.postMessage({ id: request.id, ok: true, json, timing: { wasm_copy_ms: 0, parse_ms: performance.now() - beforeParse }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    const view = new Uint8Array(request.bytes);
    const beforeParse = performance.now();
    const json = loaded.parse(request.kind, request.filename, view, JSON.stringify(request.options));
    const parsed = performance.now();
    const response: WasmResponse = {
      id: request.id,
      ok: true,
      json,
      timing: {
        worker_startup_ms: afterInit - started,
        wasm_copy_ms: beforeParse - afterInit,
        parse_ms: parsed - beforeParse,
      },
      sent_at: 0,
    };
    self.postMessage({ ...response, sent_at: performance.now() });
  } catch (error) {
    const response: WasmResponse = {
      id: request.id,
      ok: false,
      error: error instanceof Error ? error.message : String(error),
      timing: {
        wasm_copy_ms: 0,
        parse_ms: performance.now() - started,
      },
      sent_at: 0,
    };
    self.postMessage({ ...response, sent_at: performance.now() });
  }
};
