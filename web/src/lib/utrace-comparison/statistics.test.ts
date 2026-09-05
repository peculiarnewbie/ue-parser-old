import { describe, expect, it } from "vitest";
import type { UtraceDashboard } from "../types";
import {
  compareFrameDistributions,
  quantileProfile,
  summarizeFrameDistribution,
} from "./statistics";

function dashboardWithFrames(
  durationsMs: number[],
  renderingDurationsMs: number[] = [],
): UtraceDashboard {
  const frames = [...durationsMs.map((duration, index) => ({ duration, index, type: 0 })),
    ...renderingDurationsMs.map((duration, index) => ({ duration, index: durationsMs.length + index, type: 1 }))]
    .map(({ duration, index, type }) => ({
      frame_number: index,
      frame_type: type,
      begin_cycle: index * 1_000,
      end_cycle: index * 1_000 + duration * 1_000,
      duration_cycles: duration * 1_000,
      duration_seconds: duration / 1_000,
      gpu_submitted_work_count: 0,
      gpu_submitted_work_cycles: 0,
    }));
  return {
    schema_version: 1,
    status: "ok",
    path: "test.utrace",
    dashboard: {
      prologue: { cycle_frequency: 1_000_000 },
      frame_timing: { total_frame_count: frames.length, frames },
    },
  } as unknown as UtraceDashboard;
}

describe("frame distribution comparison", () => {
  it("uses every completed frame of the selected type", () => {
    const summary = summarizeFrameDistribution({
      dashboard: dashboardWithFrames([10, 12, 20], [100]),
      frameType: 0,
      budgetMs: 16.67,
    });

    expect(summary?.count).toBe(3);
    expect(summary?.samplesMs).toEqual([10, 12, 20]);
    expect(summary?.budgetMissCount).toBe(1);
    expect(summary?.budgetMissRate).toBeCloseTo(1 / 3);
  });

  it("detects a material tail regression", () => {
    const comparison = compareFrameDistributions({
      baseline: dashboardWithFrames([10, 10, 10, 10]),
      candidate: dashboardWithFrames([10, 10, 10, 14]),
      frameType: 0,
      budgetMs: 12,
    });

    expect(comparison.verdict.kind).toBe("regression");
    expect(comparison.candidate?.budgetMissCount).toBe(1);
  });

  it("returns deterministic empirical quantiles", () => {
    expect(quantileProfile([4, 1, 3, 2], [0, 0.5, 0.99])).toEqual([1, 3, 4]);
  });

  it("does not issue a verdict for an incomplete frame population", () => {
    const candidate = dashboardWithFrames([100]);
    candidate.dashboard.frame_timing!.total_frame_count = 100;
    const result = compareFrameDistributions({ baseline: dashboardWithFrames([10]), candidate, frameType: 0, budgetMs: 16 });
    expect(result.candidate?.evidence).toBe("prefix_sampled");
    expect(result.verdict.kind).toBe("inconclusive");
  });

  it("recognizes a material regression from a zero-duration baseline", () => {
    const result = compareFrameDistributions({ baseline: dashboardWithFrames([0]), candidate: dashboardWithFrames([1]), frameType: 0, budgetMs: 16 });
    expect(result.verdict.kind).toBe("regression");
  });
});
