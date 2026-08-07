import { ParseRequestError, type TimedResult } from "./api";
import type { WorkerTiming } from "./wasm-worker";
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
    if (event.data.ok && event.data.json != null) {
      entry.resolve({
        json: event.data.json,
        timing: {
          ...event.data.timing,
          worker_round_trip_ms: performance.now() - entry.sentAt,
        },
      });
    }
    else entry.reject(new Error(event.data.error ?? "WASM parser worker failed"));
  };
  worker.onerror = (event) => {
    const error = new Error(event.message || "WASM parser worker crashed");
    for (const entry of pending.values()) entry.reject(error);
    pending.clear();
    worker?.terminate();
    worker = null;
  };
  return worker;
}

export function cancelWasmParsing(): void {
  worker?.terminate();
  worker = null;
  for (const entry of pending.values()) entry.reject(new Error("WASM parsing cancelled"));
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
    pending.set(id, { resolve, reject, sentAt });
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
  message: Record<string, unknown>,
  transfers: Transferable[] = [],
): Promise<{ json: string; timing: WorkerTiming }> {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject, sentAt: performance.now() });
    getWorker().postMessage({ ...message, id }, transfers);
  });
}

export async function parseUtraceProgressWithWasm(
  file: File,
  options: UtraceDashboardQuery,
  onEvent: (event: UtraceProgressEvent) => void,
  signal?: AbortSignal,
): Promise<TimedResult<UtraceDashboard>> {
  const started = performance.now();
  const sessionId = nextId++;
  let parseMs = 0;
  let finalDashboard: UtraceDashboard | undefined;
  const dispatch = (json: string) => {
    const decoded = JSON.parse(json) as UtraceProgressEvent | UtraceProgressEvent[];
    for (const event of Array.isArray(decoded) ? decoded : [decoded]) {
      onEvent(event);
      if (event.type === "complete") finalDashboard = event.dashboard;
    }
  };
  const startupTiming = (await workerCall({
    kind: "utrace-progress-start",
    session_id: sessionId,
    filename: file.name,
    total_bytes: file.size,
    options,
  })).timing;
  const reader = file.stream().getReader();
  const abort = async () => {
    await reader.cancel().catch(() => undefined);
    await workerCall({ kind: "utrace-progress-cancel", session_id: sessionId }).catch(() => undefined);
  };
  const abortListener = () => void abort();
  signal?.addEventListener("abort", abortListener, { once: true });
  try {
    for (;;) {
      if (signal?.aborted) throw new DOMException("WASM parsing cancelled", "AbortError");
      const { value, done } = await reader.read();
      if (done) break;
      for (let offset = 0; offset < value.byteLength; offset += 1024 * 1024) {
        const chunk = value.slice(offset, Math.min(value.byteLength, offset + 1024 * 1024));
        const result = await workerCall(
          { kind: "utrace-progress-chunk", session_id: sessionId, bytes: chunk.buffer },
          [chunk.buffer],
        );
        parseMs += result.timing.parse_ms;
        dispatch(result.json);
      }
    }
    const analyzing = await workerCall({
      kind: "utrace-progress-analyzing",
      session_id: sessionId,
    });
    dispatch(analyzing.json);
    const result = await workerCall({ kind: "utrace-progress-finish", session_id: sessionId });
    parseMs += result.timing.parse_ms;
    dispatch(result.json);
  } catch (error) {
    await abort();
    throw error;
  } finally {
    signal?.removeEventListener("abort", abortListener);
  }
  if (!finalDashboard) throw new ParseRequestError(422, { error: "WASM progressive session ended without completion" });
  return {
    data: finalDashboard,
    sessionId: String(sessionId),
    timing: {
      backend: "wasm",
      client_ms: performance.now() - started,
      json_parse_ms: 0,
      parse_ms: parseMs,
      worker_startup_ms: startupTiming.worker_startup_ms,
      wasm_threads: startupTiming.wasm_threads,
    },
  };
}

export async function queryUtraceTimelineWithWasm(
  sessionId: string,
  options: {
    start_cycle?: number;
    end_cycle?: number;
    thread?: number;
    search?: string;
    limit?: number;
  },
): Promise<TimedResult<UtraceTimelineQuery>> {
  const parsedSessionId = Number(sessionId);
  if (!Number.isSafeInteger(parsedSessionId) || parsedSessionId < 0) {
    throw new ParseRequestError(422, { error: "invalid browser timeline session" });
  }
  const started = performance.now();
  const result = await workerCall({
    kind: "utrace-progress-query",
    session_id: parsedSessionId,
    options,
  });
  try {
    return {
      data: JSON.parse(result.json) as UtraceTimelineQuery,
      timing: {
        backend: "wasm",
        client_ms: performance.now() - started,
        json_parse_ms: 0,
        ...result.timing,
      },
      sessionId,
    };
  } catch {
    throw new ParseRequestError(422, { error: "WASM timeline query returned non-JSON output" });
  }
}

export async function queryUtraceGpuTimelineWithWasm(
  sessionId: string,
  options: { frame_number: number; limit?: number },
): Promise<TimedResult<UtraceGpuTimelineQuery>> {
  const parsedSessionId = Number(sessionId);
  if (!Number.isSafeInteger(parsedSessionId) || parsedSessionId < 0) {
    throw new ParseRequestError(422, { error: "invalid browser timeline session" });
  }
  const started = performance.now();
  const result = await workerCall({
    kind: "utrace-progress-gpu-query",
    session_id: parsedSessionId,
    options,
  });
  try {
    return {
      data: JSON.parse(result.json) as UtraceGpuTimelineQuery,
      timing: {
        backend: "wasm",
        client_ms: performance.now() - started,
        json_parse_ms: 0,
        ...result.timing,
      },
      sessionId,
    };
  } catch {
    throw new ParseRequestError(422, { error: "WASM GPU timeline query returned non-JSON output" });
  }
}
