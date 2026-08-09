/// <reference lib="webworker" />

import {
  UTRACE_SPAN,
  type RemoteUtraceSpan,
  type UtraceSpanAttributes,
  type UtraceSpanName,
} from "./perf-span-types";
import { finishPhaseSpanName, parseWasmFinishProfile } from "./wasm-finish-profile";

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
  spans: RemoteUtraceSpan[];
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
    finish_profile: () => string;
    query_timeline: (options: string) => string;
    query_gpu_timeline: (options: string) => string;
    free: () => void;
  };
};

let modulePromise: Promise<WasmModule> | null = null;
let wasmInitializationPromise: Promise<void> | null = null;
let threadPoolPromise: Promise<void> | null = null;
const wasmThreads = self.crossOriginIsolated && typeof SharedArrayBuffer !== "undefined";
const progressiveSessions = new Map<number, InstanceType<WasmModule["ProgressiveUtraceSession"]>>();

async function wasm(spans: RemoteUtraceSpan[]): Promise<WasmModule> {
  if (!modulePromise) {
    const importStarted = performance.now();
    modulePromise = (wasmThreads
      ? import("../generated/wasm/uasset_parser_wasm.js") as Promise<WasmModule>
      : import("../generated/wasm-single/uasset_parser_wasm.js") as Promise<WasmModule>)
      .then((loaded) => {
        spans.push(remoteSpan({
          name: UTRACE_SPAN.wasmModuleImport,
          domain: "worker",
          started: importStarted,
          attributes: { "wasm.threaded": wasmThreads },
        }));
        return loaded;
      });
  }
  const loaded = await modulePromise;
  if (!wasmInitializationPromise) {
    const initializationStarted = performance.now();
    wasmInitializationPromise = loaded.default().then(() => {
      spans.push(remoteSpan({
        name: UTRACE_SPAN.wasmInitialize,
        domain: "worker",
        started: initializationStarted,
        attributes: { "wasm.threaded": wasmThreads },
      }));
    });
  }
  await wasmInitializationPromise;
  if (loaded.initThreadPool && !threadPoolPromise) {
    const hardwareThreads = navigator.hardwareConcurrency || 1;
    const workerThreads = Math.max(1, Math.min(16, hardwareThreads - 1));
    const poolStarted = performance.now();
    threadPoolPromise = loaded.initThreadPool(workerThreads).then(() => {
      spans.push(remoteSpan({
        name: UTRACE_SPAN.wasmThreadPoolInitialize,
        domain: "worker",
        started: poolStarted,
        attributes: { "wasm.worker_count": workerThreads },
      }));
    });
  }
  await threadPoolPromise;
  return loaded;
}

self.onmessage = async (event: MessageEvent<WasmRequest>) => {
  const request = event.data;
  const started = performance.now();
  const spans: RemoteUtraceSpan[] = [];
  try {
    const loaded = await wasm(spans);
    const afterInit = performance.now();
    if (request.kind === "utrace-progress-start") {
      const operationStarted = performance.now();
      progressiveSessions.get(request.session_id)?.free();
      progressiveSessions.set(
        request.session_id,
        new loaded.ProgressiveUtraceSession(
          request.filename,
          request.total_bytes,
          JSON.stringify(request.options),
        ),
      );
      spans.push(remoteSpan({
        name: UTRACE_SPAN.sessionStart,
        domain: "wasm",
        started: operationStarted,
        attributes: {
          "utrace.file_bytes": request.total_bytes,
          "utrace.filename": request.filename,
        },
      }));
      spans.push(requestSpan(request, started));
      self.postMessage({
        id: request.id,
        ok: true,
        json: "[]",
        timing: {
          worker_startup_ms: afterInit - started,
          wasm_threads: wasmThreads,
          wasm_copy_ms: 0,
          parse_ms: performance.now() - afterInit,
          spans,
        },
        sent_at: performance.now(),
      } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-cancel") {
      const operationStarted = performance.now();
      progressiveSessions.get(request.session_id)?.free();
      progressiveSessions.delete(request.session_id);
      spans.push(remoteSpan({ name: UTRACE_SPAN.sessionCancel, domain: "wasm", started: operationStarted }));
      spans.push(requestSpan(request, started));
      self.postMessage({ id: request.id, ok: true, json: "[]", timing: { wasm_copy_ms: 0, parse_ms: 0, spans }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-chunk") {
      const session = progressiveSessions.get(request.session_id);
      if (!session) throw new Error("unknown progressive WASM session");
      const beforeParse = performance.now();
      const json = session.push_chunk(new Uint8Array(request.bytes));
      const parseMs = performance.now() - beforeParse;
      spans.push(remoteSpan({
        name: UTRACE_SPAN.sessionPush,
        domain: "wasm",
        started: beforeParse,
        attributes: { "utrace.chunk_bytes": request.bytes.byteLength },
      }));
      spans.push(requestSpan(request, started));
      self.postMessage({ id: request.id, ok: true, json, timing: { wasm_copy_ms: beforeParse - afterInit, parse_ms: parseMs, spans }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-finish") {
      const session = progressiveSessions.get(request.session_id);
      if (!session) throw new Error("unknown progressive WASM session");
      const beforeParse = performance.now();
      const json = session.finish();
      const finishedAt = performance.now();
      const parseMs = finishedAt - beforeParse;
      const finishProfile = parseWasmFinishProfile(session.finish_profile());
      spans.push(completedRemoteSpan({
        name: UTRACE_SPAN.sessionFinish,
        domain: "wasm",
        started: beforeParse,
        completed: finishedAt,
      }));
      for (const phase of finishProfile.phases) {
        spans.push(completedRemoteSpan({
          name: finishPhaseSpanName(phase.phase),
          domain: "wasm",
          started: phase.started_ms,
          completed: phase.started_ms + phase.duration_ms,
          attributes: { "wasm.finish_phase": phase.phase },
        }));
      }
      spans.push(requestSpan(request, started));
      self.postMessage({ id: request.id, ok: true, json, timing: { wasm_copy_ms: 0, parse_ms: parseMs, spans }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-query") {
      const session = progressiveSessions.get(request.session_id);
      if (!session) throw new Error("unknown progressive WASM session");
      const beforeParse = performance.now();
      const json = session.query_timeline(JSON.stringify(request.options));
      const parseMs = performance.now() - beforeParse;
      spans.push(remoteSpan({ name: UTRACE_SPAN.timelineQuery, domain: "wasm", started: beforeParse }));
      spans.push(requestSpan(request, started));
      self.postMessage({ id: request.id, ok: true, json, timing: { wasm_copy_ms: 0, parse_ms: parseMs, spans }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-gpu-query") {
      const session = progressiveSessions.get(request.session_id);
      if (!session) throw new Error("unknown progressive WASM session");
      const beforeParse = performance.now();
      const json = session.query_gpu_timeline(JSON.stringify(request.options));
      const parseMs = performance.now() - beforeParse;
      spans.push(remoteSpan({ name: UTRACE_SPAN.gpuTimelineQuery, domain: "wasm", started: beforeParse }));
      spans.push(requestSpan(request, started));
      self.postMessage({ id: request.id, ok: true, json, timing: { wasm_copy_ms: 0, parse_ms: parseMs, spans }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    if (request.kind === "utrace-progress-analyzing") {
      const session = progressiveSessions.get(request.session_id);
      if (!session) throw new Error("unknown progressive WASM session");
      const beforeParse = performance.now();
      const json = session.analyzing();
      const parseMs = performance.now() - beforeParse;
      spans.push(remoteSpan({ name: UTRACE_SPAN.sessionAnalyzing, domain: "wasm", started: beforeParse }));
      spans.push(requestSpan(request, started));
      self.postMessage({ id: request.id, ok: true, json, timing: { wasm_copy_ms: 0, parse_ms: parseMs, spans }, sent_at: performance.now() } satisfies WasmResponse);
      return;
    }
    const view = new Uint8Array(request.bytes);
    const beforeParse = performance.now();
    const json = loaded.parse(request.kind, request.filename, view, JSON.stringify(request.options));
    const parsed = performance.now();
    spans.push(remoteSpan({
      name: UTRACE_SPAN.wasmParse,
      domain: "wasm",
      started: beforeParse,
      attributes: {
        "utrace.operation": request.kind,
        "utrace.input_bytes": request.bytes.byteLength,
      },
    }));
    spans.push(requestSpan(request, started));
    const response: WasmResponse = {
      id: request.id,
      ok: true,
      json,
      timing: {
        worker_startup_ms: afterInit - started,
        wasm_copy_ms: beforeParse - afterInit,
        parse_ms: parsed - beforeParse,
        spans,
      },
      sent_at: 0,
    };
    self.postMessage({ ...response, sent_at: performance.now() });
  } catch (error) {
    spans.push(requestSpan(request, started, { "error.type": error instanceof Error ? error.name : "unknown" }));
    const response: WasmResponse = {
      id: request.id,
      ok: false,
      error: error instanceof Error ? error.message : String(error),
      timing: {
        wasm_copy_ms: 0,
        parse_ms: performance.now() - started,
        spans,
      },
      sent_at: 0,
    };
    self.postMessage({ ...response, sent_at: performance.now() });
  }
};

function requestSpan(
  request: WasmRequest,
  started: number,
  attributes: UtraceSpanAttributes = {},
): RemoteUtraceSpan {
  return remoteSpan({
    name: UTRACE_SPAN.workerRequest,
    domain: "worker",
    started,
    attributes: {
      "worker.request_kind": request.kind,
      ...attributes,
    },
  });
}

function remoteSpan(input: {
  name: UtraceSpanName;
  domain: "worker" | "wasm";
  started: number;
  attributes?: UtraceSpanAttributes;
}): RemoteUtraceSpan {
  return completedRemoteSpan({ ...input, completed: performance.now() });
}

function completedRemoteSpan(input: {
  name: UtraceSpanName;
  domain: "worker" | "wasm";
  started: number;
  completed: number;
  attributes?: UtraceSpanAttributes;
}): RemoteUtraceSpan {
  return {
    name: input.name,
    domain: input.domain,
    start_time_unix_ms: performance.timeOrigin + input.started,
    duration_ms: Math.max(0, input.completed - input.started),
    attributes: input.attributes,
  };
}
