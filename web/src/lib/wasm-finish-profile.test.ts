import { describe, expect, it } from "vitest";
import { UTRACE_SPAN } from "./perf-span-types";
import {
  WASM_FINISH_PHASES,
  finishPhaseSpanName,
  parseWasmFinishProfile,
} from "./wasm-finish-profile";

describe("WASM finish profile", () => {
  it("validates every phase in contract order", () => {
    const profile = parseWasmFinishProfile(
      JSON.stringify({
        schema_version: 1,
        phases: WASM_FINISH_PHASES.map((phase, index) => ({
          phase,
          started_ms: 100 + index,
          duration_ms: index / 10,
        })),
      }),
    );

    expect(profile.phases).toHaveLength(WASM_FINISH_PHASES.length);
    expect(profile.phases.at(-1)?.phase).toBe("json_serialize");
    expect(finishPhaseSpanName("cpu_aggregation")).toBe(
      UTRACE_SPAN.sessionFinishCpuAggregation,
    );
  });

  it("rejects missing or reordered phases", () => {
    expect(() =>
      parseWasmFinishProfile(
        JSON.stringify({
          schema_version: 1,
          phases: [...WASM_FINISH_PHASES].reverse().map((phase) => ({
            phase,
            started_ms: 1,
            duration_ms: 1,
          })),
        }),
      ),
    ).toThrow("invalid WASM finish phase");
  });
});
