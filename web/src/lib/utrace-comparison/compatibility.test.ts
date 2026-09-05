import { describe, expect, it } from "vitest";
import type { UtraceDashboard } from "../types";
import { assessComparability, compareProviderCoverage } from "./compatibility";

function dashboard(input: {
  project?: string;
  platform?: string;
  frequency?: number;
  frames?: number;
  cpu?: boolean;
  gpu?: boolean;
} = {}): UtraceDashboard {
  const frameCount = input.frames ?? 10;
  return {
    schema_version: 1,
    status: "ok",
    path: "test.utrace",
    dashboard: {
      prologue: input.frequency === 0 ? undefined : { cycle_frequency: input.frequency ?? 1_000_000 },
      session: {
        project_name: input.project ?? "Game",
        platform: input.platform ?? "Win64",
      },
      frame_timing: {
        total_frame_count: frameCount,
        frames: Array.from({ length: frameCount }, (_, frame_number) => ({ frame_number })),
      },
      cpu: { scopes: input.cpu === false ? [] : [{ spec_id: 1 }] },
      gpu: { queues: input.gpu === false ? [] : [{ queue_id: 1 }] },
      counters: { counters: [] },
      stats: { stats: [], sample_events: 0 },
      memory: { allocs: { count: 0 }, llm: { sample_events: 0 } },
      io_store: { requests_created: 0 },
      platform_file: { file_count: 0 },
      tasks: { created: 0, started: 0 },
      annotations: { bookmarks: { events: 0 }, regions: { completed: 0 } },
    },
  } as unknown as UtraceDashboard;
}

describe("capture comparability", () => {
  it("treats missing frame or timebase data as metric-specific blockers", () => {
    const result = assessComparability(
      dashboard({ frames: 0 }),
      dashboard({ frequency: 0 }),
    );

    expect(result.status).toBe("incompatible");
    expect(result.issues.map((issue) => issue.id)).toEqual([
      "missing-frames",
      "missing-timebase",
    ]);
  });

  it("warns about workload and length differences without prohibiting all metrics", () => {
    const result = assessComparability(
      dashboard({ project: "GameA", platform: "Win64", frames: 10 }),
      dashboard({ project: "GameB", platform: "Linux", frames: 30 }),
    );

    expect(result.status).toBe("review");
    expect(result.issues.map((issue) => issue.id)).toEqual([
      "project",
      "platform",
      "capture-length",
    ]);
  });

  it("reports observed provider absence instead of a zero value", () => {
    const coverage = compareProviderCoverage(
      dashboard({ cpu: true, gpu: false }),
      dashboard({ cpu: false, gpu: true }),
    );

    expect(coverage.find((provider) => provider.id === "cpu")).toMatchObject({
      baseline: true,
      candidate: false,
      evidence: "unavailable",
    });
    expect(coverage.find((provider) => provider.id === "gpu")).toMatchObject({
      baseline: false,
      candidate: true,
      evidence: "unavailable",
    });
  });
});
