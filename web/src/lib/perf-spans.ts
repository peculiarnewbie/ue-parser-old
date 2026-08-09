import {
  SpanStatusCode,
  context,
  trace,
  type Span,
} from "@opentelemetry/api";
import {
  InMemorySpanExporter,
  SimpleSpanProcessor,
  type ReadableSpan,
} from "@opentelemetry/sdk-trace-base";
import { WebTracerProvider } from "@opentelemetry/sdk-trace-web";
import {
  UTRACE_SPAN,
  type ChromeTraceEvent,
  type RemoteUtraceSpan,
  type UtraceBenchmarkSnapshot,
  type UtraceSpanAttributes,
  type UtraceSpanAttributeValue,
  type UtraceSpanDomain,
  type UtraceSpanName,
  type UtraceSpanRecord,
} from "./perf-span-types";

const exporter = new InMemorySpanExporter();
const provider = new WebTracerProvider({
  spanProcessors: [new SimpleSpanProcessor(exporter)],
});
provider.register();

const tracer = trace.getTracer("ue-shed-utrace-web", "0.1.0");
const knownSpanNames = new Set<UtraceSpanName>(Object.values(UTRACE_SPAN));

export type UtraceSpanHandle = {
  readonly span: Span;
  readonly traceId: string;
};

export function beginUtraceSpan(input: {
  name: UtraceSpanName;
  domain: UtraceSpanDomain;
  parent?: UtraceSpanHandle;
  attributes?: UtraceSpanAttributes;
  startTimeUnixMs?: number;
}): UtraceSpanHandle {
  const attributes = compactAttributes({
    "utrace.domain": input.domain,
    ...input.attributes,
  });
  const parentContext = input.parent
    ? trace.setSpan(context.active(), input.parent.span)
    : undefined;
  const span = tracer.startSpan(
    input.name,
    {
      attributes,
      startTime:
        input.startTimeUnixMs == null
          ? undefined
          : new Date(input.startTimeUnixMs),
    },
    parentContext,
  );
  return { span, traceId: span.spanContext().traceId };
}

export function endUtraceSpan(
  handle: UtraceSpanHandle,
  input: { error?: unknown; endTimeUnixMs?: number; attributes?: UtraceSpanAttributes } = {},
): void {
  if (input.attributes) handle.span.setAttributes(compactAttributes(input.attributes));
  if (input.error != null) {
    const message = input.error instanceof Error ? input.error.message : String(input.error);
    handle.span.recordException(input.error instanceof Error ? input.error : new Error(message));
    handle.span.setStatus({ code: SpanStatusCode.ERROR, message });
  } else {
    handle.span.setStatus({ code: SpanStatusCode.OK });
  }
  handle.span.end(
    input.endTimeUnixMs == null ? undefined : new Date(input.endTimeUnixMs),
  );
}

export function recordRemoteUtraceSpans(input: {
  spans: readonly RemoteUtraceSpan[] | undefined;
  parent?: UtraceSpanHandle;
}): void {
  for (const remote of input.spans ?? []) {
    const span = beginUtraceSpan({
      name: remote.name,
      domain: remote.domain,
      parent: input.parent,
      attributes: remote.attributes,
      startTimeUnixMs: remote.start_time_unix_ms,
    });
    endUtraceSpan(span, {
      endTimeUnixMs: remote.start_time_unix_ms + remote.duration_ms,
    });
  }
}

export async function afterNextPaint(): Promise<void> {
  await Promise.resolve();
  await new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}

export function resetUtraceTelemetry(): void {
  exporter.reset();
}

export function snapshotUtraceTelemetry(traceId?: string): UtraceBenchmarkSnapshot {
  return {
    schema_version: 1,
    generated_at: new Date().toISOString(),
    spans: exporter
      .getFinishedSpans()
      .map(toRecord)
      .filter((span) => traceId == null || span.trace_id === traceId)
      .sort((left, right) => left.start_time_unix_ms - right.start_time_unix_ms),
  };
}

export function toChromeTrace(
  snapshot: UtraceBenchmarkSnapshot = snapshotUtraceTelemetry(),
): { traceEvents: ChromeTraceEvent[] } {
  return {
    traceEvents: snapshot.spans.map((span) => ({
      name: span.name,
      cat: span.domain,
      ph: "X",
      ts: Math.round(span.start_time_unix_ms * 1000),
      dur: Math.max(0, Math.round(span.duration_ms * 1000)),
      pid: 1,
      tid: span.domain,
      args: span.attributes,
    })),
  };
}

function toRecord(span: ReadableSpan): UtraceSpanRecord {
  if (!knownSpanNames.has(span.name as UtraceSpanName)) {
    throw new Error(`unexpected UTrace telemetry span: ${span.name}`);
  }
  const domain = span.attributes["utrace.domain"];
  if (
    domain !== "browser" &&
    domain !== "worker" &&
    domain !== "wasm" &&
    domain !== "render"
  ) {
    throw new Error(`invalid UTrace telemetry domain for ${span.name}`);
  }
  return {
    trace_id: span.spanContext().traceId,
    span_id: span.spanContext().spanId,
    parent_span_id: span.parentSpanContext?.spanId,
    name: span.name as UtraceSpanName,
    domain,
    start_time_unix_ms: hrTimeToMilliseconds(span.startTime),
    duration_ms: hrTimeToMilliseconds(span.duration),
    status:
      span.status.code === SpanStatusCode.OK
        ? "ok"
        : span.status.code === SpanStatusCode.ERROR
          ? "error"
          : "unset",
    attributes: serializableAttributes(span.attributes),
  };
}

function compactAttributes(
  attributes: UtraceSpanAttributes,
): Record<string, UtraceSpanAttributeValue> {
  return Object.fromEntries(
    Object.entries(attributes).filter(
      (entry): entry is [string, UtraceSpanAttributeValue] => entry[1] !== undefined,
    ),
  );
}

function serializableAttributes(
  attributes: ReadableSpan["attributes"],
): Record<string, UtraceSpanAttributeValue> {
  const entries: [string, UtraceSpanAttributeValue][] = [];
  for (const [key, value] of Object.entries(attributes)) {
    if (value == null) continue;
    if (
      typeof value === "string" ||
      typeof value === "number" ||
      typeof value === "boolean" ||
      Array.isArray(value)
    ) {
      entries.push([key, value as UtraceSpanAttributeValue]);
    }
  }
  return Object.fromEntries(entries);
}

function hrTimeToMilliseconds(time: readonly [number, number]): number {
  return time[0] * 1000 + time[1] / 1_000_000;
}

declare global {
  interface Window {
    __UTRACE_BENCHMARK__: {
      reset: typeof resetUtraceTelemetry;
      snapshot: typeof snapshotUtraceTelemetry;
      chromeTrace: typeof toChromeTrace;
    };
  }
}

if (typeof window !== "undefined") {
  window.__UTRACE_BENCHMARK__ = {
    reset: resetUtraceTelemetry,
    snapshot: snapshotUtraceTelemetry,
    chromeTrace: toChromeTrace,
  };
}
