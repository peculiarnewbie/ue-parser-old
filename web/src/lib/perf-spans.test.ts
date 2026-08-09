import { describe, expect, it } from "vitest";
import { UTRACE_SPAN, type UtraceBenchmarkSnapshot } from "./perf-span-types";
import { toChromeTrace } from "./perf-spans";

describe("UTrace telemetry export", () => {
  it("exports completed spans as Chrome duration events", () => {
    const snapshot: UtraceBenchmarkSnapshot = {
      schema_version: 1,
      generated_at: "2026-08-09T00:00:00.000Z",
      spans: [
        {
          trace_id: "trace",
          span_id: "span",
          name: UTRACE_SPAN.dashboardPaint,
          domain: "render",
          start_time_unix_ms: 1000.25,
          duration_ms: 12.5,
          status: "ok",
          attributes: { "utrace.interval_count": 2500 },
        },
      ],
    };

    expect(toChromeTrace(snapshot)).toEqual({
      traceEvents: [
        {
          name: UTRACE_SPAN.dashboardPaint,
          cat: "render",
          ph: "X",
          ts: 1_000_250,
          dur: 12_500,
          pid: 1,
          tid: "render",
          args: { "utrace.interval_count": 2500 },
        },
      ],
    });
  });
});
