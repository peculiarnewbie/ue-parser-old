import { ParseRequestError, type TimedResult } from "./api";
import type { WorkerTiming } from "./wasm-worker";
import { UTRACE_SPAN } from "./perf-span-types";
import {
  beginUtraceSpan,
  endUtraceSpan,
  recordRemoteUtraceSpans,
  type UtraceSpanHandle,
} from "./perf-spans";
import type {
  UtraceDashboard,
  UtraceGpuTimelineQuery,
  UtraceProgressEvent,
  UtraceTimelineQuery,
} from "./types";
import type { UtraceDashboardQuery } from "./api";

type WasmOperation = "uasset-inspect" | "utrace-inventory" | "utrace-dashboard" | "utrace-dashboard-bundle";

type Pending = {
  resolve: (value: { json: string; timing: WorkerTiming }) => void;
  reject: (reason: Error) => void;
  sentAt: number;
  parentSpan?: UtraceSpanHandle;
  roundTripSpan: UtraceSpanHandle;
};

let worker: Worker | null = null;
let nextId = 1;
const pending = new Map<number, Pending>();

function getWorker(): Worker {
  if (worker) return worker;
  worker = new Worker(new URL("./wasm-worker.ts", import.meta.url), { type: "module" });
  worker.onmessage = (event: MessageEvent<{ id: number; ok: boolean; json?: string; error?: string; timing: WorkerTiming; sent_at: number }>) => {
    const entry = pending.get(event.data.id);
    if (!entry) return;
    pending.delete(event.data.id);
    recordRemoteUtraceSpans({ spans: event.data.timing.spans, parent: entry.parentSpan });
    if (event.data.ok && event.data.json != null) {
      endUtraceSpan(entry.roundTripSpan, {
        attributes: { "worker.response_bytes": event.data.json.length },
      });
      entry.resolve({
        json: event.data.json,
        timing: {
          ...event.data.timing,
          worker_round_trip_ms: performance.now() - entry.sentAt,
        },
      });
    }
    else {
      const error = new Error(event.data.error ?? "WASM parser worker failed");
      endUtraceSpan(entry.roundTripSpan, { error });
      entry.reject(error);
    }
  };
  worker.onerror = (event) => {
    const error = new Error(event.message || "WASM parser worker crashed");
    for (const entry of pending.values()) {
      endUtraceSpan(entry.roundTripSpan, { error });
      entry.reject(error);
    }
    pending.clear();
    worker?.terminate();
    worker = null;
  };
  return worker;
}

export function cancelWasmParsing(): void {
  worker?.terminate();
  worker = null;
  const error = new Error("WASM parsing cancelled");
  for (const entry of pending.values()) {
    endUtraceSpan(entry.roundTripSpan, { error });
    entry.reject(error);
  }
  pending.clear();
}

export async function parseWithWasm<T>(request: { kind: WasmOperation; file: File; options?: Record<string, number | undefined> }): Promise<TimedResult<T>> {
  const started = performance.now();
  const readStarted = performance.now();
  const bytes = await request.file.arrayBuffer();
  const inputReadMs = performance.now() - readStarted;
  const id = nextId++;
  const result = await new Promise<{ json: string; timing: WorkerTiming }>((resolve, reject) => {
    const sentAt = performance.now();
    const roundTripSpan = beginUtraceSpan({
      name: UTRACE_SPAN.workerRoundTrip,
      domain: "browser",
      attributes: { "worker.request_kind": request.kind },
    });
    pending.set(id, { resolve, reject, sentAt, roundTripSpan });
    getWorker().postMessage({
      id,
      kind: request.kind,
      filename: request.file.name,
      bytes,
      options: request.options ?? {},
    }, [bytes]);
  });
  const jsonStarted = performance.now();
  try {
    return {
      data: JSON.parse(result.json) as T,
      timing: {
        backend: "wasm",
        client_ms: performance.now() - started,
        json_parse_ms: performance.now() - jsonStarted,
        input_read_ms: inputReadMs,
        ...result.timing,
      },
    };
  } catch {
    throw new ParseRequestError(422, { error: "WASM parser returned non-JSON output" });
  }
}

async function workerCall(
  input: {
    message: Record<string, unknown>;
    transfers?: Transferable[];
    parentSpan?: UtraceSpanHandle;
  },
): Promise<{ json: string; timing: WorkerTiming }> {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const requestKind = String(input.message.kind ?? "unknown");
    const roundTripSpan = beginUtraceSpan({
      name: UTRACE_SPAN.workerRoundTrip,
      domain: "browser",
      parent: input.parentSpan,
      attributes: { "worker.request_kind": requestKind },
    });
    pending.set(id, {
      resolve,
      reject,
      sentAt: performance.now(),
      parentSpan: input.parentSpan,
      roundTripSpan,
    });
    getWorker().postMessage({ ...input.message, id }, input.transfers ?? []);
  });
}

export async function parseUtraceProgressWithWasm(
  input: {
    file: File;
    options: UtraceDashboardQuery;
    onEvent: (event: UtraceProgressEvent) => void;
    signal?: AbortSignal;
    parentSpan?: UtraceSpanHandle;
  },
): Promise<TimedResult<UtraceDashboard>> {
  const started = performance.now();
  const sessionId = nextId++;
  let parseMs = 0;
  let jsonParseMs = 0;
  let finalDashboard: UtraceDashboard | undefined;
  const dispatch = (json: string) => {
    const jsonSpan = beginUtraceSpan({
      name: UTRACE_SPAN.jsonDecode,
      domain: "browser",
      parent: input.parentSpan,
      attributes: { "json.bytes": json.length },
    });
    const jsonStarted = performance.now();
    const decoded = JSON.parse(json) as UtraceProgressEvent | UtraceProgressEvent[];
    jsonParseMs += performance.now() - jsonStarted;
    endUtraceSpan(jsonSpan, {
      attributes: { "json.event_count": Array.isArray(decoded) ? decoded.length : 1 },
    });
    for (const event of Array.isArray(decoded) ? decoded : [decoded]) {
      input.onEvent(event);
      if (event.type === "complete") finalDashboard = event.dashboard;
    }
  };
  const startupTiming = (await workerCall({
    message: {
      kind: "utrace-progress-start",
      session_id: sessionId,
      filename: input.file.name,
      total_bytes: input.file.size,
      options: input.options,
    },
    parentSpan: input.parentSpan,
  })).timing;
  const reader = input.file.stream().getReader();
  const fileSpan = beginUtraceSpan({
    name: UTRACE_SPAN.fileStream,
    domain: "browser",
    parent: input.parentSpan,
    attributes: {
      "utrace.filename": input.file.name,
      "utrace.file_bytes": input.file.size,
    },
  });
  let fileSpanEnded = false;
  const endFileSpan = (input: { error?: unknown } = {}) => {
    if (fileSpanEnded) return;
    fileSpanEnded = true;
    endUtraceSpan(fileSpan, {
      ...input,
      attributes: { "utrace.chunk_count": chunkCount },
    });
  };
  let chunkCount = 0;
  const abort = async () => {
    await reader.cancel().catch(() => undefined);
    await workerCall({
      message: { kind: "utrace-progress-cancel", session_id: sessionId },
      parentSpan: input.parentSpan,
    }).catch(() => undefined);
  };
  const abortListener = () => void abort();
  input.signal?.addEventListener("abort", abortListener, { once: true });
  try {
    for (;;) {
      if (input.signal?.aborted) throw new DOMException("WASM parsing cancelled", "AbortError");
      const { value, done } = await reader.read();
      if (done) break;
      for (let offset = 0; offset < value.byteLength; offset += 1024 * 1024) {
        const chunk = value.slice(offset, Math.min(value.byteLength, offset + 1024 * 1024));
        chunkCount += 1;
        const result = await workerCall({
          message: { kind: "utrace-progress-chunk", session_id: sessionId, bytes: chunk.buffer },
          transfers: [chunk.buffer],
          parentSpan: input.parentSpan,
        });
        parseMs += result.timing.parse_ms;
        dispatch(result.json);
      }
    }
    endFileSpan();
    const analyzing = await workerCall({
      message: {
        kind: "utrace-progress-analyzing",
        session_id: sessionId,
      },
      parentSpan: input.parentSpan,
    });
    dispatch(analyzing.json);
    const result = await workerCall({
      message: { kind: "utrace-progress-finish", session_id: sessionId },
      parentSpan: input.parentSpan,
    });
    parseMs += result.timing.parse_ms;
    dispatch(result.json);
  } catch (error) {
    endFileSpan({ error });
    await abort();
    throw error;
  } finally {
    input.signal?.removeEventListener("abort", abortListener);
  }
  if (!finalDashboard) throw new ParseRequestError(422, { error: "WASM progressive session ended without completion" });
  return {
    data: finalDashboard,
    sessionId: String(sessionId),
    timing: {
      backend: "wasm",
      client_ms: performance.now() - started,
      json_parse_ms: jsonParseMs,
      parse_ms: parseMs,
      worker_startup_ms: startupTiming.worker_startup_ms,
      wasm_threads: startupTiming.wasm_threads,
    },
  };
}

export async function queryUtraceTimelineWithWasm(
  input: {
    sessionId: string;
    options: {
      start_cycle?: number;
      end_cycle?: number;
      thread?: number;
      search?: string;
      limit?: number;
    };
    parentSpan?: UtraceSpanHandle;
  },
): Promise<TimedResult<UtraceTimelineQuery>> {
  const parsedSessionId = Number(input.sessionId);
  if (!Number.isSafeInteger(parsedSessionId) || parsedSessionId < 0) {
    throw new ParseRequestError(422, { error: "invalid browser timeline session" });
  }
  const started = performance.now();
  const result = await workerCall({
    message: {
      kind: "utrace-progress-query",
      session_id: parsedSessionId,
      options: input.options,
    },
    parentSpan: input.parentSpan,
  });
  const jsonSpan = beginUtraceSpan({
    name: UTRACE_SPAN.jsonDecode,
    domain: "browser",
    parent: input.parentSpan,
    attributes: { "json.bytes": result.json.length, "utrace.operation": "timeline" },
  });
  const jsonStarted = performance.now();
  try {
    const data = JSON.parse(result.json) as UtraceTimelineQuery;
    const jsonParseMs = performance.now() - jsonStarted;
    endUtraceSpan(jsonSpan);
    return {
      data,
      timing: {
        backend: "wasm",
        client_ms: performance.now() - started,
        json_parse_ms: jsonParseMs,
        ...result.timing,
      },
      sessionId: input.sessionId,
    };
  } catch (error) {
    endUtraceSpan(jsonSpan, { error });
    throw new ParseRequestError(422, { error: "WASM timeline query returned non-JSON output" });
  }
}

export async function queryUtraceGpuTimelineWithWasm(
  input: {
    sessionId: string;
    options: { frame_number: number; limit?: number };
    parentSpan?: UtraceSpanHandle;
  },
): Promise<TimedResult<UtraceGpuTimelineQuery>> {
  const parsedSessionId = Number(input.sessionId);
  if (!Number.isSafeInteger(parsedSessionId) || parsedSessionId < 0) {
    throw new ParseRequestError(422, { error: "invalid browser timeline session" });
  }
  const started = performance.now();
  const result = await workerCall({
    message: {
      kind: "utrace-progress-gpu-query",
      session_id: parsedSessionId,
      options: input.options,
    },
    parentSpan: input.parentSpan,
  });
  const jsonSpan = beginUtraceSpan({
    name: UTRACE_SPAN.jsonDecode,
    domain: "browser",
    parent: input.parentSpan,
    attributes: { "json.bytes": result.json.length, "utrace.operation": "gpu_timeline" },
  });
  const jsonStarted = performance.now();
  try {
    const data = JSON.parse(result.json) as UtraceGpuTimelineQuery;
    const jsonParseMs = performance.now() - jsonStarted;
    endUtraceSpan(jsonSpan);
    return {
      data,
      timing: {
        backend: "wasm",
        client_ms: performance.now() - started,
        json_parse_ms: jsonParseMs,
        ...result.timing,
      },
      sessionId: input.sessionId,
    };
  } catch (error) {
    endUtraceSpan(jsonSpan, { error });
    throw new ParseRequestError(422, { error: "WASM GPU timeline query returned non-JSON output" });
  }
}
