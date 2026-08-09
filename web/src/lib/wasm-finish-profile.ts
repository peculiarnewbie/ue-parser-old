import { UTRACE_SPAN, type UtraceSpanName } from "./perf-span-types";

export const WASM_FINISH_PHASES = [
  "transport_finalize",
  "inventory",
  "event_registry",
  "important_events",
  "provider_important_events",
  "normal_event_dispatch",
  "cpu_aggregation",
  "provider_finalize",
  "cpu_timeline_finalize",
  "gpu_timeline_finalize",
  "output_construction",
  "json_serialize",
] as const;

export type WasmFinishPhase = (typeof WASM_FINISH_PHASES)[number];

export type WasmFinishProfile = {
  schema_version: 1;
  phases: Array<{
    phase: WasmFinishPhase;
    started_ms: number;
    duration_ms: number;
  }>;
};

export function parseWasmFinishProfile(json: string): WasmFinishProfile {
  const value: unknown = JSON.parse(json);
  if (!isRecord(value) || value.schema_version !== 1 || !Array.isArray(value.phases)) {
    throw new Error("invalid WASM finish profile envelope");
  }
  if (value.phases.length !== WASM_FINISH_PHASES.length) {
    throw new Error("incomplete WASM finish profile");
  }
  const phases = value.phases.map((phase, index) => {
    const expectedPhase = WASM_FINISH_PHASES[index]!;
    if (
      !isRecord(phase) ||
      phase.phase !== expectedPhase ||
      !isNonNegativeFiniteNumber(phase.started_ms) ||
      !isNonNegativeFiniteNumber(phase.duration_ms)
    ) {
      throw new Error(`invalid WASM finish phase at index ${index}`);
    }
    return {
      phase: expectedPhase,
      started_ms: phase.started_ms,
      duration_ms: phase.duration_ms,
    };
  });
  return { schema_version: 1, phases };
}

export function finishPhaseSpanName(phase: WasmFinishPhase): UtraceSpanName {
  switch (phase) {
    case "transport_finalize":
      return UTRACE_SPAN.sessionFinishTransport;
    case "inventory":
      return UTRACE_SPAN.sessionFinishInventory;
    case "event_registry":
      return UTRACE_SPAN.sessionFinishEventRegistry;
    case "important_events":
      return UTRACE_SPAN.sessionFinishImportantEvents;
    case "provider_important_events":
      return UTRACE_SPAN.sessionFinishProviderImportantEvents;
    case "normal_event_dispatch":
      return UTRACE_SPAN.sessionFinishNormalDispatch;
    case "cpu_aggregation":
      return UTRACE_SPAN.sessionFinishCpuAggregation;
    case "provider_finalize":
      return UTRACE_SPAN.sessionFinishProviderFinalize;
    case "cpu_timeline_finalize":
      return UTRACE_SPAN.sessionFinishCpuTimeline;
    case "gpu_timeline_finalize":
      return UTRACE_SPAN.sessionFinishGpuTimeline;
    case "output_construction":
      return UTRACE_SPAN.sessionFinishOutput;
    case "json_serialize":
      return UTRACE_SPAN.sessionFinishJson;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value != null;
}

function isNonNegativeFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}
