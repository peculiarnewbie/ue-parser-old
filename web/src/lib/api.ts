import type { ParseErrorBody } from "./types";

/** A browser-side parse failure with the same readable shape as parser errors. */
export class ParseRequestError extends Error {
  readonly status: number;
  readonly body: ParseErrorBody;
  readonly timing: ParseTiming | null;

  constructor(status: number, body: ParseErrorBody, timing: ParseTiming | null = null) {
    super(body.stderr || body.error || `parse failed (${status})`);
    this.name = "ParseRequestError";
    this.status = status;
    this.body = body;
    this.timing = timing;
  }
}

/** Timing from the browser Worker and its Rust/WASM parser. */
export type ParseTiming = {
  backend: "wasm";
  client_ms: number;
  json_parse_ms: number;
  input_read_ms?: number;
  worker_startup_ms?: number;
  wasm_threads?: boolean;
  wasm_copy_ms?: number;
  parse_ms?: number;
  worker_round_trip_ms?: number;
};

export type TimedResult<T> = {
  data: T;
  timing: ParseTiming;
  /** Identifies a retained progressive Worker session, not a file hash. */
  sessionId?: string;
};

export type UtraceDashboardQuery = {
  max_frames?: number;
  frame?: number;
  timeline_limit?: number;
  gpu_frame?: number;
  gpu_timeline_limit?: number;
};

export function formatParseTiming(timing: ParseTiming): string {
  const parts = [`browser ${formatMs(timing.client_ms)}`];
  if (timing.wasm_threads != null) {
    parts.push(timing.wasm_threads ? "threaded WASM" : "single-thread WASM");
  }
  if (timing.input_read_ms != null && timing.input_read_ms > 1) {
    parts.push(`read ${formatMs(timing.input_read_ms)}`);
  }
  if (timing.worker_startup_ms != null && timing.worker_startup_ms > 1) {
    parts.push(`worker ${formatMs(timing.worker_startup_ms)}`);
  }
  if (timing.wasm_copy_ms != null && timing.wasm_copy_ms > 1) {
    parts.push(`WASM copy ${formatMs(timing.wasm_copy_ms)}`);
  }
  if (timing.parse_ms != null && timing.parse_ms > 1) {
    parts.push(`parse ${formatMs(timing.parse_ms)}`);
  }
  if (timing.worker_round_trip_ms != null && timing.worker_round_trip_ms > 1) {
    parts.push(`worker round-trip ${formatMs(timing.worker_round_trip_ms)}`);
  }
  if (timing.json_parse_ms > 1) {
    parts.push(`JSON ${formatMs(timing.json_parse_ms)}`);
  }
  return parts.join(" · ");
}

function formatMs(value: number): string {
  if (value < 1000) return `${Math.round(value)}ms`;
  return `${(value / 1000).toFixed(2)}s`;
}
